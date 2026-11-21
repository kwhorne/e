//! `e` as the editor other programs wait on: `git commit`, `git rebase -i`,
//! `crontab -e` run in e's terminal open their file in a tab here, and carry
//! on once it is closed. `e --wait <file>` asks over the editor socket
//! (`edit`, then `wait_closed` until the tab is gone); the terminal hands it
//! out as `GIT_EDITOR`, `EDITOR` and `VISUAL`.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use floem::reactive::SignalWith;
use serde_json::{json, Value};

use crate::state::AppState;

/// How often a wait looks for the tab.
const POLL: Duration = Duration::from_millis(200);

/// The environment that makes `e --wait` the editor of what runs in e's
/// terminal. Git reads `GIT_EDITOR` before its own `core.editor`; a shell's rc
/// file that sets `EDITOR` or `VISUAL` still has the last word on those.
/// Nothing when `terminal_editor` is off, or where there is no socket to wait
/// on (Windows).
pub fn terminal_env(on: bool) -> Vec<(String, String)> {
    if !on || cfg!(not(unix)) || crate::agent_sync::socket_env().is_empty() {
        return Vec::new();
    }
    let Ok(exe) = crate::platform::current_exe() else {
        return Vec::new();
    };
    let command = format!(
        "{} --wait",
        crate::cascade::shell_quote(&exe.to_string_lossy())
    );
    ["GIT_EDITOR", "EDITOR", "VISUAL"]
        .iter()
        .map(|k| (k.to_string(), command.clone()))
        .collect()
}

fn is_open(state: AppState, path: &Path) -> bool {
    state
        .editor
        .buffers
        .with_untracked(|bs| bs.iter().any(|b| b.file.path.as_deref() == Some(path)))
}

fn absolute(path: &str) -> PathBuf {
    let p = PathBuf::from(path);
    p.canonicalize().unwrap_or(p)
}

impl AppState {
    /// `edit`: open `path` for the person at the keyboard, at `line`, with
    /// the caret in it. Unlike `open`, no agent marker: nobody else is
    /// looking.
    pub(crate) fn socket_edit(&self, path: &str, line: usize) -> Value {
        let abs = absolute(path);
        if !abs.is_file() {
            return json!({"ok": false, "error": format!("{} is not a file", abs.display())});
        }
        self.jump_to(&format!("file://{}", abs.display()), line.max(1), 1);
        if !is_open(*self, &abs) {
            // Refused (a binary file, one too large): said in the window.
            return json!({"ok": false, "error": format!("e didn't open {}", abs.display())});
        }
        if let Some(id) = self.focused_active_id() {
            self.focus_buffer(id);
        }
        json!({"ok": true, "path": abs.to_string_lossy()})
    }

    /// `wait_closed`: answer once `path` has no tab, or at `timeout` with
    /// `closed: false` (the caller asks again: a connection's answer has to
    /// come within five minutes).
    pub(crate) fn socket_wait_closed(&self, path: &str, timeout: Duration, reply: Sender<Value>) {
        let abs = absolute(path);
        let until = Instant::now() + timeout;
        poll(*self, abs, until, reply);
    }
}

fn poll(state: AppState, path: PathBuf, until: Instant, reply: Sender<Value>) {
    if !is_open(state, &path) {
        let _ = reply.send(json!({"ok": true, "closed": true}));
        return;
    }
    if Instant::now() >= until {
        let _ = reply.send(json!({"ok": true, "closed": false}));
        return;
    }
    floem::action::exec_after(POLL, move |_| poll(state, path, until, reply));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_terminal_is_told_nothing_when_it_is_off() {
        assert!(terminal_env(false).is_empty());
    }
}
