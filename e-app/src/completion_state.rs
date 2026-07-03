//! LSP-driven completion, hover, signature help and go-to-definition, plus the
//! Laravel/Livewire/Inertia-aware completion glue.
//!
//! Extracted from the former `state.rs` god-module (fields stay on `AppState`);
//! same pattern as [`crate::debug`] / [`crate::runtime`].

use std::rc::Rc;

use floem::ext_event::create_ext_action;
use floem::kurbo::Point;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use floem::views::editor::core::cursor::{Cursor, CursorMode};
use floem::views::editor::core::editor::EditType;
use floem::views::editor::core::selection::Selection;
use floem::views::editor::text::Document;

use e_core::language::Language;
use e_lsp::{path_to_uri, SignatureInfo};

use crate::laravel::{self, LaravelData};
use crate::state::{dedup_by_label, is_word_char, line_indent, word_start, AppState};
use crate::{builtin_completion, framework_completion, snippets};

impl AppState {
    // ---- Completion & hover --------------------------------------------

    /// After an edit in a PHP buffer, decide whether to (re)trigger completion.
    pub fn autocomplete_after_edit(&self, buffer_id: u64) {
        // Laravel helper strings take priority over generic PHP completion.
        if self.try_laravel_completion(buffer_id) {
            return;
        }
        // Schema-aware completion inside raw SQL strings (DB::select("…"), …).
        if self.try_sql_completion(buffer_id) {
            return;
        }
        let Some(buf) = self.buffer_by_id(buffer_id) else {
            return;
        };
        let Some(editor) = buf.editor.get_untracked() else {
            return;
        };
        let offset = editor.cursor.get_untracked().offset();
        let text = buf.doc.text().to_string();
        let before: Vec<char> = text[..offset.min(text.len())].chars().collect();
        let last = before.last().copied();
        let prev = before.get(before.len().wrapping_sub(2)).copied();

        // Signature help on call punctuation.
        match last {
            Some('(') | Some(',') => self.request_signature_help(buffer_id),
            Some(')') => self.close_signature(),
            _ => {}
        }

        let trigger = match last {
            Some(c) if is_word_char(c) => true,
            Some('>') => prev == Some('-'),
            Some(':') => prev == Some(':'),
            _ => false,
        };

        if trigger {
            self.request_completion(buffer_id);
        } else {
            self.close_completion();
        }
    }

    /// Schema-aware completion when the cursor is inside a raw-SQL string in PHP:
    /// table names after FROM/JOIN/…, column names elsewhere, from the live DB
    /// schema cache. Returns whether it presented completions.
    pub(crate) fn try_sql_completion(&self, buffer_id: u64) -> bool {
        let Some(buf) = self.buffer_by_id(buffer_id) else {
            return false;
        };
        if buf.file.language != Language::Php {
            return false;
        }
        let Some(editor) = buf.editor.get_untracked() else {
            return false;
        };
        let cursor = editor.cursor.get_untracked();
        let offset = cursor.offset();
        let text = buf.doc.text().to_string();
        let Some((rs, _re)) = e_core::syntax::php_sql_range_at(&text, offset) else {
            return false;
        };
        if offset < rs || offset > text.len() {
            return false;
        }
        let (prefix, wants_tables) = sql_prefix_and_context(&text[rs..offset]);
        let start = offset - prefix.len();

        let schema = self.db_schema_cache.get_untracked();
        if schema.is_empty() {
            return false;
        }
        let lower = prefix.to_lowercase();
        let mut items: Vec<lsp_types::CompletionItem> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut push = |name: &str, kind: lsp_types::CompletionItemKind, detail: String| {
            if !name.to_lowercase().contains(&lower) {
                return;
            }
            if !seen.insert(name.to_string()) {
                return;
            }
            items.push(lsp_types::CompletionItem {
                label: name.to_string(),
                insert_text: Some(name.to_string()),
                kind: Some(kind),
                detail: Some(detail),
                ..Default::default()
            });
        };

        let mut tables: Vec<&String> = schema.keys().collect();
        tables.sort();
        if !wants_tables {
            // Columns first (most likely), then tables.
            let mut cols: Vec<&e_db::ColumnInfo> = schema.values().flatten().collect();
            cols.sort_by(|a, b| a.name.cmp(&b.name));
            for c in cols {
                push(
                    &c.name,
                    lsp_types::CompletionItemKind::FIELD,
                    format!("column · {}", c.data_type),
                );
            }
        }
        for t in tables {
            push(t, lsp_types::CompletionItemKind::CLASS, "table".to_string());
        }
        if items.is_empty() {
            return false;
        }

        let (_, below) = editor.points_of_offset(start, cursor.affinity);
        let vp = editor.viewport.get_untracked();
        let win = buf.win_origin.get_untracked();
        let comp = self.completion;
        comp.anchor
            .set(Point::new(win.x + below.x - vp.x0, win.y + below.y - vp.y0));
        comp.buffer_id.set(Some(buffer_id));
        comp.start_offset.set(start);
        comp.items.set(items);
        comp.selected.set(0);
        comp.open.set(true);
        true
    }

