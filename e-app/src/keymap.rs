//! Keyboard shortcuts: normalize a key event into a canonical string like
//! `cmd+shift+p`, look it up in the (default + user-overridden) binding table,
//! and return the command id.

use std::collections::HashMap;
use std::sync::OnceLock;

use floem::keyboard::{Key, Modifiers, NamedKey};

/// Default bindings: `(key string, command id)`. On macOS `cmd` is ⌘ and `ctrl`
/// is Control. User config can override or remove these.
const DEFAULT: &[(&str, &str)] = &[
    ("cmd+p", "goto-file"),
    ("cmd+shift+p", "command-palette"),
    ("cmd+shift+b", "run-task"),
    ("cmd+e", "recent"),
    ("cmd+o", "open-folder"),
    ("cmd+n", "new-file"),
    ("cmd+s", "save"),
    ("cmd+shift+s", "save-as"),
    ("cmd+w", "close"),
    ("cmd+t", "toggle-terminal"),
    ("cmd+l", "toggle-agent"),
    ("cmd+1", "toggle-sidebar"),
    ("cmd+2", "source-control"),
    ("cmd+3", "toggle-database"),
    ("cmd+enter", "run-sql"),
    ("cmd+alt+enter", "explain-sql"),
    ("cmd+alt+t", "tinker"),
    ("cmd+alt+m", "laravel-map"),
    ("cmd+alt+a", "agent-log"),
    ("cmd+shift+t", "run-tests"),
    ("cmd+alt+l", "laravel-log"),
    ("cmd+alt+i", "runtime"),
    ("cmd+alt+u", "undo-tree"),
    ("cmd+alt+r", "relations"),
    ("cmd+alt+g", "event-graph"),
    ("cmd+alt+c", "props-contract"),
    ("cmd+alt+e", "related-files"),
    ("cmd+alt+j", "livewire-companion"),
    ("cmd+alt+k", "semantic-search"),
    ("cmd+\\", "split"),
    ("cmd+shift+o", "symbols"),
    ("cmd+shift+f", "search"),
    ("cmd+alt+f", "replace"),
    ("cmd+f", "find"),
    ("ctrl+g", "goto-line"),
    ("cmd+/", "comment"),
    ("cmd+d", "duplicate-line"),
    ("cmd+shift+d", "select-next-occurrence"),
    ("cmd+shift+l", "select-all-occurrences"),
    ("cmd+alt+up", "add-cursor-above"),
    ("cmd+alt+down", "add-cursor-below"),
    ("cmd+shift+k", "delete-line"),
    ("cmd+]", "indent"),
    ("cmd+[", "outdent"),
    ("cmd+=", "zoom-in"),
    ("cmd+shift+=", "zoom-in"),
    ("cmd+-", "zoom-out"),
    ("cmd+0", "zoom-reset"),
    ("ctrl+-", "nav-back"),
    ("ctrl+shift+-", "nav-forward"),
    ("cmd+shift+m", "markdown"),
    ("cmd+,", "settings"),
    ("cmd+space", "completion"),
    ("alt+z", "word-wrap"),
    ("ctrl+`", "toggle-terminal"),
    ("alt+up", "move-line-up"),
    ("alt+down", "move-line-down"),
    ("alt+shift+down", "duplicate-line"),
    ("f1", "hover"),
    ("f2", "rename"),
    ("cmd+.", "code-actions"),
    ("f8", "theme"),
    ("f5", "debug"),
    ("f9", "debug-toggle-breakpoint"),
    ("f10", "debug-step-over"),
    ("f11", "debug-step-into"),
    ("shift+f11", "debug-step-out"),
    ("f12", "definition"),
    ("shift+f12", "references"),
    ("escape", "close-overlays"),
];

static BINDINGS: OnceLock<HashMap<String, String>> = OnceLock::new();

