//! Navigation & search: go-to-definition targets, back/forward history, the
//! workspace symbol search, global search and in-workspace replace, plus the LSP
//! references/symbols requests behind them.
//!
//! Extracted from the former `state.rs` god-module (fields stay on `AppState`).

use std::path::PathBuf;

use floem::ext_event::create_ext_action;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use floem::views::editor::core::cursor::{Cursor, CursorMode};
use floem::views::editor::core::selection::Selection;
use floem::views::editor::text::Document;

use e_lsp::{path_to_uri, uri_to_path};

use crate::picker::{PickerItem, PickerMode};
use crate::state::{grep_workspace, rel_uri, AppState, ReplaceConfirm};
use crate::workspace_search::{self, SearchOpts};

impl AppState {
    // ---- References & symbol search ------------------------------------

    /// Open the workspace symbol search (⌘T).
    pub fn open_symbol_search(&self) {
        let p = self.picker;
        p.mode.set(PickerMode::Symbols);
        p.query.set(String::new());
        p.items.set(Vec::new());
        p.selected.set(0);
        p.open.set(true);
    }

    /// Open workspace-wide text search (⌘⇧F).
    pub fn open_global_search(&self) {
        let p = self.picker;
        p.mode.set(PickerMode::Search);
        p.query.set(String::new());
        p.replace.set(String::new());
        p.items.set(Vec::new());
        p.selected.set(0);
        p.open.set(true);
    }

    /// Search options currently set in the picker.
    pub fn search_opts(&self) -> SearchOpts {
        SearchOpts {
            case_sensitive: self.picker.case_sensitive.get_untracked(),
        }
    }

    /// Stage a workspace-wide replace: work out what it *would* rewrite and put
    /// that in front of the user. Nothing is written until they confirm — this
    /// used to write straight to disk with no preview and no undo.
    pub fn replace_in_workspace(&self) {
        let query = self.picker.query.get_untracked();
        let replacement = self.picker.replace.get_untracked();
        if query.is_empty() {
            return;
        }
        let opts = self.search_opts();
        let roots = self.roots.get_untracked();
        let confirm = self.replace_confirm;
        let planned_query = query.clone();
        let open = self.open_texts();
        let send = create_ext_action(self.cx, move |plan: workspace_search::ReplacePlan| {
            if plan.is_empty() {
                eprintln!("e: nothing to replace");
                return;
            }
            confirm.set(Some(ReplaceConfirm {
                query: query.clone(),
                replacement: replacement.clone(),
                opts,
                plan,
            }));
        });
        std::thread::spawn(move || {
            send(workspace_search::plan_replace(
                &roots,
                &planned_query,
                opts,
                &open,
            ));
        });
    }

    /// Carry out the staged replace after the user confirmed it.
    ///
    /// Only the files the preview listed are touched. Those open in the
    /// editor are replaced in the editor's text — unsaved edits included, as
    /// one change that undo takes back — and a file that had no unsaved edits
    /// is saved again. The rest are rewritten on disk, in the encoding each
    /// was read in. An open tab the walk never reached (a gitignored
    /// `vendor/` file, say) wasn't in the preview, so it is left alone.
    pub fn confirm_replace_in_workspace(&self) {
        let Some(pending) = self.replace_confirm.get_untracked() else {
            return;
        };
        self.replace_confirm.set(None);
        let roots = self.roots.get_untracked();
        let open = self.open_texts();

        let listed = pending.plan.canonical_paths();
        let mut in_editor = (0usize, 0usize); // (files, matches)
        let mut left_unsaved = 0usize;
        if let Some(re) = workspace_search::matcher(&pending.query, pending.opts) {
            for buf in self.editor.buffers.get_untracked() {
                let Some(path) = buf.file.path.clone() else {
                    continue;
                };
                if !listed.contains(&workspace_search::canonical(&path)) {
                    continue;
                }
                let text = buf.doc.text().to_string();
                let ranges = workspace_search::match_ranges(&text, &re);
                if ranges.is_empty() {
                    continue;
                }
                let was_dirty = buf.dirty.get_untracked();
                let edits = ranges
                    .iter()
                    .map(|&(s, e)| (s, e, pending.replacement.clone()))
                    .collect();
                crate::state::apply_batch(
                    &buf.doc,
                    edits,
                    floem::views::editor::core::editor::EditType::InsertChars,
                );
                in_editor.0 += 1;
                in_editor.1 += ranges.len();
                if was_dirty {
                    left_unsaved += 1;
                } else {
                    self.write_buffer(buf.id);
                }
            }
        }

        let state = *self;
        let query = pending.query.clone();
        let send = create_ext_action(self.cx, move |outcome: workspace_search::ReplaceOutcome| {
            let files = outcome.files_changed + in_editor.0;
            let matches = outcome.matches_replaced + in_editor.1;
            let mut msg = format!("Replaced {matches} match(es) in {files} file(s)");
            if left_unsaved > 0 {
                msg.push_str(&format!(
                    "; {left_unsaved} with unsaved edits are changed in the editor, not saved"
                ));
            }
            for (path, err) in &outcome.failures {
                eprintln!("e: could not write {}: {err}", path.display());
            }
            if !outcome.failures.is_empty() {
                msg.push_str(&format!("; {} couldn't be written", outcome.failures.len()));
            }
            crate::toast::post(&msg);
            state.fs_rev.update(|r| *r += 1);
            state.request_search(query.clone());
        });
        std::thread::spawn(move || {
            send(workspace_search::apply_replace(
                &roots,
                &pending.query,
                pending.opts,
                &pending.replacement,
                &open,
                &pending.plan,
            ));
        });
    }

