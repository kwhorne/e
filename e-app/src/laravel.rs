//! Laravel-aware language features, inspired by the official Laravel VS Code
//! extension (`laravel.vscode-laravel`).
//!
//! When the cursor sits inside a Laravel helper — `route()`, `view()`,
//! `config()`, `env()`, `__()`/`trans()`, or a `<x-...>` Blade component — we
//! offer project-specific completions, hover with the resolved value, and
//! go-to-definition that jumps to the controller / blade / config / lang file.
//!
//! The project is introspected once (and on demand) by running `php artisan`
//! and scanning the filesystem, mirroring how the extension gathers its data.

use std::path::{Path, PathBuf};
use std::process::Command;

use lsp_types::{CompletionItem, CompletionItemKind};

#[derive(Clone, Debug)]
pub struct RouteInfo {
    pub name: String,
    pub uri: String,
    pub methods: String,
    pub action: String,
    /// Comma-separated middleware stack (e.g. `web,auth,throttle:api`).
    pub middleware: String,
}

impl RouteInfo {
    /// A route that mutates state (anything but GET/HEAD/OPTIONS).
    pub fn is_write(&self) -> bool {
        self.methods
            .split('|')
            .map(|m| m.trim().to_uppercase())
            .any(|m| matches!(m.as_str(), "POST" | "PUT" | "PATCH" | "DELETE"))
    }

    /// Whether the middleware stack enforces authentication.
    pub fn has_auth(&self) -> bool {
        let m = self.middleware.to_lowercase();
        m.contains("auth") || m.contains("can:") || m.contains("verified")
    }

    /// A write route with no authentication is the headline risk.
    pub fn is_unprotected(&self) -> bool {
        self.is_write() && !self.has_auth()
    }
}

#[derive(Clone, Debug)]
pub struct ViewInfo {
    pub name: String,
    pub path: PathBuf,
}

#[derive(Clone, Debug)]
pub struct KeyValue {
    pub key: String,
    pub value: String,
}

#[derive(Clone, Debug)]
pub struct TransEntry {
    pub key: String,
    pub value: String,
    pub file: PathBuf,
}

/// Project data scraped on startup and refreshed on demand.
#[derive(Default)]
pub struct LaravelData {
    pub root: PathBuf,
    pub routes: Vec<RouteInfo>,
    pub views: Vec<ViewInfo>,
    pub configs: Vec<KeyValue>,
    pub envs: Vec<KeyValue>,
    pub translations: Vec<TransEntry>,
    pub components: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Helper {
    Route,
    View,
    Config,
    Env,
    Trans,
    Component,
}

/// Is `root` a Laravel project?
pub fn is_laravel(root: &Path) -> bool {
    root.join("artisan").is_file()
}

/// Gather everything. Blocking — run off the UI thread.
pub fn load(root: &Path) -> LaravelData {
    LaravelData {
        root: root.to_path_buf(),
        routes: load_routes(root),
        views: load_views(root),
        configs: load_configs(root),
        envs: load_envs(root),
        translations: load_translations(root),
        components: load_components(root),
    }
}

// ---- Routes ---------------------------------------------------------------

/// PHP flags that silence deprecation/warning noise (PHP 8.x writes these to
/// stdout in CLI, which would otherwise corrupt the JSON output).
const PHP_QUIET: [&str; 4] = ["-d", "error_reporting=0", "-d", "display_errors=0"];

/// Parse the first JSON value embedded in `bytes`, tolerating leading noise.
fn parse_json(bytes: &[u8]) -> Option<serde_json::Value> {
    let s = String::from_utf8_lossy(bytes);
    let start = s.find(['[', '{'])?;
    serde_json::from_str(&s[start..]).ok()
}

/// `route:list --json` renders middleware as either an array or a newline-joined
/// string, depending on the Laravel version. Normalise both to `a,b,c`.
fn parse_middleware(v: Option<&serde_json::Value>) -> String {
    match v {
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .filter_map(|x| x.as_str())
            .collect::<Vec<_>>()
            .join(","),
        Some(serde_json::Value::String(s)) => s
            .split([',', '\n'])
            .map(|p| p.trim())
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>()
            .join(","),
        _ => String::new(),
    }
}

fn load_routes(root: &Path) -> Vec<RouteInfo> {
    let output = Command::new("php")
        .args(PHP_QUIET)
        .args(["artisan", "route:list", "--json"])
        .current_dir(root)
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    let Some(value) = parse_json(&output.stdout) else {
        return Vec::new();
    };
    let mut routes: Vec<RouteInfo> = value
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|r| {
                    let name = r.get("name").and_then(|n| n.as_str()).unwrap_or("");
                    if name.is_empty() {
                        return None;
                    }
                    Some(RouteInfo {
                        name: name.to_string(),
                        uri: r
                            .get("uri")
                            .and_then(|u| u.as_str())
                            .unwrap_or("")
                            .to_string(),
                        methods: r
                            .get("method")
                            .and_then(|m| m.as_str())
                            .unwrap_or("")
                            .to_string(),
                        action: r
                            .get("action")
                            .and_then(|a| a.as_str())
                            .unwrap_or("")
                            .to_string(),
                        middleware: parse_middleware(r.get("middleware")),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    routes.sort_by(|a, b| a.name.cmp(&b.name));
    routes.dedup_by(|a, b| a.name == b.name);
    routes
}

// ---- Views ----------------------------------------------------------------

fn load_views(root: &Path) -> Vec<ViewInfo> {
    let base = root.join("resources").join("views");
    let mut out = Vec::new();
    collect_views(&base, &base, &mut out);
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out.dedup_by(|a, b| a.name == b.name);
    out
}

fn collect_views(base: &Path, dir: &Path, out: &mut Vec<ViewInfo>) {
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
                    out.push(ViewInfo {
                        name: parts.join("."),
                        path: path.clone(),
                    });
                }
            }
        }
    }
}

