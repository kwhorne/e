//! Per-workspace session persistence: remember which files were open, the
//! active tabs and the split state, and restore them next launch.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

#[derive(Default, Debug)]
pub struct SessionData {
    pub open: Vec<String>,
    /// Contents of unsaved "untitled" scratch buffers, so they survive a quit.
    pub untitled: Vec<String>,
    /// Unsaved edits to files on disk ("hot exit"), so quitting — or a crash —
    /// never loses them: restored over the file's contents next launch.
    pub modified: Vec<ModifiedFile>,
    pub active: Option<String>,
    pub active2: Option<String>,
    pub split: bool,
}

/// A file-backed buffer's unsaved text, and the file's modification time when
/// the buffer last agreed with the disk.
#[derive(Debug, Clone, PartialEq)]
pub struct ModifiedFile {
    pub path: String,
    pub text: String,
    /// Milliseconds since the epoch; `None` when the file didn't exist yet.
    pub disk_mtime_ms: Option<u64>,
}

/// A modification time as milliseconds since the epoch, the form the session
/// file stores it in.
pub fn mtime_ms(t: std::time::SystemTime) -> Option<u64> {
    t.duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as u64)
}

fn sessions_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(
        PathBuf::from(home)
            .join(".config")
            .join("e")
            .join("sessions"),
    )
}

fn session_path(root: &Path) -> Option<PathBuf> {
    let mut hasher = DefaultHasher::new();
    root.to_string_lossy().hash(&mut hasher);
    let name = format!("{:016x}.json", hasher.finish());
    Some(sessions_dir()?.join(name))
}

pub fn load(root: &Path) -> Option<SessionData> {
    load_from(&session_path(root)?)
}

fn load_from(path: &Path) -> Option<SessionData> {
    let text = std::fs::read_to_string(path).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    let strings = |key: &str| -> Vec<String> {
        v.get(key)
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    };
    Some(SessionData {
        open: strings("open"),
        untitled: strings("untitled"),
        modified: v
            .get("modified")
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|m| {
                        Some(ModifiedFile {
                            path: m.get("path")?.as_str()?.to_string(),
                            text: m.get("text")?.as_str()?.to_string(),
                            disk_mtime_ms: m.get("disk_mtime_ms").and_then(|t| t.as_u64()),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default(),
        active: v.get("active").and_then(|s| s.as_str()).map(String::from),
        active2: v.get("active2").and_then(|s| s.as_str()).map(String::from),
        split: v.get("split").and_then(|b| b.as_bool()).unwrap_or(false),
    })
}

/// Order of session writes: a background write that finishes after a newer
/// one must not put the older session back.
static NEXT_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
static LAST_WRITTEN: std::sync::Mutex<u64> = std::sync::Mutex::new(0);

pub fn save(root: &Path, data: &SessionData) {
    let seq = NEXT_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    write_in_order(seq, root, data);
}

/// [`save`] off the calling thread — for the periodic backup of unsaved text,
/// which can be large and is written while the user types.
pub fn save_in_background(root: PathBuf, data: SessionData) {
    let seq = NEXT_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::thread::spawn(move || write_in_order(seq, &root, &data));
}

fn write_in_order(seq: u64, root: &Path, data: &SessionData) {
    let Some(path) = session_path(root) else {
        return;
    };
    let mut last = LAST_WRITTEN.lock().unwrap_or_else(|e| e.into_inner());
    if seq < *last {
        return;
    }
    save_to(&path, data);
    *last = seq;
}

fn save_to(path: &Path, data: &SessionData) {
    let value = json!({
        "open": data.open,
        "untitled": data.untitled,
        "modified": data
            .modified
            .iter()
            .map(|m| json!({
                "path": m.path,
                "text": m.text,
                "disk_mtime_ms": m.disk_mtime_ms,
            }))
            .collect::<Vec<_>>(),
        "active": data.active,
        "active2": data.active2,
        "split": data.split,
    });
    // Compact: it can now carry whole files' unsaved text.
    if let Ok(text) = serde_json::to_string(&value) {
        let _ = e_core::buffer::write(path, &text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsaved_edits_survive_a_save_and_load() {
        let path = std::env::temp_dir()
            .join(format!("e-session-{}", std::process::id()))
            .join("s.json");
        let data = SessionData {
            open: vec!["/p/app/User.php".into()],
            untitled: vec!["scratch".into()],
            modified: vec![ModifiedFile {
                path: "/p/app/User.php".into(),
                text: "<?php // unsaved \"quoted\"\n".into(),
                disk_mtime_ms: Some(1_759_000_000_123),
            }],
            active: Some("/p/app/User.php".into()),
            active2: None,
            split: false,
        };
        save_to(&path, &data);
        let back = load_from(&path).unwrap();
        assert_eq!(back.modified, data.modified);
        assert_eq!(back.untitled, data.untitled);
        assert_eq!(back.open, data.open);
    }

    #[test]
    fn an_older_session_without_modified_files_still_loads() {
        let path = std::env::temp_dir().join(format!("e-session-old-{}.json", std::process::id()));
        std::fs::write(&path, r#"{"open":["/a.php"],"untitled":[],"split":false}"#).unwrap();
        let back = load_from(&path).unwrap();
        assert_eq!(back.open, vec!["/a.php".to_string()]);
        assert!(back.modified.is_empty());
    }
}
