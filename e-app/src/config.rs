//! Global (cross-workspace) editor configuration, e.g. the chosen theme.

use std::path::PathBuf;

use serde_json::{json, Value};

fn config_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".config").join("e").join("config.json"))
}

fn read() -> Value {
    config_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| json!({}))
}

/// Whether dark mode is enabled (defaults to true).
pub fn load_dark() -> bool {
    read().get("dark").and_then(|v| v.as_bool()).unwrap_or(true)
}

pub fn save_dark(dark: bool) {
    let Some(path) = config_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut value = read();
    value["dark"] = json!(dark);
    if let Ok(text) = serde_json::to_string_pretty(&value) {
        let _ = std::fs::write(path, text);
    }
}
