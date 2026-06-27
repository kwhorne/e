//! Shared, reactive application state.
//!
//! `AppState` is `Copy` (every field is a Floem signal or `Scope`), so it can
//! be handed to as many view closures as needed without cloning ceremony.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use std::sync::mpsc::{channel, Receiver, Sender};

use floem::ext_event::create_ext_action;
use floem::kurbo::Point;
use floem::reactive::{RwSignal, Scope, SignalGet, SignalUpdate, SignalWith};
use floem::views::editor::core::cursor::{Cursor, CursorMode};
use floem::views::editor::core::editor::EditType;
use floem::views::editor::core::selection::{SelRegion, Selection};
use floem::views::editor::text::Document;
use floem::views::editor::text_document::TextDocument;
use floem::views::editor::Editor;
use lsp_types::{Diagnostic, PublishDiagnosticsParams};

use e_core::buffer::{self, FileInfo};
use e_core::git;
use e_core::language::Language;
use e_core::syntax::highlight_lines;
use e_lsp::{path_to_uri, uri_to_path, LspClient, SignatureInfo};
use e_term::Terminal;

use crate::cmd_palette::CmdPalette;
use crate::completion::{Completion, HoverState, SignatureState};
use crate::config::{self, AgentConfig, Settings};
use crate::file_ops::{copy_recursive, duplicate_name, FileOp, FileOpKind};
use crate::find::FindState;
use crate::laravel::{self, LaravelData};
use crate::outline::OutlineItem;
use crate::picker::{Picker, PickerItem, PickerMode};
use crate::rename::RenameState;
use crate::session::{self, SessionData};
use crate::snippets;
use crate::styling::{
    build_diag_lines, BracketMarks, DiagLines, FindMarks, FindSpan, GitMarks, Highlights,
};

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
    /// Per-line find-match spans.
    pub find_marks: FindMarks,
    /// Matching-bracket highlight spans.
    pub bracket_marks: BracketMarks,
    /// `file://` URI, when backed by a path (used for LSP).
    pub uri: Option<String>,
    /// The live editor, set once its view is built.
    pub editor: RwSignal<Option<Editor>>,
    /// The editor's top-left position in the window (for popups).
    pub win_origin: RwSignal<Point>,
    /// A `(line, col)` to move the caret to once the editor exists.
    pub pending_goto: RwSignal<Option<(usize, usize)>>,
    /// Last-seen modification time of the file on disk (for change detection).
    pub disk_mtime: RwSignal<Option<std::time::SystemTime>>,
    /// Set when the file changed on disk while the buffer had unsaved edits.
    pub disk_changed: RwSignal<bool>,
}

/// One terminal session (a running shell).
#[derive(Clone)]
pub struct TermSession {
    pub id: u64,
    pub term: Rc<RefCell<Terminal>>,
    /// Custom name (empty = default "zsh N").
    pub name: RwSignal<String>,
}

/// A language server we know how to launch.
struct ServerSpec {
    id: &'static str,
    program: &'static str,
    args: &'static [&'static str],
    language_id: &'static str,
}

/// The language server for a given language, if `e` knows one.
fn server_spec(language: Language) -> Option<ServerSpec> {
    let spec = |id, program, args, language_id| {
        Some(ServerSpec {
            id,
            program,
            args,
            language_id,
        })
    };
    match language {
        Language::Php => spec("intelephense", "intelephense", &["--stdio"], "php"),
        Language::Rust => spec("rust-analyzer", "rust-analyzer", &[], "rust"),
        Language::C => spec("clangd", "clangd", &[], "c"),
        Language::Cpp => spec("clangd", "clangd", &[], "cpp"),
        Language::TypeScript => spec(
            "tsserver",
            "typescript-language-server",
            &["--stdio"],
            "typescript",
        ),
        Language::JavaScript => spec(
            "tsserver",
            "typescript-language-server",
            &["--stdio"],
            "javascript",
        ),
        Language::Go => spec("gopls", "gopls", &[], "go"),
        Language::Python => spec("pyright", "pyright-langserver", &["--stdio"], "python"),
        _ => None,
    }
}