    pub fn request_completion(&self, buffer_id: u64) {
        if self.try_laravel_completion(buffer_id) {
            return;
        }
        let Some(buf) = self.buffer_by_id(buffer_id) else {
            return;
        };
        let Some(editor) = buf.editor.get_untracked() else {
            return;
        };

        let cursor = editor.cursor.get_untracked();
        let offset = cursor.offset();
        let (line, col) = editor.offset_to_line_col(offset);

        let text = buf.doc.text().to_string();

        // Framework-aware completion (Flux UI, Livewire, Tailwind, Vue, Svelte)
        // takes priority and replaces the whole multi-segment token.
        let upto = offset.min(text.len());
        let line_start = text[..upto].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let line_before = &text[line_start..upto];

        // Livewire: `wire:model="…"` completes from the component's public props.
        if let Some(partial) = crate::livewire::wire_model_partial(line_before) {
            if let Some(items) = self.livewire_property_items(&buf, &partial) {
                let comp = self.completion;
                let fstart = offset.saturating_sub(partial.len());
                let (_, below) = editor.points_of_offset(fstart, cursor.affinity);
                let vp = editor.viewport.get_untracked();
                let win = buf.win_origin.get_untracked();
                let anchor = Point::new(win.x + below.x - vp.x0, win.y + below.y - vp.y0);
                comp.buffer_id.set(Some(buffer_id));
                comp.start_offset.set(fstart);
                comp.anchor.set(anchor);
                comp.items.set(items);
                comp.selected.set(0);
                comp.open.set(true);
                return;
            }
        }

        // Inertia: `Inertia::render('…')` completes from existing page components.
        if let Some(partial) = crate::inertia::render_partial(line_before) {
            let pages = crate::inertia::list_pages(&self.root.get_untracked());
            let lower = partial.to_lowercase();
            let items: Vec<lsp_types::CompletionItem> = pages
                .into_iter()
                .filter(|p| lower.is_empty() || p.to_lowercase().contains(&lower))
                .map(|p| lsp_types::CompletionItem {
                    label: p.clone(),
                    insert_text: Some(p.clone()),
                    kind: Some(lsp_types::CompletionItemKind::FILE),
                    detail: Some("Inertia page".to_string()),
                    ..Default::default()
                })
                .collect();
            if !items.is_empty() {
                let comp = self.completion;
                let fstart = offset.saturating_sub(partial.len());
                let (_, below) = editor.points_of_offset(fstart, cursor.affinity);
                let vp = editor.viewport.get_untracked();
                let win = buf.win_origin.get_untracked();
                let anchor = Point::new(win.x + below.x - vp.x0, win.y + below.y - vp.y0);
                comp.buffer_id.set(Some(buffer_id));
                comp.start_offset.set(fstart);
                comp.anchor.set(anchor);
                comp.items.set(items);
                comp.selected.set(0);
                comp.open.set(true);
                return;
            }
        }

        // Ziggy: `route('…')` in JS completes from the Laravel route table.
        if matches!(
            buf.file.language,
            Language::TypeScript | Language::JavaScript | Language::Vue | Language::Svelte
        ) {
            if let Some(partial) = crate::inertia::route_partial(line_before) {
                if let Some(data) = self.laravel.get_untracked() {
                    let lower = partial.to_lowercase();
                    let items: Vec<lsp_types::CompletionItem> = data
                        .routes
                        .iter()
                        .filter(|r| {
                            !r.name.is_empty()
                                && (lower.is_empty() || r.name.to_lowercase().contains(&lower))
                        })
                        .map(|r| lsp_types::CompletionItem {
                            label: r.name.clone(),
                            insert_text: Some(r.name.clone()),
                            kind: Some(lsp_types::CompletionItemKind::FUNCTION),
                            detail: Some(format!("{} {}", r.methods, r.uri)),
                            ..Default::default()
                        })
                        .collect();
                    if !items.is_empty() {
                        let comp = self.completion;
                        let fstart = offset.saturating_sub(partial.len());
                        let (_, below) = editor.points_of_offset(fstart, cursor.affinity);
                        let vp = editor.viewport.get_untracked();
                        let win = buf.win_origin.get_untracked();
                        let anchor = Point::new(win.x + below.x - vp.x0, win.y + below.y - vp.y0);
                        comp.buffer_id.set(Some(buffer_id));
                        comp.start_offset.set(fstart);
                        comp.anchor.set(anchor);
                        comp.items.set(items);
                        comp.selected.set(0);
                        comp.open.set(true);
                        return;
                    }
                }
            }
        }

        // Inertia shared props: `$page.props.…` from HandleInertiaRequests::share().
        if matches!(
            buf.file.language,
            Language::TypeScript | Language::JavaScript | Language::Vue | Language::Svelte
        ) {
            if let Some(partial) = crate::inertia::props_partial(line_before) {
                let lower = partial.to_lowercase();
                let items: Vec<lsp_types::CompletionItem> =
                    crate::inertia::shared_props(&self.root.get_untracked())
                        .into_iter()
                        .filter(|p| p.to_lowercase().starts_with(&lower))
                        .map(|p| lsp_types::CompletionItem {
                            label: p.clone(),
                            insert_text: Some(p.clone()),
                            kind: Some(lsp_types::CompletionItemKind::FIELD),
                            detail: Some("Inertia shared prop".to_string()),
                            ..Default::default()
                        })
                        .collect();
                if !items.is_empty() {
                    let comp = self.completion;
                    let fstart = offset.saturating_sub(partial.len());
                    let (_, below) = editor.points_of_offset(fstart, cursor.affinity);
                    let vp = editor.viewport.get_untracked();
                    let win = buf.win_origin.get_untracked();
                    let anchor = Point::new(win.x + below.x - vp.x0, win.y + below.y - vp.y0);
                    comp.buffer_id.set(Some(buffer_id));
                    comp.start_offset.set(fstart);
                    comp.anchor.set(anchor);
                    comp.items.set(items);
                    comp.selected.set(0);
                    comp.open.set(true);
                    return;
                }
            }
        }

        // Laravel query builder: column names inside `where('…')`, `orderBy()`,
        // `select()`, … and relationship names inside `with()`, `whereHas()`.
        if matches!(buf.file.language, Language::Php | Language::Blade) {
            if let Some(ctx) = crate::querycomplete::context(line_before) {
                let root = self.root.get_untracked();
                if let Some(target) = crate::querycomplete::resolve_target(&text, offset, &root) {
                    let lower = ctx.partial.to_lowercase();
                    let items: Vec<lsp_types::CompletionItem> = if ctx.relation {
                        target
                            .model
                            .as_ref()
                            .map(|m| crate::relations::relation_names(&root, m))
                            .unwrap_or_default()
                            .into_iter()
                            .filter(|r| lower.is_empty() || r.to_lowercase().starts_with(&lower))
                            .map(|r| lsp_types::CompletionItem {
                                label: r.clone(),
                                insert_text: Some(r),
                                kind: Some(lsp_types::CompletionItemKind::REFERENCE),
                                detail: Some("relation".to_string()),
                                ..Default::default()
                            })
                            .collect()
                    } else {
                        self.db_schema_cache.with_untracked(|schema| {
                            schema
                                .get(&target.table)
                                .map(|cols| {
                                    cols.iter()
                                        .filter(|c| {
                                            lower.is_empty()
                                                || c.name.to_lowercase().starts_with(&lower)
                                        })
                                        .map(|c| lsp_types::CompletionItem {
                                            label: c.name.clone(),
                                            insert_text: Some(c.name.clone()),
                                            kind: Some(lsp_types::CompletionItemKind::FIELD),
                                            detail: Some(format!(
                                                "{} · {}",
                                                c.data_type, target.table
                                            )),
                                            ..Default::default()
                                        })
                                        .collect()
                                })
                                .unwrap_or_default()
                        })
                    };
                    if !items.is_empty() {
                        let comp = self.completion;
                        let fstart = offset.saturating_sub(ctx.partial.len());
                        let (_, below) = editor.points_of_offset(fstart, cursor.affinity);
                        let vp = editor.viewport.get_untracked();
                        let win = buf.win_origin.get_untracked();
                        let anchor = Point::new(win.x + below.x - vp.x0, win.y + below.y - vp.y0);
                        comp.buffer_id.set(Some(buffer_id));
                        comp.start_offset.set(fstart);
                        comp.anchor.set(anchor);
                        comp.items.set(items);
                        comp.selected.set(0);
                        comp.open.set(true);
                        return;
                    }
                }
            }
        }

        // Gate/policy abilities inside `can()`/`authorize()`/`@can`/`Gate::allows()`.
        if matches!(buf.file.language, Language::Php | Language::Blade) {
            if let Some(partial) =
                crate::inertia::call_string_partial(line_before, crate::policies::CALLS)
            {
                let root = self.root.get_untracked();
                let lower = partial.to_lowercase();
                let items: Vec<lsp_types::CompletionItem> = crate::policies::abilities(&root)
                    .into_iter()
                    .filter(|(n, _, _)| lower.is_empty() || n.to_lowercase().starts_with(&lower))
                    .map(|(n, _, _)| lsp_types::CompletionItem {
                        label: n.clone(),
                        insert_text: Some(n),
                        kind: Some(lsp_types::CompletionItemKind::VALUE),
                        detail: Some("ability".to_string()),
                        ..Default::default()
                    })
                    .collect();
                if !items.is_empty() {
                    let comp = self.completion;
                    let fstart = offset.saturating_sub(partial.len());
                    let (_, below) = editor.points_of_offset(fstart, cursor.affinity);
                    let vp = editor.viewport.get_untracked();
                    let win = buf.win_origin.get_untracked();
                    let anchor = Point::new(win.x + below.x - vp.x0, win.y + below.y - vp.y0);
                    comp.buffer_id.set(Some(buffer_id));
                    comp.start_offset.set(fstart);
                    comp.anchor.set(anchor);
                    comp.items.set(items);
                    comp.selected.set(0);
                    comp.open.set(true);
                    return;
                }
            }
        }

        // Validation rule names inside `validate([…])` / FormRequest `rules()`.
        if matches!(buf.file.language, Language::Php | Language::Blade) {
            if let Some(partial) = crate::validation::rule_partial(line_before) {
                let items: Vec<lsp_types::CompletionItem> = crate::validation::rule_names(&partial)
                    .into_iter()
                    .map(|r| lsp_types::CompletionItem {
                        label: r.to_string(),
                        insert_text: Some(r.to_string()),
                        kind: Some(lsp_types::CompletionItemKind::KEYWORD),
                        detail: Some("validation rule".to_string()),
                        ..Default::default()
                    })
                    .collect();
                if !items.is_empty() {
                    let comp = self.completion;
                    let fstart = offset.saturating_sub(partial.len());
                    let (_, below) = editor.points_of_offset(fstart, cursor.affinity);
                    let vp = editor.viewport.get_untracked();
                    let win = buf.win_origin.get_untracked();
                    let anchor = Point::new(win.x + below.x - vp.x0, win.y + below.y - vp.y0);
                    comp.buffer_id.set(Some(buffer_id));
                    comp.start_offset.set(fstart);
                    comp.anchor.set(anchor);
                    comp.items.set(items);
                    comp.selected.set(0);
                    comp.open.set(true);
                    return;
                }
            }
        }

        if let Some((rep, items)) =
            framework_completion::completions(buf.file.language, line_before)
        {
            let comp = self.completion;
            let fstart = offset.saturating_sub(rep);
            let (_, below) = editor.points_of_offset(fstart, cursor.affinity);
            let vp = editor.viewport.get_untracked();
            let win = buf.win_origin.get_untracked();
            let anchor = Point::new(win.x + below.x - vp.x0, win.y + below.y - vp.y0);
            comp.buffer_id.set(Some(buffer_id));
            comp.start_offset.set(fstart);
            comp.anchor.set(anchor);
            comp.items.set(items);
            comp.selected.set(0);
            comp.open.set(true);
            return;
        }

        let start = word_start(&text, offset);
        let word = text[start..offset.min(text.len())].to_string();

        // Anchor the popup at the start of the replaced word.
        let (_, below) = editor.points_of_offset(start, cursor.affinity);
        let vp = editor.viewport.get_untracked();
        let win = buf.win_origin.get_untracked();
        let anchor = Point::new(win.x + below.x - vp.x0, win.y + below.y - vp.y0);

        let comp = self.completion;
        comp.buffer_id.set(Some(buffer_id));
        comp.start_offset.set(start);
        comp.anchor.set(anchor);

        // Snippets and built-ins (keywords / buffer words / Laravel) are
        // computed synchronously; LSP results are merged in when available.
        let snippet_items = snippets::completion_items(buf.file.language, &word);
        let mut builtin_items = builtin_completion::items(buf.file.language, &word, &text);
        // Eloquent columns from the live DB schema merge in alongside LSP results.
        if let Some((_, cols)) = crate::eloquent::complete(
            buf.file.language,
            &text,
            offset,
            &self.root.get_untracked(),
            &self.db_schema_cache.get_untracked(),
        ) {
            builtin_items.extend(cols);
        }

        let show = move |items: Vec<lsp_types::CompletionItem>| {
            let items = dedup_by_label(items);
            if items.is_empty() {
                comp.open.set(false);
            } else {
                comp.items.set(items);
                comp.selected.set(0);
                comp.open.set(true);
            }
        };

        match (self.lsp_for_active(), buf.uri.clone()) {
            (Some(client), Some(uri)) => {
                let send =
                    create_ext_action(self.cx, move |lsp: Vec<lsp_types::CompletionItem>| {
                        let mut items = snippet_items.clone();
                        items.extend(lsp);
                        items.extend(builtin_items.clone());
                        show(items);
                    });
                std::thread::spawn(move || {
                    let items = client
                        .completion(&uri, line as u32, col as u32)
                        .unwrap_or_default();
                    send(items);
                });
            }
            _ => {
                let mut items = snippet_items;
                items.extend(builtin_items);
                show(items);
            }
        }
    }

