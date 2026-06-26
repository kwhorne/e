//! Shared, reactive application state.
//!
//! `AppState` is `Copy` (every field is a Floem signal or `Scope`), so it can
//! be handed to as many view closures as needed without cloning ceremony.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use std::sync::mpsc::{channel, Receiver, Sender};

use floem::ext_event::create_ext_action;
use floem::kurbo::Point;
use floem::reactive::{RwSignal, Scope, SignalGet, SignalUpdate, SignalWith};
use floem::views::editor::core::cursor::{Cursor, CursorMode};
use floem::views::editor::core::editor::EditType;
use floem::views::editor::core::selection::Selection;
use floem::views::editor::text::Document;
use floem::views::editor::text_document::TextDocument;
use floem::views::editor::Editor;
use lsp_types::{Diagnostic, PublishDiagnosticsParams};

use e_core::buffer::{self, FileInfo};
use e_core::git;
use e_core::language::Language;
use e_core::syntax::highlight_lines;
use e_lsp::{path_to_uri, uri_to_path, LspClient};
use e_term::Terminal;

use crate::completion::{Completion, HoverState};
use crate::laravel::{self, LaravelData};
use crate::outline::OutlineItem;
use crate::picker::{Picker, PickerItem, PickerMode};
use crate::styling::{build_diag_lines, DiagLines, GitMarks, Highlights};

/// One open file/tab.
#[derive(Clone)]
pub struct Buffer {
    pub id: u64,
    pub file: FileInfo,
    pub doc: Rc<TextDocument>,
    pub dirty: RwSignal<bool>,
    pub highlights: Highlights,
    /// Per-line diagnostic spans (for inline squiggles).
    pub diag_lines: DiagLines,
    /// Per-line git change markers.
    pub git_marks: GitMarks,
    /// `file://` URI, when backed by a path (used for LSP).
    pub uri: Option<String>,
    /// The live editor, set once its view is built.
    pub editor: RwSignal<Option<Editor>>,
    /// The editor's top-left position in the window (for popups).
    pub win_origin: RwSignal<Point>,
    /// A `(line, col)` to move the caret to once the editor exists.
    pub pending_goto: RwSignal<Option<(usize, usize)>>,
}

/// LSP language id, or `None` if `e` has no server for this language.
fn lsp_language_id(language: Language) -> Option<&'static str> {
    match language {
        Language::Php => Some("php"),
        _ => None,
    }
}

/// Global editor state.
#[derive(Clone, Copy)]
pub struct AppState {
    /// Scope used to create per-document signals.
    pub cx: Scope,
    /// Workspace root shown in the file tree.
    pub root: RwSignal<PathBuf>,
    /// All open buffers, in tab order.
    pub buffers: RwSignal<Vec<Buffer>>,
    /// Currently focused buffer id.
    pub active: RwSignal<Option<u64>>,
    /// Monotonic id source.
    next_id: RwSignal<u64>,
    /// Is the command palette open?
    pub palette_open: RwSignal<bool>,
    /// The PHP language server, started lazily on first PHP file.
    pub lsp: RwSignal<Option<Arc<LspClient>>>,
    /// Diagnostics keyed by `file://` URI.
    pub diagnostics: RwSignal<HashMap<String, Vec<Diagnostic>>>,
    /// Channel the LSP reader thread pushes diagnostics into.
    diag_tx: RwSignal<Sender<PublishDiagnosticsParams>>,
    /// Receiver, taken once by the UI to build a reactive signal.
    pub diag_rx: RwSignal<Option<Receiver<PublishDiagnosticsParams>>>,
    /// Completion popup state.
    pub completion: Completion,
    /// Hover popup state.
    pub hover: HoverState,
    /// Laravel project data (routes/views/config/env), if applicable.
    pub laravel: RwSignal<Option<Rc<LaravelData>>>,
    /// References / symbol-search picker.
    pub picker: Picker,
    /// Integrated terminal session (lazily spawned).
    pub terminal: RwSignal<Option<Rc<RefCell<Terminal>>>>,
    pub terminal_open: RwSignal<bool>,
    /// Bumped whenever the terminal produces output, to trigger a repaint.
    pub term_tick: RwSignal<u64>,
    term_tx: RwSignal<Sender<()>>,
    pub term_rx: RwSignal<Option<Receiver<()>>>,
    /// Document outline of the active buffer.
    pub outline: RwSignal<Vec<OutlineItem>>,
}