// ---- Config ---------------------------------------------------------------

fn load_configs(root: &Path) -> Vec<KeyValue> {
    // Best path: boot the app and dump the flattened config with values.
    if let Some(list) = php_dump_config(root) {
        if !list.is_empty() {
            return list;
        }
    }
    // Fallback: scan config/*.php for top-level keys (no values).
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
        out.push(KeyValue {
            key: file.to_string(),
            value: String::new(),
        });
        if let Ok(src) = std::fs::read_to_string(&path) {
            for key in top_level_keys(&src) {
                out.push(KeyValue {
                    key: format!("{file}.{key}"),
                    value: String::new(),
                });
            }
        }
    }
    out.sort_by(|a, b| a.key.cmp(&b.key));
    out.dedup_by(|a, b| a.key == b.key);
    out
}

/// Run a tiny PHP program that boots the app and JSON-encodes the dotted config.
fn php_dump_config(root: &Path) -> Option<Vec<KeyValue>> {
    let code = "require 'vendor/autoload.php';\
        $app = require 'bootstrap/app.php';\
        $app->make(Illuminate\\Contracts\\Console\\Kernel::class)->bootstrap();\
        echo json_encode(Illuminate\\Support\\Arr::dot(config()->all()));";
    let output = Command::new("php")
        .args(PHP_QUIET)
        .arg("-r")
        .arg(code)
        .current_dir(root)
        .output()
        .ok()?;
    let value = parse_json(&output.stdout)?;
    let obj = value.as_object()?;
    let mut out: Vec<KeyValue> = obj
        .iter()
        .map(|(k, v)| KeyValue {
            key: k.clone(),
            value: json_scalar(v),
        })
        .collect();
    out.sort_by(|a, b| a.key.cmp(&b.key));
    Some(out)
}

fn json_scalar(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => s.clone(),
        other => {
            let s = other.to_string();
            if s.len() > 80 {
                format!("{}…", &s[..80])
            } else {
                s
            }
        }
    }
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
            if !key.is_empty()
                && key
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
            {
                keys.push(key.to_string());
            }
        }
    }
    keys.sort();
    keys.dedup();
    keys.truncate(40);
    keys
}

// ---- Env ------------------------------------------------------------------