    pub fn move_completion(&self, delta: i64) {
        let comp = self.completion;
        let len = comp.items.with(|i| i.len());
        if len == 0 {
            return;
        }
        let cur = comp.selected.get_untracked() as i64;
        let next = (cur + delta).clamp(0, len as i64 - 1) as usize;
        comp.selected.set(next);
    }

    pub fn close_completion(&self) {
        if self.completion.open.get_untracked() {
            self.completion.open.set(false);
        }
    }

    /// Insert the selected completion. Returns true if something was inserted.
    pub fn accept_completion(&self) -> bool {
        let comp = self.completion;
        if !comp.open.get_untracked() {
            return false;
        }
        let items = comp.items.get_untracked();
        if items.is_empty() {
            return false;
        }
        let sel = comp.selected.get_untracked().min(items.len() - 1);
        let item = &items[sel];
        let is_snippet = item.detail.as_deref() == Some("snippet");
        let insert = item
            .insert_text
            .clone()
            .unwrap_or_else(|| item.label.clone());
        let label = item.label.clone();

        let Some(bid) = comp.buffer_id.get_untracked() else {
            return false;
        };
        let Some(buf) = self.buffer_by_id(bid) else {
            return false;
        };
        let Some(editor) = buf.editor.get_untracked() else {
            return false;
        };

        let end = editor.cursor.get_untracked().offset();
        let start = comp.start_offset.get_untracked().min(end);
        comp.open.set(false);

        if is_snippet {
            if let Some(body) = snippets::body(buf.file.language, &label) {
                let text = buf.doc.text().to_string();
                let indent = line_indent(&text, start);
                let (expanded, caret) = snippets::expand(&body, &indent);
                buf.doc.edit_single(
                    Selection::region(start, end),
                    &expanded,
                    EditType::InsertChars,
                );
                let pos = start + caret;
                editor.cursor.set(Cursor::new(
                    CursorMode::Insert(Selection::caret(pos)),
                    None,
                    None,
                ));
                return true;
            }
        }

        buf.doc.edit_single(
            Selection::region(start, end),
            &insert,
            EditType::InsertChars,
        );
        // Place the caret at the end of the inserted text (not at the old
        // offset, which would land in the middle of a longer completion).
        let pos = start + insert.len();
        editor.cursor.set(Cursor::new(
            CursorMode::Insert(Selection::caret(pos)),
            None,
            None,
        ));
        true
    }

