//! Shared, reactive application state.
//!
//! `AppState` is `Copy` (every field is a Floem signal or `Scope`), so it can
//! be handed to as many view closures as needed without cloning ceremony.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use floem::reactive::{RwSignal, Scope, SignalGet, SignalUpdate, SignalWith};
use floem::views::editor::text::Document;
use floem::views::editor::text_document::TextDocument;

use e_core::buffer::{self, FileInfo};
use e_core::syntax::highlight_lines;

use crate::styling::Highlights;

/// One open file/tab.
#[derive(Clone)]
pub struct Buffer {
    pub id: u64,
    pub file: FileInfo,
    pub doc: Rc<TextDocument>,
    pub dirty: RwSignal<bool>,
    pub highlights: Highlights,
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
}

impl AppState {
    pub fn new(cx: Scope, root: PathBuf) -> Self {
        Self {
            cx,
            root: RwSignal::new(root),
            buffers: RwSignal::new(Vec::new()),
            active: RwSignal::new(None),
            next_id: RwSignal::new(1),
            palette_open: RwSignal::new(false),
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

        let file = FileInfo::for_path(canon);
        let language = file.language;

        let highlights: Highlights = Rc::new(RefCell::new(highlight_lines(language, &content)));
        let doc = Rc::new(TextDocument::new(self.cx, content));
        let dirty = RwSignal::new(false);

        // On every edit: mark dirty, re-highlight, and invalidate the editor's
        // layout cache so the new colours are painted.
        {
            let doc = doc.clone();
            let highlights = highlights.clone();
            doc.clone().add_on_update(move |_| {
                dirty.set(true);
                let text = doc.text().to_string();
                *highlights.borrow_mut() = highlight_lines(language, &text);
                doc.cache_rev().update(|r| *r += 1);
            });
        }

        let buf = Buffer {
            id,
            file,
            doc,
            dirty,
            highlights,
        };
        self.buffers.update(|bs| bs.push(buf));
        self.active.set(Some(id));
    }

    /// Close a tab; focus a neighbour if it was active.
    pub fn close(&self, id: u64) {
        let mut focus_next = None;
        self.buffers.update(|bs| {
            if let Some(pos) = bs.iter().position(|b| b.id == id) {
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
            }
            Err(e) => eprintln!("e: save failed: {e:#}"),
        }
    }
}
