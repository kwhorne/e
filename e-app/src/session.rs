//! Per-workspace session persistence: remember which files were open, the
//! active tabs and the split state, and restore them next launch.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

#[derive(Default, Debug)]
pub struct SessionData {
    pub open: Vec<String>,
    pub active: Option<String>,
    pub active2: Option<String>,
    pub split: bool,
}

fn sessions_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".config").join("e").join("sessions"))
}

fn session_path(root: &Path) -> Option<PathBuf> {
    let mut hasher = DefaultHasher::new();
    root.to_string_lossy().hash(&mut hasher);
    let name = format!("{:016x}.json", hasher.finish());
    Some(sessions_dir()?.join(name))
}

pub fn load(root: &Path) -> Option<SessionData> {
    let path = session_path(root)?;
    let text = std::fs::read_to_string(path).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    let strings = |key: &str| -> Vec<String> {
        v.get(key)
            .and_then(|x| x.as_array())
            .map(|a| a.iter().filter_map(|s| s.as_str().map(String::from)).collect())
            .unwrap_or_default()
    };
    Some(SessionData {
        open: strings("open"),
        active: v.get("active").and_then(|s| s.as_str()).map(String::from),
        active2: v.get("active2").and_then(|s| s.as_str()).map(String::from),
        split: v.get("split").and_then(|b| b.as_bool()).unwrap_or(false),
    })
}

pub fn save(root: &Path, data: &SessionData) {
    let Some(path) = session_path(root) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let value = json!({
        "open": data.open,
        "active": data.active,
        "active2": data.active2,
        "split": data.split,
    });
    if let Ok(text) = serde_json::to_string_pretty(&value) {
        let _ = std::fs::write(path, text);
    }
}
