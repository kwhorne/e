//! Keyboard shortcuts: normalize a key event into a canonical string like
//! `cmd+shift+p`, look it up in the (default + user-overridden) binding table,
//! and return the command id.

use std::collections::HashMap;
use std::sync::OnceLock;

use floem::keyboard::{Key, KeyCode, Modifiers, NamedKey, PhysicalKey};

/// Default bindings: `(key string, command id)`. On macOS `cmd` is ⌘ and `ctrl`
/// is Control. User config can override or remove these.
const DEFAULT: &[(&str, &str)] = &[
    ("cmd+p", "goto-file"),
    ("cmd+shift+p", "command-palette"),
    ("cmd+shift+b", "run-task"),
    ("cmd+shift+a", "artisan"),
    ("cmd+shift+,", "laravel-menu"),
    ("cmd+shift+n", "new-model"),
    ("cmd+shift+r", "route-search"),
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
    ("cmd+alt+s", "agent-send-selection"),
    ("cmd+alt+v", "session-review"),
    ("cmd+shift+t", "run-tests"),
    ("cmd+alt+shift+t", "run-test-at-cursor"),
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

fn lookup(ks: &str) -> Option<String> {
    if let Some(map) = BINDINGS.get() {
        return map.get(ks).cloned();
    }
    // Before startup load: fall back to the defaults.
    DEFAULT
        .iter()
        .find(|(k, _)| normalize_string(k) == ks)
        .map(|(_, v)| v.to_string())
}

/// The command id bound to a key event, if any.
///
/// The logical key is tried first — it is what the user sees the key type, and
/// what every binding has always matched. When that finds nothing and a
/// modifier is held, the physical key's base character is tried: macOS reports
/// ⇧, as `;` (Norwegian) or `<` (US) and ⌥N as a dead key, neither of which
/// any binding names, while the key under the finger is plainly `,` or `n`.
/// Plain typing never reaches the fallback, so text stays text.
pub fn command_for(key: &Key, physical: Option<&PhysicalKey>, mods: Modifiers) -> Option<String> {
    if let Some(cmd) = normalize(key, mods).and_then(|ks| lookup(&ks)) {
        return Some(cmd);
    }
    if !(mods.meta() || mods.control() || mods.alt()) {
        return None;
    }
    let name = physical.and_then(physical_name)?;
    lookup(&build(
        mods.meta(),
        mods.control(),
        mods.alt(),
        mods.shift(),
        name,
    ))
}

/// What a physical key types with no modifiers on a US layout, for the keys a
/// shortcut can name. `None` for anything else (function keys are handled by
/// the logical key; modifiers and the rest can't be bound).
pub fn physical_name(key: &PhysicalKey) -> Option<&'static str> {
    let PhysicalKey::Code(code) = key else {
        return None;
    };
    Some(match code {
        KeyCode::KeyA => "a",
        KeyCode::KeyB => "b",
        KeyCode::KeyC => "c",
        KeyCode::KeyD => "d",
        KeyCode::KeyE => "e",
        KeyCode::KeyF => "f",
        KeyCode::KeyG => "g",
        KeyCode::KeyH => "h",
        KeyCode::KeyI => "i",
        KeyCode::KeyJ => "j",
        KeyCode::KeyK => "k",
        KeyCode::KeyL => "l",
        KeyCode::KeyM => "m",
        KeyCode::KeyN => "n",
        KeyCode::KeyO => "o",
        KeyCode::KeyP => "p",
        KeyCode::KeyQ => "q",
        KeyCode::KeyR => "r",
        KeyCode::KeyS => "s",
        KeyCode::KeyT => "t",
        KeyCode::KeyU => "u",
        KeyCode::KeyV => "v",
        KeyCode::KeyW => "w",
        KeyCode::KeyX => "x",
        KeyCode::KeyY => "y",
        KeyCode::KeyZ => "z",
        KeyCode::Digit0 => "0",
        KeyCode::Digit1 => "1",
        KeyCode::Digit2 => "2",
        KeyCode::Digit3 => "3",
        KeyCode::Digit4 => "4",
        KeyCode::Digit5 => "5",
        KeyCode::Digit6 => "6",
        KeyCode::Digit7 => "7",
        KeyCode::Digit8 => "8",
        KeyCode::Digit9 => "9",
        KeyCode::Comma => ",",
        KeyCode::Period => ".",
        KeyCode::Slash => "/",
        KeyCode::Semicolon => ";",
        KeyCode::Quote => "'",
        KeyCode::BracketLeft => "[",
        KeyCode::BracketRight => "]",
        KeyCode::Backslash => "\\",
        KeyCode::Minus => "-",
        KeyCode::Equal => "=",
        KeyCode::Backquote => "`",
        _ => return None,
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    fn chord(meta: bool, alt: bool, shift: bool) -> Modifiers {
        let mut m = Modifiers::empty();
        if meta {
            m |= Modifiers::META;
        }
        if alt {
            m |= Modifiers::ALT;
        }
        if shift {
            m |= Modifiers::SHIFT;
        }
        m
    }

    #[test]
    fn shifted_punctuation_falls_back_to_the_physical_key() {
        // ⌘⇧, on a Norwegian layout: the logical key is `;`, the key is `,`.
        let got = command_for(
            &Key::Character(";".into()),
            Some(&PhysicalKey::Code(KeyCode::Comma)),
            chord(true, false, true),
        );
        assert_eq!(got.as_deref(), Some("laravel-menu"));
        // US layout reports `<` for the same chord.
        let got = command_for(
            &Key::Character("<".into()),
            Some(&PhysicalKey::Code(KeyCode::Comma)),
            chord(true, false, true),
        );
        assert_eq!(got.as_deref(), Some("laravel-menu"));
    }

    #[test]
    fn dead_keys_fall_back_to_the_physical_key() {
        // ⌥E is a dead key (´) on macOS; ⌘⌥E must still open related files.
        let got = command_for(
            &Key::Dead(Some('´')),
            Some(&PhysicalKey::Code(KeyCode::KeyE)),
            chord(true, true, false),
        );
        assert_eq!(got.as_deref(), Some("related-files"));
        // ⌥Z types Ω on a US layout; the binding is on the key.
        let got = command_for(
            &Key::Character("Ω".into()),
            Some(&PhysicalKey::Code(KeyCode::KeyZ)),
            chord(false, true, false),
        );
        assert_eq!(got.as_deref(), Some("word-wrap"));
    }

    #[test]
    fn the_logical_key_still_wins_and_plain_typing_is_never_a_shortcut() {
        let got = command_for(
            &Key::Character("s".into()),
            Some(&PhysicalKey::Code(KeyCode::KeyS)),
            chord(true, false, false),
        );
        assert_eq!(got.as_deref(), Some("save"));
        // A bare `,` (or `;` on the comma key) is text.
        let got = command_for(
            &Key::Character(";".into()),
            Some(&PhysicalKey::Code(KeyCode::Comma)),
            chord(false, false, true),
        );
        assert_eq!(got, None);
        // A chord with no binding on either key stays unhandled.
        let got = command_for(
            &Key::Character("[".into()),
            Some(&PhysicalKey::Code(KeyCode::Digit8)),
            chord(false, true, false),
        );
        assert_eq!(got, None);
    }
}