fn load_envs(root: &Path) -> Vec<KeyValue> {
    let Ok(src) = std::fs::read_to_string(root.join(".env")) else {
        return Vec::new();
    };
    let mut out: Vec<KeyValue> = src
        .lines()
        .filter_map(|l| {
            let l = l.trim();
            if l.is_empty() || l.starts_with('#') {
                return None;
            }
            let (k, v) = l.split_once('=')?;
            let k = k.trim();
            if k.is_empty() {
                return None;
            }
            Some(KeyValue {
                key: k.to_string(),
                value: v.trim().trim_matches('"').to_string(),
            })
        })
        .collect();
    out.sort_by(|a, b| a.key.cmp(&b.key));
    out.dedup_by(|a, b| a.key == b.key);
    out
}

// ---- Translations ---------------------------------------------------------

fn load_translations(root: &Path) -> Vec<TransEntry> {
    let mut out = Vec::new();
    // `lang/` (Laravel 9+) or `resources/lang/` (older).
    for base in [root.join("lang"), root.join("resources").join("lang")] {
        if base.is_dir() {
            collect_translations(root, &base, &mut out);
        }
    }
    out.sort_by(|a, b| a.key.cmp(&b.key));
    out.dedup_by(|a, b| a.key == b.key);
    out
}

fn collect_translations(root: &Path, base: &Path, out: &mut Vec<TransEntry>) {
    let Ok(read) = std::fs::read_dir(base) else {
        return;
    };
    for entry in read.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_dir() {
            // A locale directory like `en/`. Its php files contribute
            // `file.key` translation keys.
            let Ok(inner) = std::fs::read_dir(&path) else {
                continue;
            };
            for f in inner.filter_map(|e| e.ok()) {
                let p = f.path();
                if p.extension().and_then(|e| e.to_str()) == Some("php") {
                    let ns = p
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .to_string();
                    if let Some(pairs) = php_dump_array(root, &p) {
                        for (k, v) in pairs {
                            out.push(TransEntry {
                                key: format!("{ns}.{k}"),
                                value: v,
                                file: p.clone(),
                            });
                        }
                    }
                }
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some("json") {
            // A JSON locale file: the keys are the source strings themselves.
            if let Ok(src) = std::fs::read_to_string(&path) {
                if let Ok(serde_json::Value::Object(map)) = serde_json::from_str(&src) {
                    for (k, v) in map {
                        out.push(TransEntry {
                            key: k,
                            value: json_scalar(&v),
                            file: path.clone(),
                        });
                    }
                }
            }
        }
    }
}

/// Dump a PHP file that `return`s an array as flattened `key => value` pairs.
fn php_dump_array(cwd: &Path, file: &Path) -> Option<Vec<(String, String)>> {
    let code = format!(
        "echo json_encode(Illuminate\\Support\\Arr::dot(require '{}'));",
        file.display()
    );
    // Needs the autoloader for Arr::dot.
    let full = format!("require 'vendor/autoload.php';{code}");
    let output = Command::new("php")
        .args(PHP_QUIET)
        .arg("-r")
        .arg(&full)
        .current_dir(cwd)
        .output()
        .ok()?;
    let value = parse_json(&output.stdout)?;
    let obj = value.as_object()?;
    Some(
        obj.iter()
            .map(|(k, v)| (k.clone(), json_scalar(v)))
            .collect(),
    )
}

// ---- Blade components -----------------------------------------------------

fn load_components(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    // Anonymous components: resources/views/components/**/*.blade.php
    let comp_dir = root.join("resources").join("views").join("components");
    collect_component_views(&comp_dir, &comp_dir, &mut out);
    // Class-based components: app/View/Components/**/*.php
    let class_dir = root.join("app").join("View").join("Components");
    collect_component_classes(&class_dir, &class_dir, &mut out);
    out.sort();
    out.dedup();
    out
}

fn collect_component_views(base: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_dir() {
            collect_component_views(base, &path, out);
        } else if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if let Some(stem) = name.strip_suffix(".blade.php") {
                if let Ok(rel) = path.parent().unwrap_or(dir).strip_prefix(base) {
                    let mut parts: Vec<String> = rel
                        .components()
                        .map(|c| c.as_os_str().to_string_lossy().into_owned())
                        .filter(|s| !s.is_empty())
                        .collect();
                    // `index.blade.php` represents the directory component.
                    if stem != "index" {
                        parts.push(stem.to_string());
                    }
                    if !parts.is_empty() {
                        out.push(parts.join("."));
                    }
                }
            }
        }
    }
}

