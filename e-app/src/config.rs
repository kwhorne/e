//! Global (cross-workspace) editor configuration in `~/.config/e/config.json`.

use std::path::PathBuf;

use serde_json::{json, Value};

/// User settings, loaded once at startup.
#[derive(Clone, Copy)]
pub struct Settings {
    pub dark: bool,
    pub font_size: usize,
    pub tab_width: usize,
    pub format_on_save: bool,
    pub autosave: bool,
    pub indent_guides: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            dark: true,
            font_size: 14,
            tab_width: 4,
            format_on_save: true,
            autosave: true,
            indent_guides: true,
        }
    }
}

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

pub fn load_settings() -> Settings {
    let v = read();
    let d = Settings::default();
    let bool_of = |k: &str, def: bool| v.get(k).and_then(|x| x.as_bool()).unwrap_or(def);
    let usize_of =
        |k: &str, def: usize| v.get(k).and_then(|x| x.as_u64()).map(|n| n as usize).unwrap_or(def);
    Settings {
        dark: bool_of("dark", d.dark),
        font_size: usize_of("font_size", d.font_size).clamp(8, 40),
        tab_width: usize_of("tab_width", d.tab_width).clamp(1, 16),
        format_on_save: bool_of("format_on_save", d.format_on_save),
        autosave: bool_of("autosave", d.autosave),
        indent_guides: bool_of("indent_guides", d.indent_guides),
    }
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