    /// Resolve the Laravel helper token under the cursor, if any.
    fn laravel_token(&self) -> Option<(laravel::Helper, String, Rc<LaravelData>)> {
        let data = self.laravel.get()?;
        let buf = self.active_buffer()?;
        let editor = buf.editor.get_untracked()?;
        let offset = editor.cursor.get_untracked().offset();
        let text = buf.doc.text().to_string();
        let upto = offset.min(text.len());
        let line_start = text[..upto].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let before = &text[line_start..upto];
        let line_end = text[upto..]
            .find('\n')
            .map(|i| upto + i)
            .unwrap_or(text.len());
        let after = &text[upto..line_end];
        let (helper, token) = laravel::token_at(before, after)?;
        Some((helper, token, data))
    }

    /// Caret on an ability string in `can()`/`authorize()`/`@can`/`Gate::allows()`
    /// jumps to the policy method or `Gate::define()` that declares it.
    fn goto_policy(&self) -> bool {
        let Some(buf) = self.active_buffer() else {
            return false;
        };
        if !matches!(buf.file.language, Language::Php | Language::Blade) {
            return false;
        }
        let Some(editor) = buf.editor.get_untracked() else {
            return false;
        };
        let text = buf.doc.text().to_string();
        let offset = editor.cursor.get_untracked().offset();
        let Some(name) = crate::inertia::call_string_at(&text, offset, crate::policies::CALLS)
        else {
            return false;
        };
        let root = self.root.get_untracked();
        if let Some((_, file, line)) = crate::policies::abilities(&root)
            .into_iter()
            .find(|(n, _, _)| *n == name)
        {
            self.jump_to(&path_to_uri(&file), line, 0);
            true
        } else {
            false
        }
    }