    /// The text of every file open in the editor, by canonical path, for a
    /// search to read instead of the disk.
    pub(crate) fn open_texts(&self) -> workspace_search::OpenTexts {
        self.editor.buffers.with_untracked(|bs| {
            bs.iter()
                .filter_map(|b| {
                    let path = b.file.path.clone()?;
                    let key = path.canonicalize().unwrap_or(path);
                    Some((key, b.doc.text().to_string()))
                })
                .collect()
        })
    }

    /// Discard the staged replace.
    pub fn cancel_replace_in_workspace(&self) {
        self.replace_confirm.set(None);
    }

    /// Dispatch a picker query to the right backend for the current mode.
    pub fn run_picker_query(&self, query: String) {
        match self.picker.mode.get_untracked() {
            PickerMode::Symbols => self.request_symbols(query),
            PickerMode::Search => self.request_search(query),
            PickerMode::References => {}
        }
    }

    /// Grep the workspace for `query` (called reactively from the picker).
    ///
    /// Typing runs it once the keys pause for a moment, not once per key, and
    /// a search a newer one replaced stops where it is instead of walking
    /// the whole tree for results nobody will see.
    pub fn request_search(&self, query: String) {
        use std::sync::atomic::Ordering;
        let p = self.picker;
        if query.trim().len() < 2 {
            p.items.set(Vec::new());
            return;
        }
        let gen = p.gen.get_untracked() + 1;
        p.gen.set(gen);
        workspace_search::LATEST_SEARCH.store(gen, Ordering::Relaxed);
        let roots = self.roots.get();
        let opts = self.search_opts();
        let state = *self;
        floem::action::exec_after(workspace_search::SEARCH_PAUSE, move |_| {
            if p.gen.get_untracked() != gen {
                return; // typed on
            }
            let open = state.open_texts();
            let send = create_ext_action(state.cx, move |(g, items): (u64, Vec<PickerItem>)| {
                if g == p.gen.get_untracked() {
                    p.items.set(items);
                    p.selected.set(0);
                }
            });
            std::thread::spawn(move || {
                let cancelled = || workspace_search::LATEST_SEARCH.load(Ordering::Relaxed) != gen;
                send((
                    gen,
                    grep_workspace(&roots, &query, opts, 300, &open, &cancelled),
                ));
            });
        });
    }

    /// Run a workspace/symbol query (called reactively from the picker).
    pub fn request_symbols(&self, query: String) {
        let p = self.picker;
        let Some(client) = self.lsp_for_active() else {
            return;
        };
        if query.trim().is_empty() {
            p.items.set(Vec::new());
            return;
        }
        let gen = p.gen.get_untracked() + 1;
        p.gen.set(gen);
        let root = self.root.get();
        let send = create_ext_action(self.cx, move |(g, items): (u64, Vec<PickerItem>)| {
            if g == p.gen.get_untracked() {
                p.items.set(items);
                p.selected.set(0);
            }
        });
        std::thread::spawn(move || {
            let syms = client.workspace_symbol(&query).unwrap_or_default();
            let items = syms
                .into_iter()
                .take(200)
                .map(|(name, uri, line, ch)| PickerItem {
                    label: name,
                    detail: rel_uri(&uri, &root),
                    uri,
                    line,
                    char: ch,
                })
                .collect();
            send((gen, items));
        });
    }

