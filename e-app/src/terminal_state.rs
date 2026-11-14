//! Integrated-terminal state: PTY sessions, panes, focus, rename and scrollback.
//!
//! The view lives in [`crate::terminal_view`]; this module owns the `AppState`
//! methods that drive it. Extracted from the former `state.rs` god-module
//! (fields stay on `AppState`); same pattern as [`crate::debug`] etc.

use std::cell::RefCell;
use std::rc::Rc;

use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};

use e_term::Terminal;

use crate::state::{AppState, TermSession};

impl AppState {
    // ---- Integrated terminal -------------------------------------------

    fn term_by_id(&self, id: u64) -> Option<Rc<RefCell<Terminal>>> {
        self.term
            .sessions
            .with_untracked(|ts| ts.iter().find(|t| t.id == id).map(|t| t.term.clone()))
    }

    /// The active-terminal signal of the given pane (0 or 1).
    pub(crate) fn pane_active(&self, pane: u8) -> RwSignal<Option<u64>> {
        if pane == 1 {
            self.term.active2
        } else {
            self.term.active
        }
    }

    /// The focused pane's active terminal id (reactive).
    pub fn focused_term_id(&self) -> Option<u64> {
        if self.term.focus_pane.get() == 1 {
            self.term.active2.get()
        } else {
            self.term.active.get()
        }
    }

    /// Spawn a new terminal in the focused pane, show the panel.
    pub fn new_terminal(&self) {
        let pane = self.term.focus_pane.get_untracked();
        if let Some(id) = self.spawn_terminal() {
            self.pane_active(pane).set(Some(id));
            self.term.open.set(true);
        }
    }