impl AppState {
    pub fn new(cx: Scope, root: PathBuf) -> Self {
        let (tx, rx) = channel();
        let (term_tx, term_rx) = channel();
        Self {
            cx,
            root: RwSignal::new(root),
            buffers: RwSignal::new(Vec::new()),
            active: RwSignal::new(None),
            next_id: RwSignal::new(1),
            palette_open: RwSignal::new(false),
            lsp: RwSignal::new(None),
            diagnostics: RwSignal::new(HashMap::new()),
            diag_tx: RwSignal::new(tx),
            diag_rx: RwSignal::new(Some(rx)),
            completion: Completion::new(),
            hover: HoverState::new(),
            laravel: RwSignal::new(None),
            picker: Picker::new(),
            terminal: RwSignal::new(None),
            terminal_open: RwSignal::new(false),
            term_tick: RwSignal::new(0),
            term_tx: RwSignal::new(term_tx),
            term_rx: RwSignal::new(Some(term_rx)),
            outline: RwSignal::new(Vec::new()),
        }
    }

    /// Load the document outline for the active buffer (LSP documentSymbol).
    pub fn request_outline(&self) {
        let outline = self.outline;
        let Some(buf) = self.active_buffer() else {
            outline.set(Vec::new());
            return;
        };
        let (Some(client), Some(uri)) = (self.lsp.get(), buf.uri.clone()) else {
            outline.set(Vec::new());
            return;
        };
        if lsp_language_id(buf.file.language).is_none() {
            outline.set(Vec::new());
            return;
        }
        let send = create_ext_action(self.cx, move |items: Vec<OutlineItem>| outline.set(items));
        std::thread::spawn(move || {
            let syms = client.document_symbols(&uri).unwrap_or_default();
            let items = syms
                .into_iter()
                .map(|(name, kind, line, ch, depth)| OutlineItem {
                    name,
                    kind,
                    line,
                    char: ch,
                    depth,
                })
                .collect();
            send(items);
        });
    }

    // ---- Integrated terminal -------------------------------------------

    /// Toggle the terminal, spawning a shell on first use.
    pub fn toggle_terminal(&self) {
        if self.terminal.get_untracked().is_none() {
            let tx = self.term_tx.get();
            let on_update = Box::new(move || {
                let _ = tx.send(());
            });
            let root = self.root.get();
            match Terminal::spawn(&e_term::default_shell(), &root, 30, 110, on_update) {
                Ok(t) => {
                    self.terminal.set(Some(Rc::new(RefCell::new(t))));
                    self.terminal_open.set(true);
                }
                Err(e) => eprintln!("e: terminal failed: {e:#}"),
            }
        } else {
            let open = self.terminal_open.get_untracked();
            self.terminal_open.set(!open);
        }
    }

    pub fn terminal_input(&self, bytes: &[u8]) {
        if let Some(t) = self.terminal.get_untracked() {
            t.borrow_mut().write(bytes);
        }
    }

    pub fn terminal_snapshot(&self) -> Vec<String> {
        self.terminal
            .get_untracked()
            .map(|t| t.borrow().snapshot())
            .unwrap_or_default()
    }

    pub fn buffer_by_id(&self, id: u64) -> Option<Buffer> {
        self.buffers.with(|bs| bs.iter().find(|b| b.id == id).cloned())
    }

    /// If the workspace is a Laravel project, scrape its data in the background.
    pub fn load_laravel(&self) {
        let root = self.root.get();
        if !laravel::is_laravel(&root) {
            return;
        }
        let laravel_sig = self.laravel;
        let send = create_ext_action(self.cx, move |data: LaravelData| {
            eprintln!("e: loaded Laravel project data");
            laravel_sig.set(Some(Rc::new(data)));
        });
        std::thread::spawn(move || {
            let data = laravel::load(&root);
            send(data);
        });
    }

    /// Offer Laravel completions if the cursor is inside a helper string.
    /// Returns true when the context was handled (so we skip the LSP).
    fn try_laravel_completion(&self, buffer_id: u64) -> bool {
        let Some(data) = self.laravel.get() else {
            return false;
        };
        let Some(buf) = self.buffer_by_id(buffer_id) else {
            return false;
        };
        let Some(editor) = buf.editor.get_untracked() else {
            return false;
        };
        let cursor = editor.cursor.get_untracked();
        let offset = cursor.offset();
        let text = buf.doc.text().to_string();
        let upto = offset.min(text.len());
        let line_start = text[..upto].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let line_before = &text[line_start..upto];

        let Some((helper, prefix)) = laravel::detect_context(line_before) else {
            return false;
        };

        let items = laravel::completions(&data, helper, &prefix);
        let start = offset - prefix.len();

        let (_, below) = editor.points_of_offset(start, cursor.affinity);
        let vp = editor.viewport.get_untracked();
        let win = buf.win_origin.get_untracked();

        let comp = self.completion;
        comp.anchor.set(Point::new(win.x + below.x - vp.x0, win.y + below.y - vp.y0));
        comp.buffer_id.set(Some(buffer_id));
        comp.start_offset.set(start);
        if items.is_empty() {
            comp.open.set(false);
        } else {
            comp.items.set(items);
            comp.selected.set(0);
            comp.open.set(true);
        }
        true
    }