    /// Ziggy `route('name')` under the cursor in a JS-family file, plus the
    /// Laravel data — the same route table the PHP side uses.
    fn active_ziggy_route(&self) -> Option<(String, Rc<LaravelData>)> {
        let buf = self.active_buffer()?;
        if !matches!(
            buf.file.language,
            Language::TypeScript | Language::JavaScript | Language::Vue | Language::Svelte
        ) {
            return None;
        }
        let editor = buf.editor.get_untracked()?;
        let text = buf.doc.text().to_string();
        let offset = editor.cursor.get_untracked().offset();
        let name = crate::inertia::route_at(&text, offset)?;
        let data = self.laravel.get()?;
        Some((name, data))
    }

    pub fn request_hover(&self) {
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let Some(editor) = buf.editor.get_untracked() else {
            return;
        };
        let cursor = editor.cursor.get_untracked();
        let offset = cursor.offset();
        let (line, col) = editor.offset_to_line_col(offset);

        let (_, below) = editor.points_of_offset(offset, cursor.affinity);
        let vp = editor.viewport.get_untracked();
        let win = buf.win_origin.get_untracked();
        let anchor = Point::new(win.x + below.x - vp.x0, win.y + below.y - vp.y0);

        let hover = self.hover;
        hover.anchor.set(anchor);

        // Laravel hover takes precedence inside helper strings.
        if let Some((helper, token, data)) = self.laravel_token() {
            if let Some(text) = laravel::hover_text(&data, helper, &token) {
                hover.text.set(text);
                hover.open.set(true);
                return;
            }
        }
        // Ziggy `route('…')` hover on the JS side.
        if let Some((name, data)) = self.active_ziggy_route() {
            if let Some(text) = laravel::hover_text(&data, laravel::Helper::Route, &name) {
                hover.text.set(text);
                hover.open.set(true);
                return;
            }
        }

        let (Some(client), Some(uri)) = (self.lsp_for_active(), buf.uri.clone()) else {
            return;
        };
        let send = create_ext_action(self.cx, move |text: Option<String>| match text {
            Some(text) if !text.trim().is_empty() => {
                hover.text.set(text);
                hover.open.set(true);
            }
            _ => hover.open.set(false),
        });
        std::thread::spawn(move || {
            let text = client.hover(&uri, line as u32, col as u32).ok().flatten();
            send(text);
        });
    }

