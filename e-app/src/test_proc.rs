//! Test runs as processes that can be stopped.
//!
//! A test command runs through the login shell, which starts PHP, which may
//! start more (Pest's parallel workers, a browser for Dusk). Each run gets a
//! process group of its own, so **Stop** can end all of it: SIGTERM to the
//! group, then SIGKILL to whatever is still there two seconds later. A run
//! that was stopped says so, and isn't mistaken for a failing suite.

use std::collections::{HashMap, HashSet};
use std::io;
use std::process::{Command, Output, Stdio};
use std::sync::Mutex;
use std::time::Duration;

/// Runs in flight: id → process group.
static RUNNING: Mutex<Option<HashMap<u64, i32>>> = Mutex::new(None);
/// Runs Stop was pressed for.
static STOPPED: Mutex<Option<HashSet<u64>>> = Mutex::new(None);

fn next_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static ID: AtomicU64 = AtomicU64::new(1);
    ID.fetch_add(1, Ordering::Relaxed)
}

/// How a run ended.
pub struct Ended {
    pub output: Output,
    /// Stop was pressed while it ran.
    pub stopped: bool,
}

/// Run `command` through the login shell in `cwd`, in a process group of its
/// own, and wait for it. Blocking: off the UI thread.
pub fn run(command: &str, cwd: &std::path::Path) -> io::Result<Ended> {
    use std::os::unix::process::CommandExt;
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    let child = Command::new(shell)
        .arg("-ilc")
        .arg(command)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()?;
    let id = next_id();
    let pgid = child.id() as i32;
    if let Ok(mut r) = RUNNING.lock() {
        r.get_or_insert_with(HashMap::new).insert(id, pgid);
    }
    let output = child.wait_with_output();
    if let Ok(mut r) = RUNNING.lock() {
        r.get_or_insert_with(HashMap::new).remove(&id);
    }
    let stopped = STOPPED
        .lock()
        .map(|mut s| s.get_or_insert_with(HashSet::new).remove(&id))
        .unwrap_or(false);
    Ok(Ended {
        output: output?,
        stopped,
    })
}

/// Stop every test run in flight. Returns how many there were.
pub fn stop_all() -> usize {
    let running: Vec<(u64, i32)> = RUNNING
        .lock()
        .map(|r| {
            r.as_ref()
                .map(|r| r.iter().map(|(k, v)| (*k, *v)).collect())
        })
        .ok()
        .flatten()
        .unwrap_or_default();
    for &(id, pgid) in &running {
        if let Ok(mut s) = STOPPED.lock() {
            s.get_or_insert_with(HashSet::new).insert(id);
        }
        // SAFETY: kill(2) on a process group we started; a group that is
        // already gone just gets ESRCH.
        unsafe {
            libc::kill(-pgid, libc::SIGTERM);
        }
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(2));
            let alive = RUNNING
                .lock()
                .map(|r| r.as_ref().is_some_and(|r| r.contains_key(&id)))
                .unwrap_or(false);
            if alive {
                // SAFETY: as above.
                unsafe {
                    libc::kill(-pgid, libc::SIGKILL);
                }
            }
        });
    }
    running.len()
}

/// Whether a test run is in flight.
#[cfg(test)]
fn any_running() -> bool {
    RUNNING
        .lock()
        .map(|r| r.as_ref().is_some_and(|r| !r.is_empty()))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// Stop stops every run, so these mustn't overlap.
    static SERIAL: Mutex<()> = Mutex::new(());

    #[test]
    fn stop_ends_the_whole_run_and_says_so() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir();
        let marker = dir.join(format!("e-test-proc-{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        // A child of the shell that would write the marker if it outlived Stop.
        let cmd = format!("(sleep 3; touch {}) & sleep 30", marker.display());
        let started = Instant::now();
        let handle = std::thread::spawn(move || run(&cmd, &std::env::temp_dir()).unwrap());
        while !any_running() {
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(stop_all(), 1);
        let ended = handle.join().unwrap();
        assert!(ended.stopped);
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
        // The backgrounded child went with the group.
        std::thread::sleep(Duration::from_secs(4));
        assert!(!marker.exists(), "a process of the run outlived Stop");
        assert!(!any_running());
    }

    #[test]
    fn a_run_left_alone_is_not_stopped() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let ended = run("echo hi", &std::env::temp_dir()).unwrap();
        assert!(!ended.stopped);
        assert_eq!(String::from_utf8_lossy(&ended.output.stdout).trim(), "hi");
    }
}