/// LSP `languageId` for a language, or `None` if unsupported.
fn lsp_language_id(language: Language) -> Option<&'static str> {
    server_spec(language).map(|s| s.language_id)
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
    /// Pane 0's active buffer id.
    pub active: RwSignal<Option<u64>>,
    /// Pane 1's active buffer id (split view).
    pub active2: RwSignal<Option<u64>>,
    /// Is the editor split into two panes?
    pub split: RwSignal<bool>,
    /// Which pane has focus (0 or 1).
    pub focused: RwSignal<u8>,
    /// Monotonic id source.
    next_id: RwSignal<u64>,
    /// Is the command palette open?
    pub palette_open: RwSignal<bool>,
    /// The PHP language server, started lazily on first PHP file.
    /// Running language servers, keyed by server id.
    pub lsp_clients: RwSignal<HashMap<String, Arc<LspClient>>>,
    /// Server ids that failed to start (don't retry).
    lsp_failed: RwSignal<HashSet<String>>,
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
    /// Signature-help popup state.
    pub signature: SignatureState,
    /// Laravel project data (routes/views/config/env), if applicable.
    pub laravel: RwSignal<Option<Rc<LaravelData>>>,
    /// References / symbol-search picker.
    pub picker: Picker,
    /// Integrated terminal session (lazily spawned).
    /// All open terminal sessions, in tab order.
    pub terminals: RwSignal<Vec<TermSession>>,
    /// Pane 0's active terminal session id.
    pub active_terminal: RwSignal<Option<u64>>,
    /// Pane 1's active terminal (split view).
    pub active_terminal2: RwSignal<Option<u64>>,
    /// Is the terminal split into two panes?
    pub term_split: RwSignal<bool>,
    /// Which terminal pane has focus (0 or 1).
    pub term_focus_pane: RwSignal<u8>,
    next_term_id: RwSignal<u64>,
    /// Terminal-rename prompt: the session id being renamed, and its input.
    pub term_rename_id: RwSignal<Option<u64>>,
    pub term_rename_input: RwSignal<String>,
    pub terminal_open: RwSignal<bool>,
    /// Whether the terminal panel currently has keyboard focus.
    pub terminal_focused: RwSignal<bool>,
    /// Bumped whenever the terminal produces output, to trigger a repaint.
    pub term_tick: RwSignal<u64>,
    term_tx: RwSignal<Sender<()>>,
    pub term_rx: RwSignal<Option<Receiver<()>>>,
    /// Document outline of the active buffer.
    pub outline: RwSignal<Vec<OutlineItem>>,
    /// Find-in-file state.
    pub find: FindState,
    /// Local rename state.
    pub rename: RenameState,
    /// Timestamp (ms since epoch) of the last edit, for idle auto-save.
    pub last_edit: RwSignal<u128>,
    /// Markdown reading-mode preview toggle.
    pub md_preview: RwSignal<bool>,
    /// Command palette (⌘⇧P).
    pub cmd: CmdPalette,
    /// Git diff reading-mode toggle.
    pub diff_open: RwSignal<bool>,
    /// User settings loaded from config.json.
    pub settings: Settings,
    /// Whether the left sidebar (file explorer) is visible.
    pub sidebar_open: RwSignal<bool>,
    /// File-operation name prompt (new/rename/duplicate).
    pub file_op: FileOp,
    /// Bumped after any filesystem change to refresh the file tree.
    pub fs_rev: RwSignal<u64>,
    /// Whether the About dialog is open.
    pub about_open: RwSignal<bool>,

    // ---- Agent panel (right side) --------------------------------------
    /// Whether the agent panel is visible (toggled with ⌘L).
    pub agent_open: RwSignal<bool>,
    /// The configured agents (from config.json or built-in defaults).
    pub agents: RwSignal<Vec<AgentConfig>>,
    /// The currently selected agent id.
    pub agent_current: RwSignal<String>,
    /// The running agent PTY, if started.
    pub agent_term: RwSignal<Option<Rc<RefCell<Terminal>>>>,
    /// Whether the agent panel currently has keyboard focus.
    pub agent_focused: RwSignal<bool>,
    /// Pulsed on open so the panel grabs focus without re-grabbing on close.
    pub agent_focus_pulse: RwSignal<u64>,

    /// Draggable panel widths (pixels).
    pub sidebar_width: RwSignal<f64>,
    pub agent_width: RwSignal<f64>,

    // ---- Auto-update ----------------------------------------------------
    /// The available update, if GitHub reports a newer release.
    pub update_info: RwSignal<Option<crate::updater::UpdateInfo>>,
    /// Progress of the current check/install.
    pub update_status: RwSignal<crate::updater::UpdateStatus>,
    /// Whether the changelog is expanded in the update notice.
    pub update_notes_open: RwSignal<bool>,

    /// Go-to-line prompt state.
    pub goto: crate::editing::GotoState,
    /// Buffer id awaiting a close confirmation (unsaved changes).
    pub close_confirm: RwSignal<Option<u64>>,
    /// Most-recently-used files (newest first) and the ⌘E switcher state.
    pub recent_files: RwSignal<Vec<PathBuf>>,
    pub recent: crate::recent::RecentState,
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
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
            active2: RwSignal::new(None),
            split: RwSignal::new(false),
            focused: RwSignal::new(0),
            next_id: RwSignal::new(1),
            palette_open: RwSignal::new(false),
            lsp_clients: RwSignal::new(HashMap::new()),
            lsp_failed: RwSignal::new(HashSet::new()),
            diagnostics: RwSignal::new(HashMap::new()),
            diag_tx: RwSignal::new(tx),
            diag_rx: RwSignal::new(Some(rx)),
            completion: Completion::new(),
            hover: HoverState::new(),
            signature: SignatureState::new(),
            laravel: RwSignal::new(None),
            picker: Picker::new(),
            terminals: RwSignal::new(Vec::new()),
            active_terminal: RwSignal::new(None),
            active_terminal2: RwSignal::new(None),
            term_split: RwSignal::new(false),
            term_focus_pane: RwSignal::new(0),
            next_term_id: RwSignal::new(1),
            term_rename_id: RwSignal::new(None),
            term_rename_input: RwSignal::new(String::new()),
            terminal_open: RwSignal::new(false),
            terminal_focused: RwSignal::new(false),
            term_tick: RwSignal::new(0),
            term_tx: RwSignal::new(term_tx),
            term_rx: RwSignal::new(Some(term_rx)),
            outline: RwSignal::new(Vec::new()),
            find: FindState::new(),
            rename: RenameState::new(),
            last_edit: RwSignal::new(0),
            md_preview: RwSignal::new(false),
            cmd: CmdPalette::new(),
            diff_open: RwSignal::new(false),
            settings: config::load_settings(),
            sidebar_open: RwSignal::new(true),
            file_op: FileOp::new(),
            fs_rev: RwSignal::new(0),
            about_open: RwSignal::new(false),
            agent_open: RwSignal::new(false),
            agents: RwSignal::new(config::load_agents()),
            agent_current: RwSignal::new(config::load_default_agent()),
            agent_term: RwSignal::new(None),
            agent_focused: RwSignal::new(false),
            agent_focus_pulse: RwSignal::new(0),
            sidebar_width: RwSignal::new(240.0),
            agent_width: RwSignal::new(460.0),
            update_info: RwSignal::new(None),
            update_status: RwSignal::new(crate::updater::UpdateStatus::Idle),
            update_notes_open: RwSignal::new(false),
            goto: crate::editing::GotoState::new(),
            close_confirm: RwSignal::new(None),
            recent_files: RwSignal::new(Vec::new()),
            recent: crate::recent::RecentState::new(),
        }
    }

    // ---- File explorer operations --------------------------------------

    /// Open the name prompt for a file operation rooted at `path`.
    pub fn start_file_op(&self, kind: FileOpKind, path: PathBuf) {
        let op = self.file_op;
        op.kind.set(kind);
        match kind {
            FileOpKind::NewFile | FileOpKind::NewFolder => {
                let base = if path.is_dir() {
                    path
                } else {
                    path.parent().map(|p| p.to_path_buf()).unwrap_or(path)
                };
                op.base.set(base);
                op.input.set(String::new());
            }
            FileOpKind::Rename => {
                op.input.set(
                    path.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                );
                op.base.set(path);
            }
            FileOpKind::Duplicate => {
                op.input.set(duplicate_name(&path));
                op.base.set(path);
            }
        }
        op.open.set(true);
    }

    /// Apply the pending file operation.
    pub fn confirm_file_op(&self) {
        let op = self.file_op;
        let kind = op.kind.get_untracked();
        let base = op.base.get_untracked();
        let name = op.input.get_untracked().trim().to_string();
        op.open.set(false);
        if name.is_empty() {
            return;
        }

        let mut open_after: Option<PathBuf> = None;
        let res: std::io::Result<()> = match kind {
            FileOpKind::NewFile => {
                let p = base.join(&name);
                let r = if p.exists() {
                    Ok(())
                } else {
                    std::fs::write(&p, "")
                };
                if r.is_ok() {
                    open_after = Some(p);
                }
                r
            }
            FileOpKind::NewFolder => std::fs::create_dir_all(base.join(&name)),
            FileOpKind::Rename => {
                let dst = base
                    .parent()
                    .map(|p| p.join(&name))
                    .unwrap_or_else(|| PathBuf::from(&name));
                std::fs::rename(&base, &dst)
            }
            FileOpKind::Duplicate => {
                let dst = base
                    .parent()
                    .map(|p| p.join(&name))
                    .unwrap_or_else(|| PathBuf::from(&name));
                copy_recursive(&base, &dst)
            }
        };
        if let Err(e) = res {
            eprintln!("e: file operation failed: {e}");
        }
        self.fs_rev.update(|r| *r += 1);
        if let Some(p) = open_after {
            self.open_path(p);
        }
    }

    /// Move a path to the Trash (recoverable) and close any open buffer for it.
    pub fn delete_path(&self, path: PathBuf) {
        let script = format!(
            "tell application \"Finder\" to delete POSIX file \"{}\"",
            path.display()
        );
        let _ = std::process::Command::new("osascript")
            .arg("-e")
            .arg(script)
            .output();
        if let Some(id) = self.buffers.with(|bs| {
            bs.iter()
                .find(|b| b.file.path.as_deref() == Some(path.as_path()))
                .map(|b| b.id)
        }) {
            self.close(id);
        }
        self.fs_rev.update(|r| *r += 1);
    }

    pub fn copy_path_to_clipboard(&self, path: &std::path::Path) {
        let _ = floem::Clipboard::set_contents(path.display().to_string());
    }

    pub fn reveal_in_finder(&self, path: &std::path::Path) {
        let _ = std::process::Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn();
    }

    pub fn toggle_md_preview(&self) {
        let cur = self.md_preview.get_untracked();
        self.md_preview.set(!cur);
    }

    pub fn toggle_diff(&self) {
        let cur = self.diff_open.get_untracked();
        self.diff_open.set(!cur);
    }

    // ---- Local rename --------------------------------------------------

    /// Open the rename bar for the identifier under the cursor.
    pub fn open_rename(&self) {
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let Some(editor) = buf.editor.get_untracked() else {
            return;
        };
        let offset = editor.cursor.get_untracked().offset();
        let text = buf.doc.text().to_string();
        let word = word_at(&text, offset);
        if word.is_empty() {
            return;
        }
        let r = self.rename;
        r.word.set(word.clone());
        r.new_name.set(word);
        r.open.set(true);
    }

    pub fn close_rename(&self) {
        self.rename.open.set(false);
    }

    /// Multi-cursor (⌘D): expand the caret to its word, or add a cursor at the
    /// next occurrence of the current selection.
    pub fn select_next_occurrence(&self) {
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let Some(editor) = buf.editor.get_untracked() else {
            return;
        };
        let cursor = editor.cursor.get_untracked();
        let CursorMode::Insert(sel) = cursor.mode.clone() else {
            return;
        };
        let text = buf.doc.text().to_string();
        let regions = sel.regions().to_vec();
        let all_carets = regions.iter().all(|r| r.start == r.end);

        let new_sel = if all_carets {
            // Expand each caret to the surrounding word.
            let mut s = Selection::new();
            for r in &regions {
                let (a, b) = word_range(&text, r.max());
                if b > a {
                    s.add_region(SelRegion::new(a, b, None));
                } else {
                    s.add_region(*r);
                }
            }
            s
        } else {
            // Add the next occurrence of the last non-empty region's text.
            let mut s = sel.clone();
            if let Some(last) = regions.iter().rev().find(|r| r.max() > r.min()) {
                let word = text[last.min()..last.max()].to_string();
                if let Some(pos) = find_next(&text, &word, last.max()) {
                    s.add_region(SelRegion::new(pos, pos + word.len(), None));
                }
            }
            s
        };

        editor
            .cursor
            .set(Cursor::new(CursorMode::Insert(new_sel), None, None));
    }

    /// Replace every whole-word occurrence of the original identifier.
    pub fn apply_rename(&self) {
        let r = self.rename;
        if !r.open.get_untracked() {
            return;
        }
        let word = r.word.get_untracked();
        let new_name = r.new_name.get_untracked();
        r.open.set(false);
        if new_name.is_empty() || new_name == word {
            return;
        }
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let text = buf.doc.text().to_string();
        let occ = whole_word_occurrences(&text, &word);
        if occ.is_empty() {
            return;
        }
        let edits: Vec<(Selection, String)> = occ
            .iter()
            .map(|(s, e)| (Selection::region(*s, *e), new_name.clone()))
            .collect();
        let mut it = edits.iter().map(|(s, t)| (s.clone(), t.as_str()));
        buf.doc.edit(&mut it, EditType::InsertChars);
    }

    /// Save all dirty buffers to disk (no formatting) — used by idle auto-save.
    pub fn maybe_autosave(&self) {
        if !self.settings.autosave {
            return;
        }
        let last = self.last_edit.get_untracked();
        if last == 0 || now_ms().saturating_sub(last) < 1500 {
            return;
        }
        self.last_edit.set(0);
        let buffers = self.buffers.get_untracked();
        for b in &buffers {
            if !b.dirty.get_untracked() {
                continue;
            }
            let Some(path) = b.file.path.as_ref() else {
                continue;
            };
            let text = b.doc.text().to_string();
            if buffer::write(path, &text).is_ok() {
                b.dirty.set(false);
                Self::refresh_disk_mtime(b);
                if let (Some(uri), Some(client)) =
                    (b.uri.as_ref(), self.lsp_for_language(b.file.language))
                {
                    client.did_save(uri, &text);
                }
            }
        }
    }

    // ---- External file changes -----------------------------------------

    /// Read and store the on-disk mtime for a buffer (after we write it, to
    /// avoid mistaking our own save for an external change).
    fn refresh_disk_mtime(buf: &Buffer) {
        let mtime = buf
            .file
            .path
            .as_ref()
            .and_then(|p| std::fs::metadata(p).ok())
            .and_then(|m| m.modified().ok());
        buf.disk_mtime.set(mtime);
    }

    /// Poll open files for on-disk changes (called on the idle tick). Clean
    /// buffers are reloaded silently; dirty ones are flagged for the user.
    pub fn check_external_changes(&self) {
        let buffers = self.buffers.get_untracked();
        for b in &buffers {
            let Some(path) = b.file.path.as_ref() else {
                continue;
            };
            let Some(mtime) = std::fs::metadata(path).ok().and_then(|m| m.modified().ok()) else {
                continue;
            };
            match b.disk_mtime.get_untracked() {
                None => b.disk_mtime.set(Some(mtime)),
                Some(prev) if prev != mtime => {
                    b.disk_mtime.set(Some(mtime));
                    if b.dirty.get_untracked() {
                        b.disk_changed.set(true);
                    } else {
                        self.reload_buffer(b);
                    }
                }
                _ => {}
            }
        }
    }

    /// Reload a buffer's contents from disk, discarding any unsaved edits.
    fn reload_buffer(&self, buf: &Buffer) {
        let Some(path) = buf.file.path.as_ref() else {
            return;
        };
        let Ok(content) = std::fs::read_to_string(path) else {
            return;
        };
        if content == buf.doc.text().to_string() {
            buf.disk_changed.set(false);
            return;
        }
        let old_len = buf.doc.text().len();
        let mut it = std::iter::once((Selection::region(0, old_len), content.as_str()));
        buf.doc.edit(&mut it, EditType::InsertChars);
        buf.dirty.set(false);
        buf.disk_changed.set(false);
        Self::refresh_disk_mtime(buf);
    }

    /// Reload the active buffer from disk (used by the conflict banner).
    pub fn reload_active_from_disk(&self) {
        if let Some(buf) = self.active_buffer() {
            self.reload_buffer(&buf);
        }
    }

    /// Dismiss the disk-change conflict, keeping the in-memory version.
    pub fn keep_active_version(&self) {
        if let Some(buf) = self.active_buffer() {
            buf.disk_changed.set(false);
        }
    }

    // ---- Find in file --------------------------------------------------

    pub fn open_find(&self) {
        self.find.open.set(true);
        self.find.replace_open.set(false);
    }

    /// Open the find bar with the replace row expanded.
    pub fn open_replace(&self) {
        self.find.open.set(true);
        self.find.replace_open.set(true);
    }

    pub fn close_find(&self) {
        self.find.open.set(false);
        self.find.matches.set(Vec::new());
        if let Some(buf) = self.active_buffer() {
            *buf.find_marks.borrow_mut() = Vec::new();
            buf.doc.cache_rev().update(|r| *r += 1);
        }
    }

    /// Recompute matches for the current query (called as the query changes).
    pub fn run_find(&self) {
        let query = self.find.query.get_untracked();
        let Some(buf) = self.active_buffer() else {
            return;
        };
        if query.is_empty() {
            self.find.matches.set(Vec::new());
            *buf.find_marks.borrow_mut() = Vec::new();
            buf.doc.cache_rev().update(|r| *r += 1);
            return;
        }
        let text = buf.doc.text().to_string();
        let matches = find_all_opts(
            &text,
            &query,
            self.find.case_sensitive.get_untracked(),
            self.find.whole_word.get_untracked(),
            self.find.use_regex.get_untracked(),
        );
        self.find.matches.set(matches);
        self.find.current.set(0);
        self.apply_find_marks();
    }

    /// Replace the current match with the replacement text, then re-search.
    pub fn replace_current(&self) {
        let matches = self.find.matches.get_untracked();
        if matches.is_empty() {
            return;
        }
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let cur = self.find.current.get_untracked().min(matches.len() - 1);
        let (s, e) = matches[cur];
        let rep = self.find.replace.get_untracked();
        let sel = Selection::region(s, e);
        let mut it = std::iter::once((sel, rep.as_str()));
        buf.doc.edit(&mut it, EditType::InsertChars);
        self.run_find();
    }

    /// Replace every match with the replacement text in one edit.
    pub fn replace_all(&self) {
        let matches = self.find.matches.get_untracked();
        if matches.is_empty() {
            return;
        }
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let rep = self.find.replace.get_untracked();
        let edits: Vec<(Selection, String)> = matches
            .iter()
            .map(|(s, e)| (Selection::region(*s, *e), rep.clone()))
            .collect();
        let mut it = edits.iter().map(|(s, t)| (s.clone(), t.as_str()));
        buf.doc.edit(&mut it, EditType::InsertChars);
        self.run_find();
    }

    /// Rebuild per-line highlight spans and move the caret to the current match.
    fn apply_find_marks(&self) {
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let matches = self.find.matches.get_untracked();
        let cur = self.find.current.get_untracked();
        let text = buf.doc.text().to_string();
        let starts = line_starts(&text);
        let mut lines: Vec<Vec<FindSpan>> = vec![Vec::new(); starts.len()];
        for (idx, (s, e)) in matches.iter().enumerate() {
            let line = line_of(&starts, *s);
            let ls = starts[line];
            lines[line].push(FindSpan {
                start: s - ls,
                end: e - ls,
                current: idx == cur,
            });
        }
        *buf.find_marks.borrow_mut() = lines;
        buf.doc.cache_rev().update(|r| *r += 1);

        if let Some(editor) = buf.editor.get_untracked() {
            if let Some((s, _)) = matches.get(cur) {
                editor.cursor.set(Cursor::new(
                    CursorMode::Insert(Selection::caret(*s)),
                    None,
                    None,
                ));
            }
        }
    }

    pub fn find_next(&self) {
        let n = self.find.matches.with(|m| m.len());
        if n == 0 {
            return;
        }
        self.find
            .current
            .set((self.find.current.get_untracked() + 1) % n);
        self.apply_find_marks();
    }

    pub fn find_prev(&self) {
        let n = self.find.matches.with(|m| m.len());
        if n == 0 {
            return;
        }
        let cur = self.find.current.get_untracked();
        self.find.current.set((cur + n - 1) % n);
        self.apply_find_marks();
    }

    /// Recompute the matching-bracket highlight for the active buffer and
    /// repaint. Called from a cursor-tracking effect.
    pub fn update_bracket_marks(&self) {
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let Some(editor) = buf.editor.get_untracked() else {
            return;
        };
        let offset = editor.cursor.get_untracked().offset();
        let text = buf.doc.text().to_string();
        *buf.bracket_marks.borrow_mut() = compute_bracket_marks(&text, offset);
        buf.doc.cache_rev().update(|r| *r += 1);
    }

    /// Load the document outline for the active buffer (LSP documentSymbol).
    pub fn request_outline(&self) {
        let outline = self.outline;
        let Some(buf) = self.active_buffer() else {
            outline.set(Vec::new());
            return;
        };
        let (Some(client), Some(uri)) = (self.lsp_for_active(), buf.uri.clone()) else {
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

    fn term_by_id(&self, id: u64) -> Option<Rc<RefCell<Terminal>>> {
        self.terminals
            .with_untracked(|ts| ts.iter().find(|t| t.id == id).map(|t| t.term.clone()))
    }

    /// The active-terminal signal of the given pane (0 or 1).
    fn pane_active(&self, pane: u8) -> RwSignal<Option<u64>> {
        if pane == 1 {
            self.active_terminal2
        } else {
            self.active_terminal
        }
    }

    /// The focused pane's active terminal id (reactive).
    pub fn focused_term_id(&self) -> Option<u64> {
        if self.term_focus_pane.get() == 1 {
            self.active_terminal2.get()
        } else {
            self.active_terminal.get()
        }
    }

    /// Spawn a new terminal in the focused pane, show the panel.
    pub fn new_terminal(&self) {
        let pane = self.term_focus_pane.get_untracked();
        if let Some(id) = self.spawn_terminal() {
            self.pane_active(pane).set(Some(id));
            self.terminal_open.set(true);
        }
    }

    fn spawn_terminal(&self) -> Option<u64> {
        let tx = self.term_tx.get();
        let on_update = Box::new(move || {
            let _ = tx.send(());
        });
        let root = self.root.get();
        match Terminal::spawn(&e_term::default_shell(), &root, 24, 100, on_update) {
            Ok(t) => {
                let id = self.next_term_id.get_untracked();
                self.next_term_id.set(id + 1);
                self.terminals.update(|ts| {
                    ts.push(TermSession {
                        id,
                        term: Rc::new(RefCell::new(t)),
                        name: RwSignal::new(String::new()),
                    })
                });
                Some(id)
            }
            Err(e) => {
                eprintln!("e: terminal failed: {e:#}");
                None
            }
        }
    }

    /// Split the terminal: open a new shell in the second pane.
    pub fn split_terminal(&self) {
        if let Some(id) = self.spawn_terminal() {
            self.active_terminal2.set(Some(id));
            self.term_split.set(true);
            self.term_focus_pane.set(1);
            self.terminal_open.set(true);
        }
    }

    pub fn rename_terminal(&self, id: u64, name: String) {
        self.terminals.with_untracked(|ts| {
            if let Some(s) = ts.iter().find(|t| t.id == id) {
                s.name.set(name);
            }
        });
    }

    /// Open the rename prompt for a terminal tab.
    pub fn start_term_rename(&self, id: u64) {
        let current = self.terminals.with_untracked(|ts| {
            ts.iter()
                .find(|t| t.id == id)
                .map(|t| t.name.get_untracked())
        });
        self.term_rename_input.set(current.unwrap_or_default());
        self.term_rename_id.set(Some(id));
    }

    pub fn confirm_term_rename(&self) {
        if let Some(id) = self.term_rename_id.get_untracked() {
            let name = self.term_rename_input.get_untracked().trim().to_string();
            self.rename_terminal(id, name);
        }
        self.term_rename_id.set(None);
    }

    /// Toggle the terminal panel, spawning the first shell on first use.
    pub fn toggle_terminal(&self) {
        if self.terminals.with_untracked(|t| t.is_empty()) {
            self.new_terminal();
        } else {
            let open = self.terminal_open.get_untracked();
            self.terminal_open.set(!open);
        }
    }

    /// Focus a terminal in the currently focused pane (clicking a tab).
    pub fn focus_terminal(&self, id: u64) {
        let pane = self.term_focus_pane.get_untracked();
        self.pane_active(pane).set(Some(id));
    }

    /// Close a terminal session (kills its shell).
    pub fn close_terminal(&self, id: u64) {
        let mut next = None;
        self.terminals.update(|ts| {
            if let Some(pos) = ts.iter().position(|t| t.id == id) {
                ts.remove(pos);
                if !ts.is_empty() {
                    next = Some(ts[pos.min(ts.len() - 1)].id);
                }
            }
        });
        // Replace the closed id wherever it was active.
        if self.active_terminal.get_untracked() == Some(id) {
            self.active_terminal.set(next);
        }
        if self.active_terminal2.get_untracked() == Some(id) {
            self.active_terminal2.set(None);
            self.term_split.set(false);
            self.term_focus_pane.set(0);
        }
        if self.terminals.with_untracked(|t| t.is_empty()) {
            self.terminal_open.set(false);
        }
    }

    pub fn term_input_to(&self, id: u64, bytes: &[u8]) {
        if let Some(t) = self.term_by_id(id) {
            t.borrow_mut().write(bytes);
        }
    }

    /// Resize every terminal to the pane size.
    pub fn resize_terminal(&self, rows: usize, cols: usize) {
        self.terminals.with_untracked(|ts| {
            for t in ts {
                t.term.borrow().resize(rows, cols);
            }
        });
    }

    pub fn term_runs_of(&self, id: Option<u64>) -> Vec<Vec<e_term::Run>> {
        id.and_then(|i| self.term_by_id(i))
            .map(|t| t.borrow().snapshot_runs())
            .unwrap_or_default()
    }

    pub fn term_cursor_of(&self, id: Option<u64>) -> (usize, usize) {
        id.and_then(|i| self.term_by_id(i))
            .map(|t| t.borrow().cursor())
            .unwrap_or((0, 0))
    }

    // ---- Agent panel ----------------------------------------------------

    /// The currently selected agent's config.
    pub fn current_agent(&self) -> Option<AgentConfig> {
        let id = self.agent_current.get_untracked();
        self.agents
            .with_untracked(|list| list.iter().find(|a| a.id == id).cloned())
            .or_else(|| self.agents.with_untracked(|l| l.first().cloned()))
    }

    /// Toggle the agent panel, launching the agent on first open.
    pub fn toggle_agent(&self) {
        let open = self.agent_open.get_untracked();
        if open {
            self.agent_open.set(false);
        } else {
            self.agent_open.set(true);
            if self.agent_term.get_untracked().is_none() {
                self.start_agent();
            }
            self.agent_focus_pulse.update(|x| *x += 1);
        }
    }

    /// (Re)start the selected agent in a fresh PTY.
    pub fn start_agent(&self) {
        let Some(agent) = self.current_agent() else {
            eprintln!("e: no agent configured");
            return;
        };
        let cwd = if agent.cwd.trim().is_empty() {
            self.root.get_untracked()
        } else {
            PathBuf::from(&agent.cwd)
        };
        let tx = self.term_tx.get_untracked();
        let on_update = Box::new(move || {
            let _ = tx.send(());
        });
        match Terminal::spawn_command(&agent.command, &cwd, 30, 100, on_update) {
            Ok(t) => self.agent_term.set(Some(Rc::new(RefCell::new(t)))),
            Err(e) => eprintln!("e: agent '{}' failed: {e:#}", agent.name),
        }
    }

    /// Switch to a different agent and restart the panel with it.
    pub fn select_agent(&self, id: &str) {
        self.agent_current.set(id.to_string());
        config::save_default_agent(id);
        self.agent_term.set(None);
        self.start_agent();
        self.agent_focus_pulse.update(|x| *x += 1);
    }

    pub fn restart_agent(&self) {
        self.agent_term.set(None);
        self.start_agent();
        self.agent_focus_pulse.update(|x| *x += 1);
    }

    pub fn agent_input(&self, bytes: &[u8]) {
        if let Some(t) = self.agent_term.get_untracked() {
            t.borrow_mut().write(bytes);
        }
    }

    pub fn agent_runs(&self) -> Vec<Vec<e_term::Run>> {
        self.agent_term
            .get_untracked()
            .map(|t| t.borrow().snapshot_runs())
            .unwrap_or_default()
    }

    pub fn agent_cursor(&self) -> (usize, usize) {
        self.agent_term
            .get_untracked()
            .map(|t| t.borrow().cursor())
            .unwrap_or((0, 0))
    }

    pub fn resize_agent(&self, rows: usize, cols: usize) {
        if let Some(t) = self.agent_term.get_untracked() {
            t.borrow().resize(rows, cols);
        }
    }

    /// Open the global settings file in the editor.
    pub fn open_settings(&self) {
        if let Some(path) = config::settings_path() {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if !path.exists() {
                let _ = std::fs::write(&path, "{\n}\n");
            }
            self.open_path(path);
        }
    }

    // ---- Auto-update ----------------------------------------------------

    /// Check GitHub for a newer release (non-blocking). `announce_up_to_date`
    /// controls whether an "already current" result is surfaced in the status.
    pub fn check_for_updates(&self, announce_up_to_date: bool) {
        use crate::updater::{self, UpdateStatus};
        if self.update_status.get_untracked() == UpdateStatus::Downloading {
            return;
        }
        self.update_status.set(UpdateStatus::Checking);
        let info_sig = self.update_info;
        let status_sig = self.update_status;
        let send = create_ext_action(self.cx, move |result: Option<updater::UpdateInfo>| {
            match result {
                Some(info) => {
                    info_sig.set(Some(info));
                    status_sig.set(UpdateStatus::Idle);
                }
                None => {
                    status_sig.set(if announce_up_to_date {
                        UpdateStatus::UpToDate
                    } else {
                        UpdateStatus::Idle
                    });
                }
            }
        });
        std::thread::spawn(move || {
            let result = updater::check().unwrap_or(None);
            send(result);
        });
    }

    /// Download and install the available update in place (non-blocking).
    pub fn install_update(&self) {
        use crate::updater::{self, UpdateStatus};
        if self.update_status.get_untracked() == UpdateStatus::Downloading {
            return;
        }
        self.update_status.set(UpdateStatus::Downloading);
        let status_sig = self.update_status;
        let send = create_ext_action(self.cx, move |result: Result<(), String>| match result {
            Ok(()) => status_sig.set(UpdateStatus::Installed),
            Err(e) => status_sig.set(UpdateStatus::Failed(e)),
        });
        std::thread::spawn(move || {
            let result = updater::install().map_err(|e| format!("{e:#}"));
            send(result);
        });
    }

    /// Dismiss the update notice (until the next check).
    pub fn dismiss_update(&self) {
        self.update_info.set(None);
        self.update_notes_open.set(false);
        self.update_status.set(crate::updater::UpdateStatus::Idle);
    }

    /// Relaunch the application (used after an update is installed).
    pub fn restart_app(&self) {
        let exe = std::env::current_exe().ok();
        // If we're running inside a macOS .app bundle, relaunch the bundle so
        // the window comes to the front; otherwise relaunch the bare binary.
        if let Some(exe) = exe.as_ref() {
            let bundle = exe
                .ancestors()
                .find(|p| p.extension().map(|e| e == "app").unwrap_or(false));
            if let Some(bundle) = bundle {
                let _ = std::process::Command::new("open")
                    .arg("-n")
                    .arg(bundle)
                    .spawn();
                std::process::exit(0);
            }
        }
        if let Some(exe) = exe {
            let _ = std::process::Command::new(exe)
                .arg(self.root.get_untracked())
                .spawn();
        }
        std::process::exit(0);
    }

    pub fn buffer_by_id(&self, id: u64) -> Option<Buffer> {
        self.buffers
            .with(|bs| bs.iter().find(|b| b.id == id).cloned())
    }

    /// The active-buffer signal of the focused pane.
    fn focused_active(&self) -> RwSignal<Option<u64>> {
        if self.focused.get_untracked() == 1 {
            self.active2
        } else {
            self.active
        }
    }

    /// Buffer id active in the focused pane, tracked reactively.
    pub fn focused_active_id(&self) -> Option<u64> {
        if self.focused.get() == 1 {
            self.active2.get()
        } else {
            self.active.get()
        }
    }

    /// Focus a buffer in the currently focused pane (e.g. clicking a tab).
    pub fn focus_buffer(&self, id: u64) {
        self.focused_active().set(Some(id));
    }

    fn buffer_id_by_path(&self, path: &str) -> Option<u64> {
        let canon = std::path::Path::new(path).canonicalize().ok();
        self.buffers.with(|bs| {
            bs.iter()
                .find(|b| b.file.path.as_deref() == canon.as_deref())
                .map(|b| b.id)
        })
    }

    /// Restore the previous session for this workspace (open files, tabs, split).
    pub fn restore_session(&self) {
        let Some(data) = session::load(&self.root.get_untracked()) else {
            return;
        };
        for p in &data.open {
            self.open_path(PathBuf::from(p));
        }
        if let Some(a) = data
            .active
            .as_deref()
            .and_then(|a| self.buffer_id_by_path(a))
        {
            self.active.set(Some(a));
        }
        if data.split {
            self.split.set(true);
            if let Some(a2) = data
                .active2
                .as_deref()
                .and_then(|a| self.buffer_id_by_path(a))
            {
                self.active2.set(Some(a2));
            }
        }
    }

    /// Persist the current session.
    pub fn save_session(&self) {
        let buffers = self.buffers.get_untracked();
        let path_of = |id: Option<u64>| -> Option<String> {
            id.and_then(|i| buffers.iter().find(|b| b.id == i))
                .and_then(|b| b.file.path.as_ref())
                .map(|p| p.display().to_string())
        };
        let open: Vec<String> = buffers
            .iter()
            .filter_map(|b| b.file.path.as_ref().map(|p| p.display().to_string()))
            .collect();
        let data = SessionData {
            open,
            active: path_of(self.active.get_untracked()),
            active2: path_of(self.active2.get_untracked()),
            split: self.split.get_untracked(),
        };
        session::save(&self.root.get_untracked(), &data);
    }

    /// Toggle the two-pane split view.
    pub fn toggle_split(&self) {
        let on = !self.split.get_untracked();
        self.split.set(on);
        if on {
            if self.active2.get_untracked().is_none() {
                self.active2.set(self.active.get_untracked());
            }
            self.focused.set(1);
        } else {
            self.focused.set(0);
        }
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
        comp.anchor
            .set(Point::new(win.x + below.x - vp.x0, win.y + below.y - vp.y0));
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

    /// Look up a running language server for `language` (does not start one).
    pub fn lsp_for_language(&self, language: Language) -> Option<Arc<LspClient>> {
        let spec = server_spec(language)?;
        self.lsp_clients.with(|m| m.get(spec.id).cloned())
    }

    /// The language server for the active buffer, if running.
    pub fn lsp_for_active(&self) -> Option<Arc<LspClient>> {
        self.lsp_for_language(self.active_buffer()?.file.language)
    }

    /// Start (or reuse) the language server for `language`.
    fn ensure_lsp(&self, language: Language) -> Option<Arc<LspClient>> {
        let spec = server_spec(language)?;
        if let Some(client) = self.lsp_clients.with(|m| m.get(spec.id).cloned()) {
            return Some(client);
        }
        if self.lsp_failed.with(|f| f.contains(spec.id)) {
            return None;
        }
        let tx = self.diag_tx.get();
        let handler: e_lsp::DiagnosticsHandler = Box::new(move |p| {
            let _ = tx.send(p);
        });
        let root = self.root.get();
        match LspClient::start(spec.program, spec.args, &root, handler) {
            Ok(client) => {
                eprintln!("e: started {} for {}", spec.id, root.display());
                self.lsp_clients.update(|m| {
                    m.insert(spec.id.to_string(), client.clone());
                });
                Some(client)
            }
            Err(e) => {
                eprintln!("e: could not start {} ({e:#})", spec.program);
                self.lsp_failed.update(|f| {
                    f.insert(spec.id.to_string());
                });
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
            self.focused_active().set(Some(id));
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
        // Keep/auto-indent on newline (matches editor expectations).
        doc.auto_indent.set(true);
        let dirty = RwSignal::new(false);
        let version = RwSignal::new(1i64);

        // Hand the document to the language server, if we have one.
        if let (Some(lang_id), Some(uri)) = (lsp_language_id(language), uri.as_ref()) {
            if let Some(client) = self.ensure_lsp(language) {
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
                app.last_edit.set(now_ms());
                let text = doc.text().to_string();
                *highlights.borrow_mut() = highlight_lines(language, &text);
                if let Some(head) = &head_text {
                    let lc = text.split_inclusive('\n').count().max(1);
                    *git_marks.borrow_mut() = git::marks(head, &text, lc);
                }
                doc.cache_rev().update(|r| *r += 1);

                if let (Some(uri), Some(client)) = (uri.as_ref(), app.lsp_for_language(language)) {
                    if lsp_language_id(language).is_some() {
                        let v = version.get() + 1;
                        version.set(v);
                        client.did_change_full(uri, v, &text);
                    }
                }
                // Trigger completion (LSP + snippets + Laravel helpers).
                app.autocomplete_after_edit(id);
            });
        }

        let disk_mtime = file
            .path
            .as_ref()
            .and_then(|p| std::fs::metadata(p).ok())
            .and_then(|m| m.modified().ok());

        let buf = Buffer {
            id,
            file,
            doc,
            dirty,
            highlights,
            diag_lines: Rc::new(RefCell::new(Vec::new())),
            git_marks,
            find_marks: Rc::new(RefCell::new(Vec::new())),
            bracket_marks: Rc::new(RefCell::new(Vec::new())),
            uri,
            editor: RwSignal::new(None),
            win_origin: RwSignal::new(Point::ZERO),
            pending_goto: RwSignal::new(None),
            disk_mtime: RwSignal::new(disk_mtime),
            disk_changed: RwSignal::new(false),
        };
        self.buffers.update(|bs| bs.push(buf));
        self.focused_active().set(Some(id));
    }

    /// Close a tab; focus a neighbour if it was active.
    /// Close a tab, prompting first if it has unsaved changes.
    pub fn close(&self, id: u64) {
        let dirty = self
            .buffers
            .with_untracked(|bs| bs.iter().find(|b| b.id == id).map(|b| b.dirty.get_untracked()))
            .unwrap_or(false);
        if dirty {
            self.close_confirm.set(Some(id));
        } else {
            self.force_close(id);
        }
    }

    /// Save the pending buffer, then close it.
    pub fn confirm_close_save(&self) {
        if let Some(id) = self.close_confirm.get_untracked() {
            self.close_confirm.set(None);
            let prev = self.focused_active().get_untracked();
            self.focused_active().set(Some(id));
            self.save_active();
            self.focused_active().set(prev);
            self.force_close(id);
        }
    }

    /// Discard changes and close the pending buffer.
    pub fn confirm_close_discard(&self) {
        if let Some(id) = self.close_confirm.get_untracked() {
            self.close_confirm.set(None);
            self.force_close(id);
        }
    }

    pub fn cancel_close(&self) {
        self.close_confirm.set(None);
    }

    pub fn force_close(&self, id: u64) {
        let mut focus_next = None;
        let mut closed_uri = None;
        let mut closed_lang = None;
        self.buffers.update(|bs| {
            if let Some(pos) = bs.iter().position(|b| b.id == id) {
                closed_uri = bs[pos].uri.clone();
                closed_lang = Some(bs[pos].file.language);
                bs.remove(pos);
                if !bs.is_empty() {
                    let n = pos.min(bs.len() - 1);
                    focus_next = Some(bs[n].id);
                }
            }
        });
        if self.active.get_untracked() == Some(id) {
            self.active.set(focus_next);
        }
        if self.active2.get_untracked() == Some(id) {
            self.active2.set(focus_next);
        }
        if let (Some(uri), Some(lang)) = (closed_uri, closed_lang) {
            if let Some(client) = self.lsp_for_language(lang) {
                client.did_close(&uri);
            }
        }
    }

    pub fn active_buffer(&self) -> Option<Buffer> {
        let active = self.focused_active_id()?;
        self.buffers
            .with(|bs| bs.iter().find(|b| b.id == active).cloned())
    }

    /// Format the active buffer in place via the language server (PHP only).
    pub fn format_active(&self) {
        let Some(buf) = self.active_buffer() else {
            return;
        };
        if lsp_language_id(buf.file.language).is_none() {
            return;
        }
        let (Some(client), Some(uri), Some(editor)) = (
            self.lsp_for_active(),
            buf.uri.clone(),
            buf.editor.get_untracked(),
        ) else {
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
                let s = editor.offset_of_line_col(
                    e.range.start.line as usize,
                    e.range.start.character as usize,
                );
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

    /// Strip trailing whitespace and ensure a final newline in the active buffer.
    fn trim_active(&self) {
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let text = buf.doc.text().to_string();
        let (edits, needs_newline) = trailing_trim_edits(&text);
        if edits.is_empty() && !needs_newline {
            return;
        }
        // Delete trailing whitespace bottom-up so offsets stay valid.
        for (s, e) in edits.into_iter().rev() {
            buf.doc
                .edit_single(Selection::region(s, e), "", EditType::Delete);
        }
        if needs_newline {
            let len = buf.doc.text().len();
            buf.doc
                .edit_single(Selection::region(len, len), "\n", EditType::InsertChars);
        }
    }

    /// Save the active buffer to disk (formatting / trimming first, if enabled).
    pub fn save_active(&self) {
        if self.settings.format_on_save {
            self.format_active();
        }
        if self.settings.trim_on_save {
            self.trim_active();
        }
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
                buf.disk_changed.set(false);
                Self::refresh_disk_mtime(&buf);
                eprintln!("e: saved {}", path.display());
                if let (Some(uri), Some(client)) =
                    (buf.uri.as_ref(), self.lsp_for_language(buf.file.language))
                {
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

    /// `(line, col, selection_len)` of the active editor's cursor (1-based).
    /// Reactive: reads the cursor signal, so call it inside a view closure.
    pub fn cursor_info(&self) -> Option<(usize, usize, usize)> {
        let buf = self.active_buffer()?;
        let editor = buf.editor.get()?;
        let cursor = editor.cursor.get();
        let offset = cursor.offset();
        let (line, col) = editor.offset_to_line_col(offset);
        let sel_len = match &cursor.mode {
            CursorMode::Insert(sel) => sel.regions().iter().map(|r| r.max() - r.min()).sum(),
            _ => 0,
        };
        Some((line + 1, col + 1, sel_len))
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

    /// All non-empty diagnostics across open files, grouped and sorted.
    pub fn all_diagnostics(&self) -> Vec<(String, Vec<Diagnostic>)> {
        self.diagnostics.with(|map| {
            let mut groups: Vec<(String, Vec<Diagnostic>)> = map
                .iter()
                .filter(|(_, d)| !d.is_empty())
                .map(|(uri, d)| {
                    let mut dd = d.clone();
                    dd.sort_by_key(|x| x.range.start.line);
                    (uri.clone(), dd)
                })
                .collect();
            groups.sort_by(|a, b| a.0.cmp(&b.0));
            groups
        })
    }

    /// Total number of diagnostics across all open files.
    pub fn total_diagnostic_count(&self) -> usize {
        self.diagnostics.with(|m| m.values().map(|v| v.len()).sum())
    }

    /// A `file://` URI shown relative to the workspace root.
    pub fn rel_path(&self, uri: &str) -> String {
        rel_uri(uri, &self.root.get())
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

        // Snippets are computed synchronously and shown first.
        let snippet_items = snippets::completion_items(buf.file.language, &word);

        let show = move |items: Vec<lsp_types::CompletionItem>| {
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
                        show(items);
                    });
                std::thread::spawn(move || {
                    let items = client
                        .completion(&uri, line as u32, col as u32)
                        .unwrap_or_default();
                    send(items);
                });
            }
            _ => show(snippet_items),
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
                let (expanded, caret) = snippets::expand(body, &indent);
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
        true
    }

    pub fn request_hover(&self) {
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
        p.items.set(Vec::new());
        p.selected.set(0);
        p.open.set(true);
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
        let root = self.root.get();
        let send = create_ext_action(self.cx, move |(g, items): (u64, Vec<PickerItem>)| {
            if g == p.gen.get_untracked() {
                p.items.set(items);
                p.selected.set(0);
            }
        });
        std::thread::spawn(move || {
            let items = grep_workspace(&root, &query, 300);
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

    /// Open `uri` and place the caret at `(line, col)`.
    pub fn jump_to(&self, uri: &str, line: usize, col: usize) {
        self.open_path(uri_to_path(uri));
        let Some(buf) = self.active_buffer() else {
            return;
        };
        if let Some(editor) = buf.editor.get_untracked() {
            let offset = editor.offset_of_line_col(line, col);
            editor.cursor.set(Cursor::new(
                CursorMode::Insert(Selection::caret(offset)),
                None,
                None,
            ));
        } else {
            // The editor view isn't built yet; apply once it is.
            buf.pending_goto.set(Some((line, col)));
        }
    }
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

pub(crate) fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
}

/// Byte ranges of trailing whitespace per line, plus whether a final newline
/// is missing. Used by trim-on-save.
fn trailing_trim_edits(text: &str) -> (Vec<(usize, usize)>, bool) {
    let mut edits = Vec::new();
    let mut off = 0;
    for line in text.split_inclusive('\n') {
        let content = line.strip_suffix('\n').unwrap_or(line);
        let trimmed = content.trim_end_matches([' ', '\t', '\r']);
        if trimmed.len() < content.len() {
            edits.push((off + trimmed.len(), off + content.len()));
        }
        off += line.len();
    }
    let needs_newline = !text.is_empty() && !text.ends_with('\n');
    (edits, needs_newline)
}

/// Leading whitespace of the line containing `offset`.
fn line_indent(text: &str, offset: usize) -> String {
    let offset = offset.min(text.len());
    let ls = text[..offset].rfind('\n').map(|i| i + 1).unwrap_or(0);
    text[ls..]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect()
}

/// Byte range of the identifier surrounding `offset`.
fn word_range(text: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(text.len());
    let mut start = offset;
    for (i, c) in text[..offset].char_indices().rev() {
        if is_word_char(c) {
            start = i;
        } else {
            break;
        }
    }
    let mut end = offset;
    for (i, c) in text[offset..].char_indices() {
        if is_word_char(c) {
            end = offset + i + c.len_utf8();
        } else {
            break;
        }
    }
    (start, end)
}

/// The identifier surrounding `offset`, if any.
fn word_at(text: &str, offset: usize) -> String {
    let (start, end) = word_range(text, offset);
    text[start..end].to_string()
}

/// Next occurrence of `word` at or after `from`, wrapping to the start.
fn find_next(text: &str, word: &str, from: usize) -> Option<usize> {
    if word.is_empty() {
        return None;
    }
    let from = from.min(text.len());
    if let Some(p) = text[from..].find(word) {
        return Some(from + p);
    }
    text[..from].find(word)
}

/// Byte ranges of every whole-word (identifier-boundary) occurrence of `word`.
fn whole_word_occurrences(text: &str, word: &str) -> Vec<(usize, usize)> {
    let (hay, w) = (text.as_bytes(), word.as_bytes());
    let mut out = Vec::new();
    if w.is_empty() || w.len() > hay.len() {
        return out;
    }
    let mut i = 0;
    while i + w.len() <= hay.len() {
        if &hay[i..i + w.len()] == w {
            let before = i == 0 || !is_word_byte(hay[i - 1]);
            let after = i + w.len() >= hay.len() || !is_word_byte(hay[i + w.len()]);
            if before && after {
                out.push((i, i + w.len()));
                i += w.len();
                continue;
            }
        }
        i += 1;
    }
    out
}

/// All non-overlapping matches of `query` in `text`, honouring the
/// case-sensitive / whole-word / regex options.
fn find_all_opts(
    text: &str,
    query: &str,
    case: bool,
    word: bool,
    regex: bool,
) -> Vec<(usize, usize)> {
    if query.is_empty() {
        return Vec::new();
    }

    if regex {
        let mut pat = query.to_string();
        if word {
            pat = format!(r"\b(?:{pat})\b");
        }
        if !case {
            pat = format!("(?i){pat}");
        }
        return match regex::Regex::new(&pat) {
            Ok(re) => re
                .find_iter(text)
                .filter(|m| m.end() > m.start())
                .map(|m| (m.start(), m.end()))
                .collect(),
            Err(_) => Vec::new(),
        };
    }

    let (h, n) = (text.as_bytes(), query.as_bytes());
    if n.len() > h.len() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i + n.len() <= h.len() {
        let hit = (0..n.len()).all(|k| {
            if case {
                h[i + k] == n[k]
            } else {
                h[i + k].eq_ignore_ascii_case(&n[k])
            }
        });
        if hit {
            let (s, e) = (i, i + n.len());
            let boundary_ok = !word
                || ((s == 0 || !is_word_byte(h[s - 1])) && (e == h.len() || !is_word_byte(h[e])));
            if boundary_ok {
                out.push((s, e));
                i = e;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Find the matching bracket for a bracket adjacent to `offset`, returning
/// per-line highlight spans for both brackets.
fn compute_bracket_marks(text: &str, offset: usize) -> Vec<Vec<(usize, usize)>> {
    let bytes = text.as_bytes();
    let opens = b"([{";
    let closes = b")]}";

    // Prefer the bracket just before the cursor, else the one at the cursor.
    let candidates = [offset.checked_sub(1), Some(offset)];
    for pos in candidates.into_iter().flatten() {
        let Some(&b) = bytes.get(pos) else { continue };
        let other = if let Some(i) = opens.iter().position(|&o| o == b) {
            find_match(bytes, pos, closes[i], b, true)
        } else if let Some(i) = closes.iter().position(|&c| c == b) {
            find_match(bytes, pos, opens[i], b, false)
        } else {
            None
        };
        if let Some(m) = other {
            let starts = line_starts(text);
            let mut lines: Vec<Vec<(usize, usize)>> = vec![Vec::new(); starts.len()];
            for p in [pos, m] {
                let line = line_of(&starts, p);
                let ls = starts[line];
                lines[line].push((p - ls, p - ls + 1));
            }
            return lines;
        }
    }
    Vec::new()
}

/// Scan for the matching bracket. `target` is the bracket we look for, `self_ch`
/// the one we started on, `forward` the scan direction.
fn find_match(bytes: &[u8], from: usize, target: u8, self_ch: u8, forward: bool) -> Option<usize> {
    let mut depth = 0i32;
    if forward {
        let mut i = from;
        while i < bytes.len() {
            let c = bytes[i];
            if c == self_ch {
                depth += 1;
            } else if c == target {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            i += 1;
        }
    } else {
        let mut i = from as isize;
        while i >= 0 {
            let c = bytes[i as usize];
            if c == self_ch {
                depth += 1;
            } else if c == target {
                depth -= 1;
                if depth == 0 {
                    return Some(i as usize);
                }
            }
            i -= 1;
        }
    }
    None
}

/// Byte offset where each line starts.
pub(crate) fn line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0usize];
    let mut off = 0;
    for line in text.split_inclusive('\n') {
        off += line.len();
        if line.ends_with('\n') {
            starts.push(off);
        }
    }
    if starts.is_empty() {
        starts.push(0);
    }
    starts
}

pub(crate) fn line_of(starts: &[usize], byte: usize) -> usize {
    starts.partition_point(|&s| s <= byte).saturating_sub(1)
}

/// Walk the workspace and collect lines matching `query` (case-insensitive).
fn grep_workspace(root: &std::path::Path, query: &str, max: usize) -> Vec<PickerItem> {
    let needle = query.to_lowercase();
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if out.len() >= max {
            break;
        }
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in read.filter_map(|e| e.ok()) {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || name == "target" || name == "node_modules" {
                continue;
            }
            let path = entry.path();
            match entry.file_type() {
                Ok(t) if t.is_dir() => stack.push(path),
                Ok(_) => {
                    // Skip large files; read the rest as UTF-8 (binaries fail).
                    if entry
                        .metadata()
                        .map(|m| m.len() > 2_000_000)
                        .unwrap_or(true)
                    {
                        continue;
                    }
                    let Ok(content) = std::fs::read_to_string(&path) else {
                        continue;
                    };
                    for (li, line) in content.lines().enumerate() {
                        if let Some(col) = line.to_lowercase().find(&needle) {
                            out.push(PickerItem {
                                label: line.trim_start().chars().take(120).collect(),
                                detail: format!(
                                    "{}:{}",
                                    rel_uri(&path_to_uri(&path), root),
                                    li + 1
                                ),
                                uri: path_to_uri(&path),
                                line: li as u32,
                                char: col as u32,
                            });
                            if out.len() >= max {
                                return out;
                            }
                        }
                    }
                }
                Err(_) => {}
            }
        }
    }
    out
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

#[cfg(test)]
mod bracket_tests {
    use super::compute_bracket_marks;
    #[test]
    fn matches_outer_paren() {
        // "foo(bar(baz))" — cursor after first '(' (offset 4)
        let m = compute_bracket_marks("foo(bar(baz))", 4);
        let mut spans: Vec<(usize, usize)> = m.into_iter().flatten().collect();
        spans.sort();
        assert_eq!(spans, vec![(3, 4), (12, 13)]);
    }
    #[test]
    fn matches_close_brace() {
        // cursor right after the closing brace
        let m = compute_bracket_marks("a{b{c}d}", 8);
        let mut spans: Vec<(usize, usize)> = m.into_iter().flatten().collect();
        spans.sort();
        assert_eq!(spans, vec![(1, 2), (7, 8)]);
    }
}

#[cfg(test)]
mod rename_tests {
    use super::{whole_word_occurrences, word_at};

    #[test]
    fn word_boundaries() {
        let t = "let foo = foo_bar + foo;";
        // whole-word 'foo' should match positions 4 and 20, NOT inside 'foo_bar'
        let occ = whole_word_occurrences(t, "foo");
        assert_eq!(occ, vec![(4, 7), (20, 23)]);
    }

    #[test]
    fn word_under_cursor() {
        let t = "$user->name";
        assert_eq!(word_at(t, 2), "$user"); // cursor inside $user
        assert_eq!(word_at(t, 8), "name"); // cursor inside name
    }
}

#[cfg(test)]
mod trim_tests {
    use super::trailing_trim_edits;

    #[test]
    fn finds_trailing_and_missing_newline() {
        // "a  \nb\t\nc" : line0 trailing 2 spaces (1..3), line1 trailing tab (5..6), no final \n
        let (edits, nl) = trailing_trim_edits("a  \nb\t\nc");
        assert_eq!(edits, vec![(1, 3), (5, 6)]);
        assert!(nl);
    }

    #[test]
    fn clean_text_no_edits() {
        let (edits, nl) = trailing_trim_edits("a\nb\n");
        assert!(edits.is_empty());
        assert!(!nl);
    }
}