    pub fn close_hover(&self) {
        if self.hover.open.get_untracked() {
            self.hover.open.set(false);
        }
    }

    pub fn request_signature_help(&self, buffer_id: u64) {
        let Some(buf) = self.buffer_by_id(buffer_id) else {
            return;
        };
        let (Some(client), Some(uri), Some(editor)) = (
            self.lsp_for_active(),
            buf.uri.clone(),
            buf.editor.get_untracked(),
        ) else {
            return;
        };
        let cursor = editor.cursor.get_untracked();
        let offset = cursor.offset();
        let (line, col) = editor.offset_to_line_col(offset);

        // Anchor just above the caret line.
        let (above, _) = editor.points_of_offset(offset, cursor.affinity);
        let vp = editor.viewport.get_untracked();
        let win = buf.win_origin.get_untracked();
        let anchor = Point::new(win.x + above.x - vp.x0, win.y + above.y - vp.y0 - 26.0);

        let sig = self.signature;
        sig.anchor.set(anchor);
        let send = create_ext_action(self.cx, move |info: Option<SignatureInfo>| match info {
            Some(i) => {
                sig.label.set(i.label);
                sig.active
                    .set(i.active.map(|(a, b)| (a as usize, b as usize)));
                sig.open.set(true);
            }
            None => sig.open.set(false),
        });
        std::thread::spawn(move || {
            let info = client
                .signature_help(&uri, line as u32, col as u32)
                .ok()
                .flatten();
            send(info);
        });
    }