    pub(crate) fn spawn_terminal(&self) -> Option<u64> {
        let tx = self.term_tx.get();
        let on_update = Box::new(move || {
            let _ = tx.send(());
        });
        let root = self.root.get();
        // The terminal is where CLI agents (Claude Code, Codex …) are run, so it
        // carries the editor's socket.
        let env = crate::agent_sync::socket_env();
        match Terminal::spawn(&e_term::default_shell(), &root, &env, 24, 100, on_update) {
            Ok(t) => {
                let id = self.next_term_id.get_untracked();
                self.next_term_id.set(id + 1);
                self.term.sessions.update(|ts| {
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
            self.term.active2.set(Some(id));
            self.term.split.set(true);
            self.term.focus_pane.set(1);
            self.term.open.set(true);
        }
    }

    pub fn rename_terminal(&self, id: u64, name: String) {
        self.term.sessions.with_untracked(|ts| {
            if let Some(s) = ts.iter().find(|t| t.id == id) {
                s.name.set(name);
            }
        });
    }

    /// Open the rename prompt for a terminal tab.
    pub fn start_term_rename(&self, id: u64) {
        let current = self.term.sessions.with_untracked(|ts| {
            ts.iter()
                .find(|t| t.id == id)
                .map(|t| t.name.get_untracked())
        });
        self.term.rename_input.set(current.unwrap_or_default());
        self.term.rename_id.set(Some(id));
    }

    pub fn confirm_term_rename(&self) {
        if let Some(id) = self.term.rename_id.get_untracked() {
            let name = self.term.rename_input.get_untracked().trim().to_string();
            self.rename_terminal(id, name);
        }
        self.term.rename_id.set(None);
    }

    /// Toggle the terminal panel, spawning the first shell on first use.
    pub fn toggle_terminal(&self) {
        if self.term.sessions.with_untracked(|t| t.is_empty()) {
            self.new_terminal();
        } else {
            let open = self.term.open.get_untracked();
            self.term.open.set(!open);
        }
    }

    /// Focus a terminal in the currently focused pane (clicking a tab).
    pub fn focus_terminal(&self, id: u64) {
        let pane = self.term.focus_pane.get_untracked();
        self.pane_active(pane).set(Some(id));
    }

    /// Close the sessions whose shell has exited (`exit`, Ctrl-D, a finished
    /// command). Called on every terminal tick; a dead PTY would otherwise sit
    /// there as a prompt that answers nothing.
    pub fn reap_exited_terminals(&self) {
        let dead: Vec<u64> = self.term.sessions.with_untracked(|ts| {
            ts.iter()
                .filter(|t| t.term.borrow().has_exited())
                .map(|t| t.id)
                .collect()
        });
        for id in dead {
            self.close_terminal(id);
        }
    }

    /// Close a terminal session (kills its shell).
    pub fn close_terminal(&self, id: u64) {
        let mut next = None;
        self.term.sessions.update(|ts| {
            if let Some(pos) = ts.iter().position(|t| t.id == id) {
                ts.remove(pos);
                if !ts.is_empty() {
                    next = Some(ts[pos.min(ts.len() - 1)].id);
                }
            }
        });
        // Replace the closed id wherever it was active.
        if self.term.active.get_untracked() == Some(id) {
            self.term.active.set(next);
        }
        if self.term.active2.get_untracked() == Some(id) {
            self.term.active2.set(None);
            self.term.split.set(false);
            self.term.focus_pane.set(0);
        }
        if self.term.sessions.with_untracked(|t| t.is_empty()) {
            self.term.open.set(false);
        }
    }

    pub fn term_input_to(&self, id: u64, bytes: &[u8]) {
        if let Some(t) = self.term_by_id(id) {
            t.borrow_mut().write(bytes);
        }
    }

    /// Resize every terminal to the pane size.
    pub fn resize_terminal(&self, rows: usize, cols: usize) {
        self.term.sessions.with_untracked(|ts| {
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

    pub fn term_cursor_of(&self, id: Option<u64>) -> Option<(usize, usize)> {
        id.and_then(|i| self.term_by_id(i))
            .and_then(|t| t.borrow().cursor())
    }

    /// Scroll a terminal's scrollback. `up` scrolls into history.
    pub fn term_scroll(&self, id: Option<u64>, up: bool, lines: usize) {
        if let Some(t) = id.and_then(|i| self.term_by_id(i)) {
            if up {
                t.borrow().scroll_up(lines);
            } else {
                t.borrow().scroll_down(lines);
            }
            self.term.tick.update(|x| *x += 1);
        }
    }

    pub fn term_scroll_bottom(&self, id: Option<u64>) {
        if let Some(t) = id.and_then(|i| self.term_by_id(i)) {
            t.borrow().scroll_to_bottom();
        }
    }
}

/// The integrated terminal: its sessions, panes and panel.
///
/// `Copy`, like `AppState`: signals and handles only.
#[derive(Clone, Copy)]
pub struct TerminalPanel {
    /// Integrated terminal session (lazily spawned).
    /// All open terminal sessions, in tab order.
    pub sessions: floem::reactive::RwSignal<Vec<TermSession>>,
    /// Pane 0's active terminal session id.
    pub active: floem::reactive::RwSignal<Option<u64>>,
    /// Pane 1's active terminal (split view).
    pub active2: floem::reactive::RwSignal<Option<u64>>,
    /// Is the terminal split into two panes?
    pub split: floem::reactive::RwSignal<bool>,
    /// Which terminal pane has focus (0 or 1).
    pub focus_pane: floem::reactive::RwSignal<u8>,
    /// Terminal-rename prompt: the session id being renamed, and its input.
    pub rename_id: floem::reactive::RwSignal<Option<u64>>,
    pub rename_input: floem::reactive::RwSignal<String>,
    pub open: floem::reactive::RwSignal<bool>,
    /// Whether the terminal panel currently has keyboard focus.
    pub focused: floem::reactive::RwSignal<bool>,
    /// Bumped whenever the terminal produces output, to trigger a repaint.
    pub tick: floem::reactive::RwSignal<u64>,
    pub rx: floem::reactive::RwSignal<Option<std::sync::mpsc::Receiver<()>>>,
    /// Height of the bottom terminal panel (drag-resizable).
    pub height: floem::reactive::RwSignal<f64>,
}