/// Build the binding table from the defaults plus user overrides. A user value
/// of `"none"`/`""` removes a default binding.
pub fn load(user: HashMap<String, String>) {
    let mut map: HashMap<String, String> = DEFAULT
        .iter()
        .map(|(k, v)| (normalize_string(k), v.to_string()))
        .collect();
    for (k, v) in user {
        let k = normalize_string(&k);
        if v.is_empty() || v == "none" {
            map.remove(&k);
        } else {
            map.insert(k, v);
        }
    }
    let _ = BINDINGS.set(map);
}

/// The command id bound to a key event, if any.
pub fn command_for(key: &Key, mods: Modifiers) -> Option<String> {
    let ks = normalize(key, mods)?;
    if let Some(map) = BINDINGS.get() {
        return map.get(&ks).cloned();
    }
    // Before startup load: fall back to the defaults.
    DEFAULT
        .iter()
        .find(|(k, _)| normalize_string(k) == ks)
        .map(|(_, v)| v.to_string())
}

/// Canonicalize a user-written key string (sort modifiers, lowercase).
fn normalize_string(s: &str) -> String {
    let parts: Vec<&str> = s.split('+').collect();
    if parts.is_empty() {
        return String::new();
    }
    // The last part is the key; the rest are modifiers.
    let (key, mods) = parts.split_last().unwrap();
    let key = key.to_lowercase();
    let has = |m: &str| mods.iter().any(|p| p.eq_ignore_ascii_case(m));
    build(
        has("cmd") || has("meta"),
        has("ctrl") || has("control"),
        has("alt") || has("option"),
        has("shift"),
        &key,
    )
}

/// Build the canonical string for a set of modifiers + key name.
fn build(cmd: bool, ctrl: bool, alt: bool, shift: bool, key: &str) -> String {
    let mut out = String::new();
    if cmd {
        out.push_str("cmd+");
    }
    if ctrl {
        out.push_str("ctrl+");
    }
    if alt {
        out.push_str("alt+");
    }
    if shift {
        out.push_str("shift+");
    }
    out.push_str(key);
    out
}

/// Normalize a live key event into the canonical string, or `None` for keys we
/// don't map.
pub fn normalize(key: &Key, mods: Modifiers) -> Option<String> {
    let key_name = match key {
        Key::Character(s) => {
            let c = s.to_lowercase();
            match c.as_str() {
                " " => "space".to_string(),
                // `+` shares the modifier separator; fold it onto `=` (same key).
                "+" => "=".to_string(),
                other => other.to_string(),
            }
        }
        Key::Named(named) => match named {
            NamedKey::F1 => "f1".into(),
            NamedKey::F2 => "f2".into(),
            NamedKey::F3 => "f3".into(),
            NamedKey::F4 => "f4".into(),
            NamedKey::F5 => "f5".into(),
            NamedKey::F6 => "f6".into(),
            NamedKey::F7 => "f7".into(),
            NamedKey::F8 => "f8".into(),
            NamedKey::F9 => "f9".into(),
            NamedKey::F10 => "f10".into(),
            NamedKey::F11 => "f11".into(),
            NamedKey::F12 => "f12".into(),
            NamedKey::ArrowUp => "up".into(),
            NamedKey::ArrowDown => "down".into(),
            NamedKey::ArrowLeft => "left".into(),
            NamedKey::ArrowRight => "right".into(),
            NamedKey::Space => "space".into(),
            NamedKey::Escape => "escape".into(),
            NamedKey::Enter => "enter".into(),
            NamedKey::Tab => "tab".into(),
            NamedKey::Backspace => "backspace".into(),
            NamedKey::Delete => "delete".into(),
            NamedKey::Home => "home".into(),
            NamedKey::End => "end".into(),
            _ => return None,
        },
        _ => return None,
    };
    // Ignore bare modifier presses.
    if key_name.is_empty() {
        return None;
    }
    Some(build(
        mods.meta(),
        mods.control(),
        mods.alt(),
        mods.shift(),
        &key_name,
    ))
}