    pub fn close_signature(&self) {
        if self.signature.open.get_untracked() {
            self.signature.open.set(false);
        }
    }

    /// Jump to the definition of the symbol under the cursor (LSP).
    pub fn goto_definition(&self) {
        // Livewire: caret on a `wire:model` property jumps to its declaration.
        if self.livewire_goto() {
            return;
        }
        // Inertia: caret on `Inertia::render('Page')` jumps to the component.
        if self.goto_inertia_page() {
            return;
        }
        // Ziggy: caret on `route('name')` in JS jumps to the controller.
        if let Some((name, data)) = self.active_ziggy_route() {
            if let Some((p, l, c)) = laravel::navigate(&data, laravel::Helper::Route, &name) {
                self.jump_to(&path_to_uri(&p), l, c);
                return;
            }
        }
        // Gates/policies: caret on an ability jumps to the policy method.
        if self.goto_policy() {
            return;
        }
        // Events: caret on a dispatched event class jumps to a listener.
        if self.goto_event() {
            return;
        }
        // Laravel navigation first: route -> controller, view -> blade, etc.
        if let Some((helper, token, data)) = self.laravel_token() {
            if let Some((path, line, col)) = laravel::navigate(&data, helper, &token) {
                let uri = path_to_uri(&path);
                self.jump_to(&uri, line, col);
                return;
            }
        }
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let (Some(client), Some(uri), Some(editor)) = (
            self.lsp_for_active(),
            buf.uri.clone(),
            buf.editor.get_untracked(),
        ) else {
            return;
        };
        let (line, col) = editor.offset_to_line_col(editor.cursor.get_untracked().offset());
        let app = *self;
        let send = create_ext_action(self.cx, move |loc: Option<(String, u32, u32)>| match loc {
            Some((u, l, c)) => app.jump_to(&u, l as usize, c as usize),
            None => eprintln!("e: no definition found"),
        });
        std::thread::spawn(move || {
            let loc = client
                .definition(&uri, line as u32, col as u32)
                .ok()
                .flatten();
            send(loc);
        });
    }
}