    /// Start (or reuse) the PHP language server for this workspace.
    fn ensure_php_lsp(&self) -> Option<Arc<LspClient>> {
        if let Some(client) = self.lsp.get() {
            return Some(client);
        }
        let tx = self.diag_tx.get();
        let handler: e_lsp::DiagnosticsHandler = Box::new(move |p| {
            let _ = tx.send(p);
        });
        let root = self.root.get();
        match LspClient::start("intelephense", &["--stdio"], &root, handler) {
            Ok(client) => {
                eprintln!("e: started intelephense for {}", root.display());
                self.lsp.set(Some(client.clone()));
                Some(client)
            }
            Err(e) => {
                eprintln!("e: could not start intelephense ({e:#}). Install with `npm i -g intelephense`.");
                None
            }
        }
    }

    /// Open a file by path. If it's already open, just focus it.
    pub fn open_path(&self, path: PathBuf) {
        let canon = path.canonicalize().unwrap_or(path);

        // Already open? Focus the existing tab.
        let existing = self.buffers.with(|bs| {
            bs.iter()
                .find(|b| b.file.path.as_deref() == Some(canon.as_path()))
                .map(|b| b.id)
        });
        if let Some(id) = existing {
            self.active.set(Some(id));
            return;
        }

        let content = match buffer::read_to_string(&canon) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("e: open failed: {e:#}");
                return;
            }
        };

        let id = self.next_id.get();
        self.next_id.set(id + 1);

        let file = FileInfo::for_path(canon.clone());
        let language = file.language;
        let uri = file.path.as_ref().map(|p| path_to_uri(p));

        let highlights: Highlights = Rc::new(RefCell::new(highlight_lines(language, &content)));

        // Git change markers vs HEAD.
        let head_text = file.path.as_ref().and_then(|p| git::head_text(p));
        let line_count = content.split_inclusive('\n').count().max(1);
        let git_marks: GitMarks = Rc::new(RefCell::new(match &head_text {
            Some(h) => git::marks(h, &content, line_count),
            None => Vec::new(),
        }));

        let doc = Rc::new(TextDocument::new(self.cx, content.clone()));
        let dirty = RwSignal::new(false);
        let version = RwSignal::new(1i64);

        // Hand the document to the language server, if we have one.
        if let (Some(lang_id), Some(uri)) = (lsp_language_id(language), uri.as_ref()) {
            if let Some(client) = self.ensure_php_lsp() {
                client.did_open(uri, lang_id, 1, &content);
            }
        }

        // On every edit: mark dirty, re-highlight, invalidate the layout cache,
        // and notify the language server.
        {
            let doc = doc.clone();
            let highlights = highlights.clone();
            let git_marks = git_marks.clone();
            let head_text = head_text.clone();
            let app = *self;
            let uri = uri.clone();
            doc.clone().add_on_update(move |_| {
                dirty.set(true);
                let text = doc.text().to_string();
                *highlights.borrow_mut() = highlight_lines(language, &text);
                if let Some(head) = &head_text {
                    let lc = text.split_inclusive('\n').count().max(1);
                    *git_marks.borrow_mut() = git::marks(head, &text, lc);
                }
                doc.cache_rev().update(|r| *r += 1);

                if let (Some(uri), Some(client)) = (uri.as_ref(), app.lsp.get()) {
                    if lsp_language_id(language).is_some() {
                        let v = version.get() + 1;
                        version.set(v);
                        client.did_change_full(uri, v, &text);
                    }
                }
                // Laravel helper completion works in both PHP and Blade files.
                if matches!(language, Language::Php | Language::Blade) {
                    app.autocomplete_after_edit(id);
                }
            });
        }

        let buf = Buffer {
            id,
            file,
            doc,
            dirty,
            highlights,
            diag_lines: Rc::new(RefCell::new(Vec::new())),
            git_marks,
            uri,
            editor: RwSignal::new(None),
            win_origin: RwSignal::new(Point::ZERO),
            pending_goto: RwSignal::new(None),
        };
        self.buffers.update(|bs| bs.push(buf));
        self.active.set(Some(id));
    }

    /// Close a tab; focus a neighbour if it was active.
    pub fn close(&self, id: u64) {
        let mut focus_next = None;
        let mut closed_uri = None;
        self.buffers.update(|bs| {
            if let Some(pos) = bs.iter().position(|b| b.id == id) {
                closed_uri = bs[pos].uri.clone();
                bs.remove(pos);
                if !bs.is_empty() {
                    let n = pos.min(bs.len() - 1);
                    focus_next = Some(bs[n].id);
                }
            }
        });
        if self.active.get() == Some(id) {
            self.active.set(focus_next);
        }
        if let (Some(uri), Some(client)) = (closed_uri, self.lsp.get()) {
            client.did_close(&uri);
        }
    }

    pub fn active_buffer(&self) -> Option<Buffer> {
        let active = self.active.get()?;
        self.buffers
            .with(|bs| bs.iter().find(|b| b.id == active).cloned())
    }

    /// Format the active buffer in place via the language server (PHP only).
    fn format_active(&self) {
        let Some(buf) = self.active_buffer() else {
            return;
        };
        if lsp_language_id(buf.file.language).is_none() {
            return;
        }
        let (Some(client), Some(uri), Some(editor)) =
            (self.lsp.get(), buf.uri.clone(), buf.editor.get_untracked())
        else {
            return;
        };
        let edits = match client.formatting(&uri, 4, true) {
            Ok(e) if !e.is_empty() => e,
            _ => return,
        };
        // Resolve to offsets against the current text, then apply bottom-up so
        // earlier offsets stay valid.
        let mut offs: Vec<(usize, usize, String)> = edits
            .into_iter()
            .map(|e| {
                let s = editor
                    .offset_of_line_col(e.range.start.line as usize, e.range.start.character as usize);
                let en = editor
                    .offset_of_line_col(e.range.end.line as usize, e.range.end.character as usize);
                (s, en, e.new_text)
            })
            .collect();
        offs.sort_by(|a, b| b.0.cmp(&a.0));
        for (s, en, text) in offs {
            buf.doc
                .edit_single(Selection::region(s, en), &text, EditType::InsertChars);
        }
    }

    /// Save the active buffer to disk (formatting PHP first).
    pub fn save_active(&self) {
        self.format_active();
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let Some(path) = buf.file.path.as_ref() else {
            eprintln!("e: save-as is not implemented yet");
            return;
        };
        let text = buf.doc.text().to_string();
        match buffer::write(path, &text) {
            Ok(()) => {
                buf.dirty.set(false);
                eprintln!("e: saved {}", path.display());
                if let (Some(uri), Some(client)) = (buf.uri.as_ref(), self.lsp.get()) {
                    client.did_save(uri, &text);
                }
                self.request_outline();
            }
            Err(e) => eprintln!("e: save failed: {e:#}"),
        }
    }

    /// Rebuild a buffer's inline diagnostic spans and repaint it.
    pub fn apply_diagnostics_to_buffer(&self, uri: &str, diags: &[Diagnostic]) {
        let Some(buf) = self
            .buffers
            .with(|bs| bs.iter().find(|b| b.uri.as_deref() == Some(uri)).cloned())
        else {
            return;
        };
        let text = buf.doc.text().to_string();
        *buf.diag_lines.borrow_mut() = build_diag_lines(diags, &text);
        buf.doc.cache_rev().update(|r| *r += 1);
    }

    /// `(errors, warnings)` for the active buffer.
    pub fn active_diagnostic_counts(&self) -> (usize, usize) {
        let Some(buf) = self.active_buffer() else {
            return (0, 0);
        };
        let Some(uri) = buf.uri.as_ref() else {
            return (0, 0);
        };
        self.diagnostics.with(|map| {
            let Some(diags) = map.get(uri) else {
                return (0, 0);
            };
            let mut errors = 0;
            let mut warnings = 0;
            for d in diags {
                match d.severity {
                    Some(lsp_types::DiagnosticSeverity::ERROR) => errors += 1,
                    Some(lsp_types::DiagnosticSeverity::WARNING) => warnings += 1,
                    _ => {}
                }
            }
            (errors, warnings)
        })
    }

    /// Diagnostics for the active buffer, sorted by line.
    pub fn active_diagnostics(&self) -> Vec<Diagnostic> {
        let Some(buf) = self.active_buffer() else {
            return Vec::new();
        };
        let Some(uri) = buf.uri.as_ref() else {
            return Vec::new();
        };
        let mut diags = self
            .diagnostics
            .with(|map| map.get(uri).cloned().unwrap_or_default());
        diags.sort_by_key(|d| d.range.start.line);
        diags
    }

    // ---- Completion & hover --------------------------------------------

    /// After an edit in a PHP buffer, decide whether to (re)trigger completion.
    pub fn autocomplete_after_edit(&self, buffer_id: u64) {
        // Laravel helper strings take priority over generic PHP completion.
        if self.try_laravel_completion(buffer_id) {
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

    pub fn request_completion(&self, buffer_id: u64) {
        if self.try_laravel_completion(buffer_id) {
            return;
        }
        let Some(buf) = self.buffer_by_id(buffer_id) else {
            return;
        };
        let (Some(client), Some(uri), Some(editor)) =
            (self.lsp.get(), buf.uri.clone(), buf.editor.get_untracked())
        else {
            return;
        };

        let cursor = editor.cursor.get_untracked();
        let offset = cursor.offset();
        let (line, col) = editor.offset_to_line_col(offset);

        let text = buf.doc.text().to_string();
        let start = word_start(&text, offset);

        // Anchor the popup at the start of the replaced word.
        let (_, below) = editor.points_of_offset(start, cursor.affinity);
        let vp = editor.viewport.get_untracked();
        let win = buf.win_origin.get_untracked();
        let anchor = Point::new(win.x + below.x - vp.x0, win.y + below.y - vp.y0);

        let comp = self.completion;
        comp.buffer_id.set(Some(buffer_id));
        comp.start_offset.set(start);
        comp.anchor.set(anchor);

        let send = create_ext_action(self.cx, move |items: Vec<lsp_types::CompletionItem>| {
            if items.is_empty() {
                comp.open.set(false);
            } else {
                comp.items.set(items);
                comp.selected.set(0);
                comp.open.set(true);
            }
        });
        std::thread::spawn(move || {
            let items = client.completion(&uri, line as u32, col as u32).unwrap_or_default();
            send(items);
        });
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
        let insert = item
            .insert_text
            .clone()
            .unwrap_or_else(|| item.label.clone());

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
        buf.doc
            .edit_single(Selection::region(start, end), &insert, EditType::InsertChars);
        comp.open.set(false);
        true
    }

    pub fn request_hover(&self) {
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let (Some(client), Some(uri), Some(editor)) =
            (self.lsp.get(), buf.uri.clone(), buf.editor.get_untracked())
        else {
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

    /// Jump to the definition of the symbol under the cursor (LSP).
    pub fn goto_definition(&self) {
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let (Some(client), Some(uri), Some(editor)) =
            (self.lsp.get(), buf.uri.clone(), buf.editor.get_untracked())
        else {
            return;
        };
        let (line, col) = editor.offset_to_line_col(editor.cursor.get_untracked().offset());
        let app = *self;
        let send = create_ext_action(self.cx, move |loc: Option<(String, u32, u32)>| match loc {
            Some((u, l, c)) => app.jump_to(&u, l as usize, c as usize),
            None => eprintln!("e: no definition found"),
        });
        std::thread::spawn(move || {
            let loc = client.definition(&uri, line as u32, col as u32).ok().flatten();
            send(loc);
        });
    }

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

    /// Run a workspace/symbol query (called reactively from the picker).
    pub fn request_symbols(&self, query: String) {
        let p = self.picker;
        let Some(client) = self.lsp.get() else {
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
        let (Some(client), Some(uri), Some(editor)) =
            (self.lsp.get(), buf.uri.clone(), buf.editor.get_untracked())
        else {
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
            let refs = client.references(&uri, line as u32, col as u32).unwrap_or_default();
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

    /// Open `uri` and place the caret at `(line, col)`.
    pub fn jump_to(&self, uri: &str, line: usize, col: usize) {
        self.open_path(uri_to_path(uri));
        let Some(buf) = self.active_buffer() else {
            return;
        };
        if let Some(editor) = buf.editor.get_untracked() {
            let offset = editor.offset_of_line_col(line, col);
            editor
                .cursor
                .set(Cursor::new(CursorMode::Insert(Selection::caret(offset)), None, None));
        } else {
            // The editor view isn't built yet; apply once it is.
            buf.pending_goto.set(Some((line, col)));
        }
    }
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// Display a `file://` URI relative to the workspace root.
fn rel_uri(uri: &str, root: &std::path::Path) -> String {
    let path = uri_to_path(uri);
    path.strip_prefix(root)
        .unwrap_or(&path)
        .to_string_lossy()
        .into_owned()
}

/// Byte offset where the identifier ending at `offset` begins.
fn word_start(text: &str, offset: usize) -> usize {
    let offset = offset.min(text.len());
    let mut start = offset;
    for (i, c) in text[..offset].char_indices().rev() {
        if is_word_char(c) {
            start = i;
        } else {
            break;
        }
    }
    start
}
