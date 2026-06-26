//! Laravel-aware completion, inspired by the official Laravel VS Code
//! extension. When the cursor sits inside a `route()`, `view()`, `config()`
//! or `env()` string argument, we offer project-specific completions gathered
//! by running `php artisan` and scanning the project.

use std::path::Path;
use std::process::Command;

use lsp_types::{CompletionItem, CompletionItemKind};

/// Project data scraped once on startup.
#[derive(Default)]
pub struct LaravelData {
    pub routes: Vec<String>,
    pub views: Vec<String>,
    pub configs: Vec<String>,
    pub envs: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Helper {
    Route,
    View,
    Config,
    Env,
}

/// Is `root` a Laravel project?
pub fn is_laravel(root: &Path) -> bool {
    root.join("artisan").is_file()
}

/// Gather routes, views, configs and env keys. Blocking — run off the UI thread.
pub fn load(root: &Path) -> LaravelData {
    LaravelData {
        routes: load_routes(root),
        views: load_views(root),
        configs: load_configs(root),
        envs: load_envs(root),
    }
}

fn load_routes(root: &Path) -> Vec<String> {
    let output = Command::new("php")
        .args(["artisan", "route:list", "--json"])
        .current_dir(root)
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&output.stdout) else {
        return Vec::new();
    };
    let mut names: Vec<String> = value
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|r| r.get("name").and_then(|n| n.as_str()))
                .filter(|n| !n.is_empty())
                .map(|n| n.to_string())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names.dedup();
    names
}

fn load_views(root: &Path) -> Vec<String> {
    let base = root.join("resources").join("views");
    let mut out = Vec::new();
    collect_views(&base, &base, &mut out);
    out.sort();
    out.dedup();
    out
}

fn collect_views(base: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_dir() {
            collect_views(base, &path, out);
        } else if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if let Some(stem) = name.strip_suffix(".blade.php") {
                if let Ok(rel) = path.parent().unwrap_or(dir).strip_prefix(base) {
                    let mut parts: Vec<String> = rel
                        .components()
                        .map(|c| c.as_os_str().to_string_lossy().into_owned())
                        .filter(|s| !s.is_empty())
                        .collect();
                    parts.push(stem.to_string());
                    out.push(parts.join("."));
                }
            }
        }
    }
}

fn load_configs(root: &Path) -> Vec<String> {
    let dir = root.join("config");
    let Ok(read) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in read.filter_map(|e| e.ok()) {
        let path = entry.path();
        let Some(file) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if path.extension().and_then(|e| e.to_str()) != Some("php") {
            continue;
        }
        out.push(file.to_string());
        // Best-effort: first-level keys `'key' => ...` in the file.
        if let Ok(src) = std::fs::read_to_string(&path) {
            for key in top_level_keys(&src) {
                out.push(format!("{file}.{key}"));
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Extract `'key'` / `"key"` that appear directly before `=>`.
fn top_level_keys(src: &str) -> Vec<String> {
    let mut keys = Vec::new();
    for (i, _) in src.match_indices("=>") {
        let before = src[..i].trim_end();
        let Some(quote) = before.chars().last() else {
            continue;
        };
        if quote != '\'' && quote != '"' {
            continue;
        }
        let inner = &before[..before.len() - 1];
        if let Some(start) = inner.rfind(quote) {
            let key = &inner[start + 1..];
            if !key.is_empty() && key.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') {
                keys.push(key.to_string());
            }
        }
    }
    keys.sort();
    keys.dedup();
    keys.truncate(40);
    keys
}

fn load_envs(root: &Path) -> Vec<String> {
    let Ok(src) = std::fs::read_to_string(root.join(".env")) else {
        return Vec::new();
    };
    let mut out: Vec<String> = src
        .lines()
        .filter_map(|l| {
            let l = l.trim();
            if l.is_empty() || l.starts_with('#') {
                return None;
            }
            l.split('=').next().map(|k| k.trim().to_string())
        })
        .filter(|k| !k.is_empty())
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Detect a Laravel helper context from the line text before the cursor.
/// Returns the helper and the already-typed prefix inside the string.
pub fn detect_context(line_before_cursor: &str) -> Option<(Helper, String)> {
    let quote = line_before_cursor
        .rfind(['\'', '"'])
        .map(|i| (i, line_before_cursor.as_bytes()[i] as char))?;
    let (qpos, qchar) = quote;
    let prefix = &line_before_cursor[qpos + 1..];
    // The string must still be open (no closing quote after the prefix).
    if prefix.contains(qchar) {
        return None;
    }

    let before = line_before_cursor[..qpos].trim_end();
    let before = before.strip_suffix('(')?.trim_end();

    for (helper, name) in [
        (Helper::Route, "route"),
        (Helper::View, "view"),
        (Helper::Config, "config"),
        (Helper::Env, "env"),
    ] {
        if let Some(idx) = before.len().checked_sub(name.len()) {
            if before[idx..].eq_ignore_ascii_case(name) {
                let boundary = idx == 0 || {
                    let b = before.as_bytes()[idx - 1];
                    !(b.is_ascii_alphanumeric() || b == b'_')
                };
                if boundary {
                    return Some((helper, prefix.to_string()));
                }
            }
        }
    }
    None
}

/// Build completion items for a helper + prefix.
pub fn completions(data: &LaravelData, helper: Helper, prefix: &str) -> Vec<CompletionItem> {
    let source = match helper {
        Helper::Route => &data.routes,
        Helper::View => &data.views,
        Helper::Config => &data.configs,
        Helper::Env => &data.envs,
    };
    let lower = prefix.to_lowercase();
    source
        .iter()
        .filter(|name| lower.is_empty() || name.to_lowercase().contains(&lower))
        .take(100)
        .map(|name| CompletionItem {
            label: name.clone(),
            insert_text: Some(name.clone()),
            kind: Some(CompletionItemKind::VALUE),
            detail: Some("Laravel".to_string()),
            ..Default::default()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_helpers() {
        assert_eq!(detect_context("$x = view('dash"), Some((Helper::View, "dash".into())));
        assert_eq!(detect_context("return route('users.in"), Some((Helper::Route, "users.in".into())));
        assert_eq!(detect_context("config(\"app.na"), Some((Helper::Config, "app.na".into())));
        assert_eq!(detect_context("env('APP_"), Some((Helper::Env, "APP_".into())));
        // not a helper / closed string
        assert_eq!(detect_context("$y = preview('x"), None);
        assert_eq!(detect_context("view('done')"), None);
        assert_eq!(detect_context("$z = 1 + 2"), None);
    }

    #[test]
    fn collects_views_dotted() {
        let dir = std::env::temp_dir().join("e_laravel_views_test");
        let views = dir.join("resources").join("views");
        std::fs::create_dir_all(views.join("admin")).unwrap();
        std::fs::write(views.join("dashboard.blade.php"), "x").unwrap();
        std::fs::write(views.join("admin").join("users.blade.php"), "x").unwrap();
        let mut got = load_views(&dir);
        got.sort();
        assert_eq!(got, vec!["admin.users".to_string(), "dashboard".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