/// Given the SQL text before the cursor, return the identifier prefix being
/// typed and whether the context wants table names (after FROM/JOIN/INTO/…)
/// rather than columns.
fn sql_prefix_and_context(sql_before: &str) -> (String, bool) {
    let bytes = sql_before.as_bytes();
    let mut i = bytes.len();
    while i > 0 && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_') {
        i -= 1;
    }
    let prefix = sql_before[i..].to_string();
    // The whitespace-delimited word right before the prefix decides the context.
    let last_word = sql_before[..i]
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .rfind(|w| !w.is_empty())
        .unwrap_or("")
        .to_ascii_uppercase();
    let wants_tables = matches!(
        last_word.as_str(),
        "FROM" | "JOIN" | "INTO" | "UPDATE" | "TABLE"
    );
    (prefix, wants_tables)
}

#[cfg(test)]
mod tests {
    use super::sql_prefix_and_context;

    #[test]
    fn sql_context_tables_after_from() {
        assert_eq!(
            sql_prefix_and_context("SELECT * FROM us"),
            ("us".into(), true)
        );
        assert_eq!(
            sql_prefix_and_context("SELECT id FROM users JOIN po"),
            ("po".into(), true)
        );
    }

    #[test]
    fn sql_context_columns_elsewhere() {
        assert_eq!(sql_prefix_and_context("SELECT i"), ("i".into(), false));
        assert_eq!(
            sql_prefix_and_context("SELECT id FROM users WHERE ac"),
            ("ac".into(), false)
        );
    }
}