    /// Find references to the symbol under the cursor (Shift+F12).
    pub fn request_references(&self) {
        // A route name, view or translation key: the language server can't
        // follow those strings, so `e` searches the project itself.
        if self.find_laravel_key_usages() {
            return;
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

        let p = self.picker;
        p.mode.set(PickerMode::References);
        p.query.set(String::new());
        p.items.set(Vec::new());
        p.selected.set(0);
        p.open.set(true);

        let root = self.root.get();
        let send = create_ext_action(self.cx, move |items: Vec<PickerItem>| {
            p.items.set(items);
            p.selected.set(0);
        });
        std::thread::spawn(move || {
            let refs = client
                .references(&uri, line as u32, col as u32)
                .unwrap_or_default();
            let items = refs
                .into_iter()
                .map(|(u, l, c)| PickerItem {
                    label: rel_uri(&u, &root),
                    detail: format!(":{}", l + 1),
                    uri: u,
                    line: l,
                    char: c,
                })
                .collect();
            send(items);
        });
    }

    /// Route Search: every route in the project in the picker — method and URI
    /// to match on, name and action beside — and Enter opens the controller
    /// action, or the routes file line for a closure.
    pub fn open_route_search(&self) {
        let Some(data) = self.project.laravel.get_untracked() else {
            Self::notify("Not a Laravel project (no routes loaded)");
            return;
        };
        let root = self.root.get_untracked();
        let p = self.picker;
        p.mode.set(PickerMode::References);
        p.query.set(String::new());
        p.items.set(Vec::new());
        p.selected.set(0);
        p.open.set(true);
        let send = create_ext_action(self.cx, move |items: Vec<PickerItem>| {
            p.items.set(items);
            p.selected.set(0);
        });
        std::thread::spawn(move || {
            let route_files = crate::laravel::route_files(&root);
            let items = data
                .all_routes
                .iter()
                .map(|r| {
                    let (path, line) = crate::laravel::controller_location(&root, &r.action)
                        .map(|(p, l, _)| (p, l))
                        .or_else(|| crate::laravel::route_definition(&route_files, &r.uri))
                        .unwrap_or_else(|| (root.join("routes/web.php"), 0));
                    let name = if r.name.is_empty() {
                        String::new()
                    } else {
                        format!("{}  ", r.name)
                    };
                    PickerItem {
                        label: format!("{} /{}", r.methods, r.uri.trim_start_matches('/')),
                        detail: format!("{name}{}", r.action),
                        uri: path_to_uri(&path),
                        line: line as u32,
                        char: 0,
                    }
                })
                .collect();
            send(items);
        });
    }

    /// Find Unused Views: Blade views nothing references, in the picker; Enter
    /// opens one so it can be checked and deleted.
    pub fn find_unused_views(&self) {
        let Some(data) = self.project.laravel.get_untracked() else {
            Self::notify("Not a Laravel project (no views loaded)");
            return;
        };
        let root = self.root.get_untracked();
        let p = self.picker;
        p.mode.set(PickerMode::References);
        p.query.set(String::new());
        p.items.set(vec![PickerItem {
            label: "Scanning…".into(),
            detail: String::new(),
            uri: String::new(),
            line: 0,
            char: 0,
        }]);
        p.selected.set(0);
        p.open.set(true);
        let send = create_ext_action(self.cx, move |items: Vec<PickerItem>| {
            if items.is_empty() {
                p.open.set(false);
                Self::notify("Every view is referenced somewhere");
                return;
            }
            p.items.set(items);
            p.selected.set(0);
        });
        std::thread::spawn(move || {
            let items = crate::laravel::unused_views(&root, &data)
                .into_iter()
                .map(|v| PickerItem {
                    label: v.name.clone(),
                    detail: v
                        .path
                        .strip_prefix(&root)
                        .unwrap_or(&v.path)
                        .to_string_lossy()
                        .into_owned(),
                    uri: path_to_uri(&v.path),
                    line: 0,
                    char: 0,
                })
                .collect();
            send(items);
        });
    }

    /// Open `uri` and place the caret at `(line, col)`, recording the spot we
    /// jumped from in the navigation history.
    pub fn jump_to(&self, uri: &str, line: usize, col: usize) {
        self.record_nav();
        self.goto_location(uri_to_path(uri), line, col);
    }

    /// Open a file and place the caret at `(line, col)` without touching the
    /// navigation history (used by back/forward themselves).
    pub(crate) fn goto_location(&self, path: PathBuf, line: usize, col: usize) {
        self.open_path(path);
        // A freshly opened document's rope is populated on the next reactive
        // flush, so apply now and retry until it's ready for cold opens.
        if !self.apply_goto(line, col) {
            self.retry_goto(line, col, 0);
        }
    }

    fn retry_goto(&self, line: usize, col: usize, attempt: usize) {
        if attempt >= 6 {
            return;
        }
        let state = *self;
        floem::action::exec_after(std::time::Duration::from_millis(40), move |_| {
            if !state.apply_goto(line, col) {
                state.retry_goto(line, col, attempt + 1);
            }
        });
    }

    /// Place the caret at `(line, col)` in the active buffer. Returns false when
    /// the editor/document isn't ready yet (so the caller can retry).
    fn apply_goto(&self, line: usize, col: usize) -> bool {
        let Some(buf) = self.active_buffer() else {
            return false;
        };
        let Some(editor) = buf.editor.get_untracked() else {
            return false;
        };
        // The document hasn't been populated yet — try again shortly.
        if line > 0 && buf.doc.text().is_empty() {
            return false;
        }
        let offset = editor.offset_of_line_col(line, col);
        editor.cursor.set(Cursor::new(
            CursorMode::Insert(Selection::caret(offset)),
            None,
            None,
        ));
        buf.pending_goto.set(None);
        true
    }

    fn current_location(&self) -> Option<(PathBuf, usize, usize)> {
        let buf = self.active_buffer()?;
        let path = buf.file.path.clone()?;
        let editor = buf.editor.get_untracked()?;
        let (line, col) = editor.offset_to_line_col(editor.cursor.get_untracked().offset());
        Some((path, line, col))
    }

    /// Record the current location as a back-navigation target.
    fn record_nav(&self) {
        if let Some(loc) = self.current_location() {
            let dup = self.nav.back.with_untracked(|v| {
                v.last()
                    .map(|l| l.0 == loc.0 && l.1 == loc.1)
                    .unwrap_or(false)
            });
            if !dup {
                self.nav.back.update(|v| {
                    v.push(loc);
                    if v.len() > 100 {
                        v.remove(0);
                    }
                });
                self.nav.forward.update(|v| v.clear());
            }
        }
    }

    /// Navigate to the previous location in the history.
    pub fn nav_back(&self) {
        let Some(target) = self.nav.back.try_update(|v| v.pop()).flatten() else {
            return;
        };
        if let Some(cur) = self.current_location() {
            self.nav.forward.update(|v| v.push(cur));
        }
        self.goto_location(target.0, target.1, target.2);
    }

    /// Navigate to the next location in the history.
    pub fn nav_forward(&self) {
        let Some(target) = self.nav.forward.try_update(|v| v.pop()).flatten() else {
            return;
        };
        if let Some(cur) = self.current_location() {
            self.nav.back.update(|v| v.push(cur));
        }
        self.goto_location(target.0, target.1, target.2);
    }
}

/// Getting around: back and forward, recent files, pinned tabs.
///
/// `Copy`, like `AppState`: signals and handles only.
#[derive(Clone, Copy)]
pub struct Navigation {
    /// Navigation history (locations jumped from / to).
    pub back: floem::reactive::RwSignal<Vec<(std::path::PathBuf, usize, usize)>>,
    pub forward: floem::reactive::RwSignal<Vec<(std::path::PathBuf, usize, usize)>>,
    /// Most-recently-used files (newest first) and the ⌘E switcher state.
    pub recent_files: floem::reactive::RwSignal<Vec<std::path::PathBuf>>,
    pub pinned_tabs: floem::reactive::RwSignal<std::collections::HashSet<u64>>,
}
