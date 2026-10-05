//! The few things done differently per OS: opening a URL, revealing a file,
//! moving one to the trash, and showing a notification.
//!
//! Paths and messages go to helper programs as arguments, never spliced into
//! a script, so quotes and backslashes (every PHP namespace has one) arrive
//! intact. Spawned helpers are waited on from a thread, so they don't linger
//! as zombies.

use std::path::Path;
use std::process::Command;

/// Start `cmd` and reap it in the background.
fn spawn_detached(mut cmd: Command) {
    if let Ok(mut child) = cmd.spawn() {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

/// Open a URL in the default browser (or mail client, for `mailto:`).
pub fn open_url(url: &str) {
    #[cfg(target_os = "macos")]
    let cmd = {
        let mut c = Command::new("open");
        c.arg(url);
        c
    };
    #[cfg(target_os = "windows")]
    let cmd = {
        let mut c = Command::new("cmd");
        c.args(["/C", "start", ""]).arg(url);
        c
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let cmd = {
        let mut c = Command::new("xdg-open");
        c.arg(url);
        c
    };
    spawn_detached(cmd);
}

/// Show `path` in the file manager, selected where the platform can.
pub fn reveal(path: &Path) {
    #[cfg(target_os = "macos")]
    let cmd = {
        let mut c = Command::new("open");
        c.arg("-R").arg(path);
        c
    };
    #[cfg(target_os = "windows")]
    let cmd = {
        let mut c = Command::new("explorer");
        c.arg(format!("/select,{}", path.display()));
        c
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let cmd = {
        let mut c = Command::new("xdg-open");
        c.arg(path.parent().unwrap_or(path));
        c
    };
    spawn_detached(cmd);
}

/// Move `path` to the trash. Blocking — call it off the UI thread.
pub fn trash(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let output = Command::new("osascript")
        .args([
            "-e",
            "on run argv",
            "-e",
            // Coerced outside the `tell`: Finder can't resolve `POSIX file`.
            "set f to (POSIX file (item 1 of argv)) as alias",
            "-e",
            "tell application \"Finder\" to delete f",
            "-e",
            "end run",
        ])
        .arg(path)
        .output();
    #[cfg(not(target_os = "macos"))]
    let output = Command::new("gio").arg("trash").arg(path).output();
    match output {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => Err(String::from_utf8_lossy(&o.stderr).trim().to_string()),
        Err(e) => Err(e.to_string()),
    }
}

/// Show a desktop notification: macOS through `osascript`, elsewhere through
/// `notify-send`.
pub fn notify(message: &str) {
    #[cfg(target_os = "macos")]
    {
        let mut c = Command::new("osascript");
        c.args([
            "-e",
            "on run argv",
            "-e",
            "display notification (item 1 of argv) with title \"e\"",
            "-e",
            "end run",
        ])
        .arg(message);
        spawn_detached(c);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let mut c = Command::new("notify-send");
        c.arg("e").arg(message);
        spawn_detached(c);
    }
}
