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

use e_lsp::uri_to_path;

use crate::picker::{PickerItem, PickerMode};
use crate::state::{grep_workspace, rel_uri, replace_in_dir, AppState};

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

    /// Replace every occurrence of the search query across the workspace.
    pub fn replace_in_workspace(&self) {
        let query = self.picker.query.get_untracked();
        let replace = self.picker.replace.get_untracked();
        if query.is_empty() {
            return;
        }
        let root = self.root.get_untracked();
        let files = replace_in_dir(&root, &query, &replace);
        eprintln!("e: replaced in {files} file(s)");
        self.fs_rev.update(|r| *r += 1);
        // Reload affected open buffers and refresh the result list.
        self.check_external_changes();
        self.request_search(query);
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
    pub fn request_search(&self, query: String) {
        let p = self.picker;
        if query.trim().len() < 2 {
            p.items.set(Vec::new());
            return;
        }
        let gen = p.gen.get_untracked() + 1;
        p.gen.set(gen);
        let roots = self.roots.get();
        let send = create_ext_action(self.cx, move |(g, items): (u64, Vec<PickerItem>)| {
            if g == p.gen.get_untracked() {
                p.items.set(items);
                p.selected.set(0);
            }
        });
        std::thread::spawn(move || {
            let mut items = Vec::new();
            for root in &roots {
                items.extend(grep_workspace(root, &query, 300));
                if items.len() >= 300 {
                    items.truncate(300);
                    break;
                }
            }
            send((gen, items));
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

    /// Open `uri` and place the caret at `(line, col)`, recording the spot we
    /// jumped from in the navigation history.
    pub fn jump_to(&self, uri: &str, line: usize, col: usize) {
        self.record_nav();
        self.goto_location(uri_to_path(uri), line, col);
    }

    /// Open a file and place the caret at `(line, col)` without touching the
    /// navigation history (used by back/forward themselves).
    fn goto_location(&self, path: PathBuf, line: usize, col: usize) {
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
            let dup = self.nav_back_stack.with_untracked(|v| {
                v.last()
                    .map(|l| l.0 == loc.0 && l.1 == loc.1)
                    .unwrap_or(false)
            });
            if !dup {
                self.nav_back_stack.update(|v| {
                    v.push(loc);
                    if v.len() > 100 {
                        v.remove(0);
                    }
                });
                self.nav_fwd_stack.update(|v| v.clear());
            }
        }
    }

    /// Navigate to the previous location in the history.
    pub fn nav_back(&self) {
        let Some(target) = self.nav_back_stack.try_update(|v| v.pop()).flatten() else {
            return;
        };
        if let Some(cur) = self.current_location() {
            self.nav_fwd_stack.update(|v| v.push(cur));
        }
        self.goto_location(target.0, target.1, target.2);
    }

    /// Navigate to the next location in the history.
    pub fn nav_forward(&self) {
        let Some(target) = self.nav_fwd_stack.try_update(|v| v.pop()).flatten() else {
            return;
        };
        if let Some(cur) = self.current_location() {
            self.nav_back_stack.update(|v| v.push(cur));
        }
        self.goto_location(target.0, target.1, target.2);
    }
}
