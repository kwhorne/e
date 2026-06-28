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
    pub trim_on_save: bool,
    pub autosave: bool,
    pub indent_guides: bool,
    pub auto_close: bool,
    pub inlay_hints: bool,
    pub sticky_scroll: bool,
    /// Explorer/Git sidebar on the right instead of the left.
    pub sidebar_right: bool,
    /// Agent panel on the left instead of the right.
    pub agent_left: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            dark: true,
            font_size: 14,
            tab_width: 4,
            format_on_save: true,
            trim_on_save: true,
            autosave: true,
            indent_guides: true,
            auto_close: true,
            inlay_hints: true,
            sticky_scroll: true,
            sidebar_right: false,
            agent_left: false,
        }
    }
}

fn config_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(
        PathBuf::from(home)
            .join(".config")
            .join("e")
            .join("config.json"),
    )
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
    let usize_of = |k: &str, def: usize| {
        v.get(k)
            .and_then(|x| x.as_u64())
            .map(|n| n as usize)
            .unwrap_or(def)
    };
    Settings {
        dark: bool_of("dark", d.dark),
        font_size: usize_of("font_size", d.font_size).clamp(8, 40),
        tab_width: usize_of("tab_width", d.tab_width).clamp(1, 16),
        format_on_save: bool_of("format_on_save", d.format_on_save),
        trim_on_save: bool_of("trim_on_save", d.trim_on_save),
        autosave: bool_of("autosave", d.autosave),
        indent_guides: bool_of("indent_guides", d.indent_guides),
        auto_close: bool_of("auto_close", d.auto_close),
        inlay_hints: bool_of("inlay_hints", d.inlay_hints),
        sticky_scroll: bool_of("sticky_scroll", d.sticky_scroll),
        sidebar_right: v.get("sidebar_side").and_then(|x| x.as_str()) == Some("right"),
        agent_left: v.get("agent_side").and_then(|x| x.as_str()) == Some("left"),
    }
}

/// One configurable coding agent that can run in the right-hand panel.
#[derive(Clone, Debug)]
pub struct AgentConfig {
    /// Stable identifier used as the default-agent key.
    pub id: String,
    /// Display name shown in the panel header / settings.
    pub name: String,
    /// Command line, run through the login shell (`$SHELL -lc "<command>"`).
    pub command: String,
    /// Working directory. Empty → the current workspace root.
    pub cwd: String,
}

/// Built-in agents, used when `config.json` has no `agents` section.
pub fn default_agents() -> Vec<AgentConfig> {
    vec![
        AgentConfig {
            id: "elyra".into(),
            name: "Elyra".into(),
            command: "elyra".into(),
            cwd: String::new(),
        },
        AgentConfig {
            id: "claude".into(),
            name: "Claude Code".into(),
            command: "claude".into(),
            cwd: String::new(),
        },
        AgentConfig {
            id: "codex".into(),
            name: "Codex".into(),
            command: "codex".into(),
            cwd: String::new(),
        },
    ]
}

/// The configured agents (built-ins overridable via `agents.list`).
pub fn load_agents() -> Vec<AgentConfig> {
    let v = read();
    let Some(list) = v
        .get("agents")
        .and_then(|a| a.get("list"))
        .and_then(|l| l.as_array())
    else {
        return default_agents();
    };
    let agents: Vec<AgentConfig> = list
        .iter()
        .filter_map(|item| {
            let id = item.get("id")?.as_str()?.to_string();
            let command = item.get("command")?.as_str()?.to_string();
            let name = item
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or(&id)
                .to_string();
            let cwd = item
                .get("cwd")
                .and_then(|c| c.as_str())
                .unwrap_or("")
                .to_string();
            Some(AgentConfig {
                id,
                name,
                command,
                cwd,
            })
        })
        .collect();
    if agents.is_empty() {
        default_agents()
    } else {
        agents
    }
}

/// The id of the default agent (defaults to `elyra`).
pub fn load_default_agent() -> String {
    read()
        .get("agents")
        .and_then(|a| a.get("default"))
        .and_then(|d| d.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "elyra".to_string())
}

/// Persist the chosen default agent into `config.json`.
pub fn save_default_agent(id: &str) {
    let Some(path) = config_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut value = read();
    if !value.get("agents").map(|a| a.is_object()).unwrap_or(false) {
        value["agents"] = json!({});
    }
    value["agents"]["default"] = json!(id);
    // Seed the list with the built-ins on first write so users can edit it.
    if value["agents"].get("list").is_none() {
        let list: Vec<Value> = default_agents()
            .into_iter()
            .map(|a| json!({ "id": a.id, "name": a.name, "command": a.command, "cwd": a.cwd }))
            .collect();
        value["agents"]["list"] = json!(list);
    }
    if let Ok(text) = serde_json::to_string_pretty(&value) {
        let _ = std::fs::write(path, text);
    }
}

/// Path to the global settings file (`~/.config/e/config.json`).
pub fn settings_path() -> Option<PathBuf> {
    config_path()
}

/// Merge a single key/value into `config.json`, preserving everything else.
pub fn set_value(key: &str, value: Value) {
    let Some(path) = config_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut v = read();
    if !v.is_object() {
        v = json!({});
    }
    v[key] = value;
    if let Ok(text) = serde_json::to_string_pretty(&v) {
        let _ = std::fs::write(path, text);
    }
}

pub fn set_bool(key: &str, b: bool) {
    set_value(key, json!(b));
}
pub fn set_usize(key: &str, n: usize) {
    set_value(key, json!(n));
}
pub fn set_str(key: &str, s: &str) {
    set_value(key, json!(s));
}

/// User-defined snippets, keyed by language id, from the `snippets` section:
/// `"snippets": { "php": [ { "prefix": "dd", "body": "dd($0);" } ] }`.
pub fn load_user_keybindings() -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    let v = read();
    if let Some(obj) = v.get("keybindings").and_then(|s| s.as_object()) {
        for (k, val) in obj {
            if let Some(cmd) = val.as_str() {
                out.insert(k.clone(), cmd.to_string());
            }
        }
    }
    out
}

pub fn load_user_snippets() -> std::collections::HashMap<String, Vec<(String, String)>> {
    let mut out = std::collections::HashMap::new();
    let v = read();
    let Some(obj) = v.get("snippets").and_then(|s| s.as_object()) else {
        return out;
    };
    for (lang, arr) in obj {
        let Some(arr) = arr.as_array() else { continue };
        let mut list = Vec::new();
        for item in arr {
            let prefix = item.get("prefix").and_then(|p| p.as_str());
            let body = item.get("body").and_then(|b| b.as_str());
            if let (Some(p), Some(b)) = (prefix, body) {
                list.push((p.to_string(), b.to_string()));
            }
        }
        if !list.is_empty() {
            out.insert(lang.clone(), list);
        }
    }
    out
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
