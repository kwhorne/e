//! The PATH a GUI launch gets, versus the one the shell has.
//!
//! An app started from the Dock, Finder or `open` inherits launchd's
//! environment, whose PATH is `/usr/bin:/bin:/usr/sbin:/sbin`. php (Grove puts
//! it in `~/.grove/bin`), node, intelephense (`~/.npm-global/bin`), Composer's
//! `vendor/bin` — none of it is there, so every tool reads as "not installed"
//! and Artisan has no commands, while the same app started from a terminal
//! finds them all. Ask the user's login shell for its PATH once at startup and
//! adopt it, the way VS Code and the JetBrains IDEs do.

use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

const MARKER: &str = "__E_PATH__";

/// Set this process's PATH to the login shell's, merged with what it already
/// had. Call first thing in `main`, before any thread exists. `E_NO_SHELL_PATH`
/// skips it.
pub fn adopt_login_path() {
    if std::env::var_os("E_NO_SHELL_PATH").is_some() {
        return;
    }
    let Some(login) = login_shell_path(Duration::from_secs(4)) else {
        return;
    };
    let current = std::env::var("PATH").unwrap_or_default();
    let merged = merge_paths(&login, &current);
    if merged != current {
        std::env::set_var("PATH", &merged);
    }
}

/// `$PATH` as the login shell sets it up. `-ilc`: login *and* interactive, so
/// both the profile (where Homebrew writes) and the rc file (where nvm and
/// friends write) run. `None` when the shell fails or takes longer than
/// `timeout` — a slow rc file must not hold the window back.
pub fn login_shell_path(timeout: Duration) -> Option<String> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    let script = format!("printf '{MARKER}%s{MARKER}' \"$PATH\"");
    let mut child = Command::new(&shell)
        .args(["-ilc", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut s = String::new();
        let _ = std::io::Read::read_to_string(&mut stdout, &mut s);
        let _ = tx.send(s);
    });
    let out = match rx.recv_timeout(timeout) {
        Ok(s) => s,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
    };
    let _ = child.wait();
    extract(&out)
}

/// The PATH between the markers, whatever else the rc files printed.
pub fn extract(out: &str) -> Option<String> {
    let start = out.find(MARKER)? + MARKER.len();
    let end = out[start..].find(MARKER)? + start;
    let path = out[start..end].trim();
    (!path.is_empty()).then(|| path.to_string())
}

/// The login shell's entries first, in its order, then anything the current
/// PATH had that the shell didn't — a terminal's additions survive. No
/// duplicates.
pub fn merge_paths(login: &str, current: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for entry in login.split(':').chain(current.split(':')) {
        if !entry.is_empty() && !out.contains(&entry) {
            out.push(entry);
        }
    }
    out.join(":")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_the_path_from_a_chatty_shell() {
        let out = "Welcome!\nzsh: no job control\n__E_PATH__/a/bin:/b/bin__E_PATH__\nbye\n";
        assert_eq!(extract(out).as_deref(), Some("/a/bin:/b/bin"));
        assert_eq!(extract("nothing here"), None);
        assert_eq!(extract("__E_PATH____E_PATH__"), None);
    }

    #[test]
    fn merge_keeps_the_shell_order_and_the_extras() {
        assert_eq!(
            merge_paths("/opt/homebrew/bin:/usr/bin:/bin", "/usr/bin:/bin:/extra"),
            "/opt/homebrew/bin:/usr/bin:/bin:/extra"
        );
        assert_eq!(merge_paths("", "/usr/bin"), "/usr/bin");
        assert_eq!(merge_paths("/a::/b", "/a"), "/a:/b");
    }

    /// The login shell answers within the timeout and has a PATH.
    #[test]
    fn the_login_shell_reports_a_path() {
        let path = login_shell_path(Duration::from_secs(20)).expect("login shell answers");
        assert!(path.contains("/bin"), "{path}");
    }
}
