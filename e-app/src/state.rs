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

use floem::reactive::{RwSignal, Scope, SignalGet, SignalUpdate, SignalWith};
use floem::views::editor::text::Document;
use floem::views::editor::text_document::TextDocument;
use lsp_types::{Diagnostic, PublishDiagnosticsParams};

use e_core::buffer::{self, FileInfo};
use e_core::language::Language;
use e_core::syntax::highlight_lines;
use e_lsp::{path_to_uri, LspClient};

use crate::styling::Highlights;

/// One open file/tab.
#[derive(Clone)]
pub struct Buffer {
    pub id: u64,
    pub file: FileInfo,
    pub doc: Rc<TextDocument>,
    pub dirty: RwSignal<bool>,
    pub highlights: Highlights,
    /// `file://` URI, when backed by a path (used for LSP).
    pub uri: Option<String>,
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
}

impl AppState {
    pub fn new(cx: Scope, root: PathBuf) -> Self {
        let (tx, rx) = channel();
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
        }
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
            let app = *self;
            let uri = uri.clone();
            doc.clone().add_on_update(move |_| {
                dirty.set(true);
                let text = doc.text().to_string();
                *highlights.borrow_mut() = highlight_lines(language, &text);
                doc.cache_rev().update(|r| *r += 1);

                if let (Some(uri), Some(client)) = (uri.as_ref(), app.lsp.get()) {
                    if lsp_language_id(language).is_some() {
                        let v = version.get() + 1;
                        version.set(v);
                        client.did_change_full(uri, v, &text);
                    }
                }
            });
        }

        let buf = Buffer {
            id,
            file,
            doc,
            dirty,
            highlights,
            uri,
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

    /// Save the active buffer to disk.
    pub fn save_active(&self) {
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
            }
            Err(e) => eprintln!("e: save failed: {e:#}"),
        }
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
}