fn collect_component_classes(base: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_dir() {
            collect_component_classes(base, &path, out);
        } else if let Some(stem) = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".php"))
        {
            if let Ok(rel) = path.parent().unwrap_or(dir).strip_prefix(base) {
                let mut parts: Vec<String> = rel
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .filter(|s| !s.is_empty())
                    .map(|s| kebab(&s))
                    .collect();
                parts.push(kebab(stem));
                out.push(parts.join("."));
            }
        }
    }
}

/// `MyButton` -> `my-button`.
fn kebab(s: &str) -> String {
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() {
            if i != 0 {
                out.push('-');
            }
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

// ---- Context detection ----------------------------------------------------

/// Detect a Laravel helper context from the line text before the cursor,
/// returning the helper and the already-typed prefix.
pub fn detect_context(line_before_cursor: &str) -> Option<(Helper, String)> {
    // Blade component: `<x-...` (may contain dots), not yet closed.
    if let Some(idx) = line_before_cursor.rfind("<x-") {
        let after = &line_before_cursor[idx + 3..];
        if !after.contains(['>', ' ', '\t', '/', '"', '\'']) {
            return Some((Helper::Component, after.to_string()));
        }
    }

    let quote = line_before_cursor
        .rfind(['\'', '"'])
        .map(|i| (i, line_before_cursor.as_bytes()[i] as char))?;
    let (qpos, qchar) = quote;
    let prefix = &line_before_cursor[qpos + 1..];
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
        (Helper::Trans, "__"),
        (Helper::Trans, "trans"),
        (Helper::Trans, "trans_choice"),
        (Helper::Trans, "lang"), // @lang(...)
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

/// Detect the full token under the cursor (for hover / go-to-definition).
/// `before` is the line text up to the cursor, `after` the rest of the line.
pub fn token_at(before: &str, after: &str) -> Option<(Helper, String)> {
    // Component `<x-name>` — find the enclosing tag.
    if let Some(idx) = before.rfind("<x-") {
        let head = &before[idx + 3..];
        if !head.contains(['>', ' ', '\t', '/']) {
            let tail: String = after
                .chars()
                .take_while(|c| !matches!(c, '>' | ' ' | '\t' | '/' | '"' | '\''))
                .collect();
            return Some((Helper::Component, format!("{head}{tail}")));
        }
    }
    // String helper — reuse detect_context for the helper, then extend the
    // prefix with the remainder of the string after the cursor.
    let (helper, prefix) = detect_context(before)?;
    if helper == Helper::Component {
        return Some((helper, prefix));
    }
    let qchar = before.chars().rev().find(|c| *c == '\'' || *c == '"')?;
    let tail: String = after.chars().take_while(|c| *c != qchar).collect();
    Some((helper, format!("{prefix}{tail}")))
}

// ---- Completions ----------------------------------------------------------

/// Build completion items for a helper + prefix.
pub fn completions(data: &LaravelData, helper: Helper, prefix: &str) -> Vec<CompletionItem> {
    let lower = prefix.to_lowercase();
    let matches = |name: &str| lower.is_empty() || name.to_lowercase().contains(&lower);

    let item = |label: String, detail: String| CompletionItem {
        label: label.clone(),
        insert_text: Some(label),
        kind: Some(CompletionItemKind::VALUE),
        detail: Some(detail),
        ..Default::default()
    };

    match helper {
        Helper::Route => data
            .routes
            .iter()
            .filter(|r| matches(&r.name))
            .take(100)
            .map(|r| {
                let detail = if r.uri.is_empty() {
                    "route".to_string()
                } else {
                    format!("{} {}", r.methods, r.uri)
                };
                item(r.name.clone(), detail)
            })
            .collect(),
        Helper::View => data
            .views
            .iter()
            .filter(|v| matches(&v.name))
            .take(100)
            .map(|v| item(v.name.clone(), "view".to_string()))
            .collect(),
        Helper::Config => data
            .configs
            .iter()
            .filter(|c| matches(&c.key))
            .take(100)
            .map(|c| {
                let d = if c.value.is_empty() {
                    "config".to_string()
                } else {
                    c.value.clone()
                };
                item(c.key.clone(), d)
            })
            .collect(),
        Helper::Env => data
            .envs
            .iter()
            .filter(|e| matches(&e.key))
            .take(100)
            .map(|e| {
                let d = if e.value.is_empty() {
                    "env".to_string()
                } else {
                    e.value.clone()
                };
                item(e.key.clone(), d)
            })
            .collect(),
        Helper::Trans => data
            .translations
            .iter()
            .filter(|t| matches(&t.key))
            .take(100)
            .map(|t| item(t.key.clone(), t.value.clone()))
            .collect(),
        Helper::Component => data
            .components
            .iter()
            .filter(|c| matches(c))
            .take(100)
            .map(|c| item(c.clone(), "component".to_string()))
            .collect(),
    }
}

// ---- Hover ----------------------------------------------------------------

/// Resolve a hover string for the exact token under the cursor.
pub fn hover_text(data: &LaravelData, helper: Helper, token: &str) -> Option<String> {
    match helper {
        Helper::Route => {
            let r = data.routes.iter().find(|r| r.name == token)?;
            Some(format!(
                "**Route** `{}`\n\n`{} {}`\n\n{}",
                r.name, r.methods, r.uri, r.action
            ))
        }
        Helper::View => {
            let v = data.views.iter().find(|v| v.name == token)?;
            let rel = v.path.strip_prefix(&data.root).unwrap_or(&v.path).display();
            Some(format!("**View** `{}`\n\n{}", v.name, rel))
        }
        Helper::Config => {
            let c = data.configs.iter().find(|c| c.key == token)?;
            Some(format!("**Config** `{}`\n\n`{}`", c.key, c.value))
        }
        Helper::Env => {
            let e = data.envs.iter().find(|e| e.key == token)?;
            Some(format!("**Env** `{}`\n\n`{}`", e.key, e.value))
        }
        Helper::Trans => {
            let t = data.translations.iter().find(|t| t.key == token)?;
            Some(format!("**Translation** `{}`\n\n{}", t.key, t.value))
        }
        Helper::Component => {
            let _ = data.components.iter().find(|c| *c == token)?;
            Some(format!("**Component** `<x-{token}>`"))
        }
    }
}

// ---- Go to definition -----------------------------------------------------

/// Resolve the token under the cursor to a target `(path, line, col)`.
pub fn navigate(
    data: &LaravelData,
    helper: Helper,
    token: &str,
) -> Option<(PathBuf, usize, usize)> {
    match helper {
        Helper::View => {
            let v = data.views.iter().find(|v| v.name == token)?;
            Some((v.path.clone(), 0, 0))
        }
        Helper::Trans => {
            let t = data.translations.iter().find(|t| t.key == token)?;
            Some((t.file.clone(), 0, 0))
        }
        Helper::Config => {
            // `app.name` -> config/app.php, jump to the key line if we can.
            let file = token.split('.').next()?;
            let path = data.root.join("config").join(format!("{file}.php"));
            if !path.is_file() {
                return None;
            }
            let line = token
                .split('.')
                .nth(1)
                .and_then(|key| find_key_line(&path, key))
                .unwrap_or(0);
            Some((path, line, 0))
        }
        Helper::Env => {
            let path = data.root.join(".env");
            let line = find_env_line(&path, token).unwrap_or(0);
            Some((path, line, 0))
        }
        Helper::Route => {
            let r = data.routes.iter().find(|r| r.name == token)?;
            controller_location(&data.root, &r.action)
        }
        Helper::Component => {
            // Prefer the anonymous blade file.
            let rel: PathBuf = token.split('.').collect();
            let blade = data
                .root
                .join("resources/views/components")
                .join(&rel)
                .with_extension("blade.php");
            if blade.is_file() {
                return Some((blade, 0, 0));
            }
            let index = data
                .root
                .join("resources/views/components")
                .join(&rel)
                .join("index.blade.php");
            if index.is_file() {
                return Some((index, 0, 0));
            }
            None
        }
    }
}

/// View names referenced by a route's controller action (best-effort: scans the
/// controller file for `view('…')` and `Inertia::render('…')`). Used by the
/// architecture map.
pub fn route_views(data: &LaravelData, action: &str) -> Vec<String> {
    let Some((path, _, _)) = controller_location(&data.root, action) else {
        return Vec::new();
    };
    let Ok(src) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for needle in ["view(", "Inertia::render(", "inertia("] {
        let mut from = 0;
        while let Some(rel) = src[from..].find(needle) {
            let start = from + rel + needle.len();
            from = start;
            let rest = src[start..].trim_start();
            let Some(q) = rest.chars().next() else {
                continue;
            };
            if q != '\'' && q != '"' {
                continue;
            }
            if let Some(end) = rest[1..].find(q) {
                let name = &rest[1..1 + end];
                if !name.is_empty() && !out.contains(&name.to_string()) {
                    out.push(name.to_string());
                }
            }
        }
    }
    out
}

/// Turn `App\Http\Controllers\UserController@index` into a file + method line.
fn controller_location(root: &Path, action: &str) -> Option<(PathBuf, usize, usize)> {
    let (class, method) = match action.split_once('@') {
        Some((c, m)) => (c, Some(m)),
        None => (action, None),
    };
    if class.is_empty() || class.contains("Closure") {
        return None;
    }
    // Map namespace to a file under app/ (PSR-4: App\ -> app/).
    let rel = class.trim_start_matches('\\').replace('\\', "/");
    let rel = rel.strip_prefix("App/").unwrap_or(&rel);
    let path = root.join("app").join(format!("{rel}.php"));
    if !path.is_file() {
        return None;
    }
    let line = method
        .and_then(|m| find_function_line(&path, m))
        .unwrap_or(0);
    Some((path, line, 0))
}

fn find_key_line(path: &Path, key: &str) -> Option<usize> {
    let src = std::fs::read_to_string(path).ok()?;
    let needle1 = format!("'{key}'");
    let needle2 = format!("\"{key}\"");
    src.lines()
        .position(|l| l.contains(&needle1) || l.contains(&needle2))
}

fn find_env_line(path: &Path, key: &str) -> Option<usize> {
    let src = std::fs::read_to_string(path).ok()?;
    let needle = format!("{key}=");
    src.lines()
        .position(|l| l.trim_start().starts_with(&needle))
}

fn find_function_line(path: &Path, method: &str) -> Option<usize> {
    let src = std::fs::read_to_string(path).ok()?;
    let needle = format!("function {method}(");
    src.lines().position(|l| l.contains(&needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_helpers() {
        assert_eq!(
            detect_context("$x = view('dash"),
            Some((Helper::View, "dash".into()))
        );
        assert_eq!(
            detect_context("return route('users.in"),
            Some((Helper::Route, "users.in".into()))
        );
        assert_eq!(
            detect_context("config(\"app.na"),
            Some((Helper::Config, "app.na".into()))
        );
        assert_eq!(
            detect_context("env('APP_"),
            Some((Helper::Env, "APP_".into()))
        );
        assert_eq!(
            detect_context("{{ __('messages.wel"),
            Some((Helper::Trans, "messages.wel".into()))
        );
        assert_eq!(
            detect_context("    <x-forms.inp"),
            Some((Helper::Component, "forms.inp".into()))
        );
        assert_eq!(detect_context("$y = preview('x"), None);
        assert_eq!(detect_context("view('done')"), None);
    }

    #[test]
    fn token_spans_the_cursor() {
        assert_eq!(
            token_at("route('users.", "index')"),
            Some((Helper::Route, "users.index".into()))
        );
        assert_eq!(
            token_at("<x-forms.inp", "ut>"),
            Some((Helper::Component, "forms.input".into()))
        );
    }

    #[test]
    fn kebabs_names() {
        assert_eq!(kebab("MyButton"), "my-button");
        assert_eq!(kebab("Alert"), "alert");
    }

    #[test]
    fn collects_views_dotted() {
        let dir = std::env::temp_dir().join("e_laravel_views_test");
        let views = dir.join("resources").join("views");
        std::fs::create_dir_all(views.join("admin")).unwrap();
        std::fs::write(views.join("dashboard.blade.php"), "x").unwrap();
        std::fs::write(views.join("admin").join("users.blade.php"), "x").unwrap();
        let mut got: Vec<String> = load_views(&dir).into_iter().map(|v| v.name).collect();
        got.sort();
        assert_eq!(
            got,
            vec!["admin.users".to_string(), "dashboard".to_string()]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
