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
        self.terminals
            .with_untracked(|ts| ts.iter().find(|t| t.id == id).map(|t| t.term.clone()))
    }

    /// The active-terminal signal of the given pane (0 or 1).
    pub(crate) fn pane_active(&self, pane: u8) -> RwSignal<Option<u64>> {
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

    pub(crate) fn spawn_terminal(&self) -> Option<u64> {
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
            self.term_tick.update(|x| *x += 1);
        }
    }

    pub fn term_scroll_bottom(&self, id: Option<u64>) {
        if let Some(t) = id.and_then(|i| self.term_by_id(i)) {
            t.borrow().scroll_to_bottom();
        }
    }
}
