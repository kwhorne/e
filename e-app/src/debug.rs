//! Step-debugging: the editor half of native debugging.
//!
//! Drives a Debug Adapter Protocol session through [`e_dap::DapClient`]. For PHP
//! the adapter is `vscode-php-debug` run over Node (both supplied by Grove), which
//! Xdebug connects out to on DBGp port 9003 once `grove debug on` has loaded it
//! into the FPM pools. The same machinery serves any DAP adapter (JS, Rust, …).
//!
//! DAP is event-heavy, so adapter events are marshalled onto the UI thread via
//! `create_ext_action` and fan out into reactive signals the debug panel renders.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;

use floem::ext_event::{create_ext_action, create_signal_from_channel};
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use floem::views::editor::text::Document;
use serde_json::json;

use e_core::language::Language;
use e_dap::{DapClient, DapEvent};
use e_lsp::path_to_uri;

use crate::state::AppState;

/// How to reach a debug adapter.
enum Transport {
    /// Spawn the adapter and talk DAP over its stdio (PHP).
    Stdio,
    /// Spawn the adapter as a server and connect over TCP (JS, Rust).
    TcpServer { port: u16 },
}

/// A resolved adapter invocation for one language.
struct AdapterSpec {
    transport: Transport,
    program: String,
    args: Vec<String>,
    adapter_id: String,
    launch: serde_json::Value,
}

/// A call-stack frame flattened for display.
#[derive(Clone, Debug, PartialEq)]
pub struct DebugFrame {
    pub name: String,
    pub path: String,
    pub line: i64,
}

/// A variable in the current frame's scopes.
#[derive(Clone, Debug, PartialEq)]
pub struct DebugVar {
    pub name: String,
    pub value: String,
    pub ty: String,
}

impl AppState {
    /// Copy a file's breakpoint lines into its open buffer's gutter marks.
    pub(crate) fn sync_bp_marks(&self, path: &str) {
        let lines: HashSet<usize> = self.debug_breakpoints.with_untracked(|m| {
            m.get(path)
                .map(|v| v.iter().map(|l| (*l as usize).saturating_sub(1)).collect())
                .unwrap_or_default()
        });
        self.buffers.with_untracked(|bs| {
            for b in bs {
                if buffer_matches(b, path) {
                    *b.bp_marks.borrow_mut() = lines.clone();
                    b.doc.cache_rev().update(|r| *r += 1);
                }
            }
        });
    }

    /// Mark `line0` in the buffer for `path` as the stopped line; clear others.
    fn set_stop_line(&self, path: &str, line0: usize) {
        self.buffers.with_untracked(|bs| {
            for b in bs {
                let want = if buffer_matches(b, path) {
                    Some(line0)
                } else {
                    None
                };
                if *b.stop_line.borrow() != want {
                    *b.stop_line.borrow_mut() = want;
                    b.doc.cache_rev().update(|r| *r += 1);
                }
            }
        });
    }

    /// Remove the stopped-line highlight from every buffer.
    fn clear_stop_lines(&self) {
        self.buffers.with_untracked(|bs| {
            for b in bs {
                if b.stop_line.borrow().is_some() {
                    *b.stop_line.borrow_mut() = None;
                    b.doc.cache_rev().update(|r| *r += 1);
                }
            }
        });
    }

    /// Enable/disable Xdebug in Grove's runtime (`grove debug on|off`). Runs off
    /// the UI thread and degrades gracefully when Grove isn't installed — the
    /// editor and its other debug adapters work regardless.
    pub fn set_grove_xdebug(&self, on: bool) {
        let state = *self;
        let report = create_ext_action(self.cx, move |msg: String| state.debug_status.set(msg));
        std::thread::spawn(move || {
            let arg = if on { "on" } else { "off" };
            let msg = match std::process::Command::new("grove")
                .args(["debug", arg])
                .output()
            {
                Ok(o) if o.status.success() => format!("Grove: Xdebug {arg}"),
                Ok(o) => {
                    let err = String::from_utf8_lossy(&o.stderr);
                    let err = err.trim();
                    if err.is_empty() {
                        format!("Grove: could not turn Xdebug {arg}")
                    } else {
                        format!("Grove: {err}")
                    }
                }
                Err(_) => "Grove CLI not found — install Grove to toggle Xdebug".to_string(),
            };
            report(msg);
        });
    }

    /// Reflect Grove's actual Xdebug state in the `xdebug` setting at startup, so
    /// the toggle matches reality. Best-effort + off the UI thread; a no-op when
    /// Grove (or its daemon) isn't running.
    pub fn sync_grove_xdebug_state(&self) {
        let state = *self;
        let apply = create_ext_action(self.cx, move |on: bool| {
            state.settings.update(|st| st.xdebug = on);
        });
        std::thread::spawn(move || {
            if let Ok(out) = std::process::Command::new("grove")
                .args(["debug", "status"])
                .output()
            {
                if out.status.success() {
                    let text = String::from_utf8_lossy(&out.stdout);
                    // "Xdebug enabled …" vs "Xdebug disabled …".
                    if text.contains("Xdebug enabled") {
                        apply(true);
                    } else if text.contains("Xdebug disabled") {
                        apply(false);
                    }
                }
            }
        });
    }

    /// Toggle the debug panel without touching the session.
    pub fn toggle_debug_panel(&self) {
        let open = !self.debug_open.get_untracked();
        self.debug_open.set(open);
    }

    /// F5: start a session, or continue if one is already paused.
    pub fn debug_start(&self) {
        if self.debug_client.get_untracked().is_some() {
            self.debug_continue();
            return;
        }
        self.debug_open.set(true);

        let root = self.root.get_untracked();
        let active_file = self
            .active_buffer()
            .and_then(|b| b.file.path.clone())
            .map(|p| p.to_string_lossy().into_owned());
        let language = self.active_buffer().map(|b| b.file.language);

        let spec = match adapter_spec(language, &root, active_file.as_deref()) {
            Ok(s) => s,
            Err(e) => {
                self.debug_status.set(e);
                return;
            }
        };

        // Bridge the adapter's continuous event stream onto the UI thread via a
        // channel (`create_ext_action` is one-shot; DAP events are not).
        let state = *self;
        let (tx, rx) = channel::<DapEvent>();
        let events = create_signal_from_channel(rx);
        self.cx.create_effect(move |_| {
            if let Some(ev) = events.get() {
                state.on_dap_event(ev);
            }
        });
        let on_event: e_dap::EventHandler = Box::new(move |ev| {
            let _ = tx.send(ev);
        });

        // Marshal the created client (or an error) back to the UI thread.
        let store = create_ext_action(
            self.cx,
            move |res: Result<std::sync::Arc<DapClient>, String>| match res {
                Ok(c) => {
                    state.debug_client.set(Some(c));
                    state.debug_status.set("launching…".to_string());
                }
                Err(e) => state.debug_status.set(e),
            },
        );

        self.debug_status.set("starting adapter…".to_string());

        // Spawning the adapter and connecting (TCP adapters can take a moment)
        // must never block the UI thread — do it all on a worker. The client is
        // stored *before* the handshake so the `initialized` handler can find
        // it to send breakpoints + configurationDone.
        let program = spec.program;
        let args = spec.args;
        let transport = spec.transport;
        let adapter_id = spec.adapter_id;
        let launch = spec.launch;
        std::thread::spawn(move || {
            let argv: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
            let client = match transport {
                Transport::Stdio => DapClient::start(&program, &argv, on_event),
                Transport::TcpServer { port } => DapClient::spawn_server_and_connect(
                    &program,
                    &argv,
                    ("127.0.0.1", port),
                    std::time::Duration::from_secs(8),
                    on_event,
                ),
            };
            match client {
                Ok(c) => {
                    store(Ok(c.clone()));
                    let _ = c.initialize(&adapter_id);
                    let _ = c.launch(launch);
                }
                Err(e) => store(Err(format!("adapter failed to start: {e}"))),
            }
        });
    }

    /// Handle an adapter-pushed event (already on the UI thread).
    pub(crate) fn on_dap_event(&self, ev: DapEvent) {
        match ev.event.as_str() {
            "initialized" => {
                self.debug_status.set("running".to_string());
                if let Some(client) = self.debug_client.get_untracked() {
                    let bps = self.debug_breakpoints.get_untracked();
                    std::thread::spawn(move || {
                        for (path, lines) in &bps {
                            let _ = client.set_breakpoints(path, lines);
                        }
                        let _ = client.set_exception_breakpoints(&[]);
                        let _ = client.configuration_done();
                    });
                }
            }
            "stopped" => {
                self.debug_status.set("paused".to_string());
                let tid = ev.body["threadId"].as_i64().unwrap_or(1);
                self.debug_thread.set(tid);
                self.debug_refresh(tid);
            }
            "continued" => {
                self.debug_status.set("running".to_string());
                self.debug_frames.set(Vec::new());
                self.debug_vars.set(Vec::new());
                self.clear_stop_lines();
            }
            "output" => {
                if let Some(line) = ev.body["output"].as_str() {
                    let line = line.trim_end().to_string();
                    if !line.is_empty() {
                        self.debug_output.update(|o| {
                            o.push(line);
                            let len = o.len();
                            if len > 500 {
                                o.drain(0..len - 500);
                            }
                        });
                    }
                }
            }
            "terminated" | "exited" => {
                self.debug_status.set("terminated".to_string());
                self.debug_client.set(None);
                self.debug_frames.set(Vec::new());
                self.debug_vars.set(Vec::new());
                self.clear_stop_lines();
            }
            _ => {}
        }
    }

    /// Fetch the call stack + top-frame variables after a stop.
    fn debug_refresh(&self, thread_id: i64) {
        let Some(client) = self.debug_client.get_untracked() else {
            return;
        };
        let state = *self;
        let store = create_ext_action(
            self.cx,
            move |(frames, vars): (Vec<DebugFrame>, Vec<DebugVar>)| {
                state.debug_vars.set(vars);
                // Reveal + mark the stopped location.
                if let Some(top) = frames.first() {
                    if !top.path.is_empty() {
                        let line0 = (top.line.max(1) - 1) as usize;
                        state.jump_to(&path_to_uri(Path::new(&top.path)), line0, 0);
                        state.set_stop_line(&top.path, line0);
                    }
                }
                state.debug_frames.set(frames);
            },
        );
        std::thread::spawn(move || {
            let frames = client.stack_trace(thread_id).unwrap_or_default();
            let mut vars = Vec::new();
            if let Some(top) = frames.first() {
                if let Ok(scopes) = client.scopes(top.id) {
                    for scope in scopes.iter().take(3) {
                        if let Ok(vs) = client.variables(scope.variables_reference) {
                            for v in vs {
                                vars.push(DebugVar {
                                    name: v.name,
                                    value: v.value,
                                    ty: v.type_name.unwrap_or_default(),
                                });
                            }
                        }
                    }
                }
            }
            let dframes = frames
                .iter()
                .map(|f| DebugFrame {
                    name: f.name.clone(),
                    path: f.source_path.clone().unwrap_or_default(),
                    line: f.line,
                })
                .collect();
            store((dframes, vars));
        });
    }

    fn control(&self, f: impl FnOnce(&DapClient, i64) + Send + 'static) {
        let Some(client) = self.debug_client.get_untracked() else {
            return;
        };
        let tid = self.debug_thread.get_untracked();
        std::thread::spawn(move || f(&client, tid));
    }

    pub fn debug_continue(&self) {
        self.debug_status.set("running".to_string());
        self.debug_frames.set(Vec::new());
        self.debug_vars.set(Vec::new());
        self.control(|c, t| {
            let _ = c.continue_(t);
        });
    }

    pub fn debug_step_over(&self) {
        self.control(|c, t| {
            let _ = c.next(t);
        });
    }

    pub fn debug_step_into(&self) {
        self.control(|c, t| {
            let _ = c.step_in(t);
        });
    }

    pub fn debug_step_out(&self) {
        self.control(|c, t| {
            let _ = c.step_out(t);
        });
    }

    pub fn debug_stop(&self) {
        if let Some(client) = self.debug_client.get_untracked() {
            std::thread::spawn(move || client.disconnect(true));
        }
        self.debug_client.set(None);
        self.debug_status.set("stopped".to_string());
        self.debug_frames.set(Vec::new());
        self.debug_vars.set(Vec::new());
        self.clear_stop_lines();
    }

    /// F9: toggle a breakpoint on the caret's line in the active file.
    pub fn debug_toggle_breakpoint(&self) {
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let Some(path) = buf.file.path.clone() else {
            return;
        };
        let Some(editor) = buf.editor.get_untracked() else {
            return;
        };
        let (line0, _) = editor.offset_to_line_col(editor.cursor.get_untracked().offset());
        let line = (line0 + 1) as u32;
        let key = path.to_string_lossy().into_owned();

        self.debug_breakpoints.update(|m| {
            let v = m.entry(key.clone()).or_default();
            if let Some(pos) = v.iter().position(|l| *l == line) {
                v.remove(pos);
                if v.is_empty() {
                    m.remove(&key);
                }
            } else {
                v.push(line);
                v.sort_unstable();
            }
        });

        // Paint the dot in the open buffer immediately.
        self.sync_bp_marks(&key);

        // Push the updated set for this file to a live session.
        if let Some(client) = self.debug_client.get_untracked() {
            let lines = self
                .debug_breakpoints
                .with_untracked(|m| m.get(&key).cloned().unwrap_or_default());
            std::thread::spawn(move || {
                let _ = client.set_breakpoints(&key, &lines);
            });
        }
    }
}

/// Resolve the adapter + launch config for the active buffer's language.
fn adapter_spec(
    language: Option<Language>,
    root: &std::path::Path,
    active_file: Option<&str>,
) -> Result<AdapterSpec, String> {
    let node = grove_node_binary()
        .unwrap_or_else(|| PathBuf::from("node"))
        .to_string_lossy()
        .into_owned();
    let cwd = root.to_string_lossy().into_owned();

    match language {
        // Laravel-first: default to PHP/Xdebug when the language is unknown.
        Some(Language::Php) | Some(Language::Blade) | None => {
            let adapter = php_debug_adapter_js().ok_or_else(|| {
                "No php-debug adapter found. Set E_PHP_DEBUG_ADAPTER to phpDebug.js".to_string()
            })?;
            Ok(AdapterSpec {
                transport: Transport::Stdio,
                program: node,
                args: vec![adapter.to_string_lossy().into_owned()],
                adapter_id: "php".to_string(),
                launch: json!({
                    "name": "Listen for Xdebug", "type": "php", "request": "launch",
                    "hostname": "127.0.0.1", "port": 9003, "stopOnEntry": false,
                    "cwd": cwd, "pathMappings": {},
                }),
            })
        }
        Some(Language::JavaScript)
        | Some(Language::TypeScript)
        | Some(Language::Vue)
        | Some(Language::Svelte) => {
            let server = js_debug_server().ok_or_else(|| {
                "No js-debug adapter. Set E_JS_DEBUG_ADAPTER to dapDebugServer.js".to_string()
            })?;
            let port = env_port("E_JS_DEBUG_PORT", 8123);
            let program = active_file.ok_or_else(|| "Open a .js/.ts file to debug".to_string())?;
            Ok(AdapterSpec {
                transport: Transport::TcpServer { port },
                program: node,
                args: vec![server.to_string_lossy().into_owned(), port.to_string()],
                adapter_id: "pwa-node".to_string(),
                launch: json!({
                    "name": "Launch", "type": "pwa-node", "request": "launch",
                    "program": program, "cwd": cwd, "console": "internalConsole",
                }),
            })
        }
        Some(Language::Rust) | Some(Language::C) | Some(Language::Cpp) => {
            let codelldb = codelldb_binary().ok_or_else(|| {
                "No codelldb found. Set E_CODELLDB to the codelldb binary".to_string()
            })?;
            let port = env_port("E_CODELLDB_PORT", 9552);
            let program = std::env::var("E_DEBUG_PROGRAM")
                .ok()
                .or_else(|| guess_cargo_binary(root))
                .ok_or_else(|| "Set E_DEBUG_PROGRAM to the executable to debug".to_string())?;
            Ok(AdapterSpec {
                transport: Transport::TcpServer { port },
                program: codelldb.to_string_lossy().into_owned(),
                args: vec!["--port".to_string(), port.to_string()],
                adapter_id: "lldb".to_string(),
                launch: json!({
                    "name": "Launch", "type": "lldb", "request": "launch",
                    "program": program, "cwd": cwd,
                }),
            })
        }
        Some(other) => Err(format!("No debug adapter configured for {other:?}")),
    }
}

fn env_port(var: &str, default: u16) -> u16 {
    std::env::var(var)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Best-effort: the debug binary for a Cargo project (`target/debug/<name>`).
fn guess_cargo_binary(root: &std::path::Path) -> Option<String> {
    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).ok()?;
    let name = manifest.lines().find_map(|l| {
        let l = l.trim();
        l.strip_prefix("name")
            .and_then(|r| r.trim_start().strip_prefix('='))
            .map(|v| v.trim().trim_matches('"').to_string())
    })?;
    let bin = root.join("target/debug").join(&name);
    bin.exists().then(|| bin.to_string_lossy().into_owned())
}

/// Locate `vscode-js-debug`'s `dapDebugServer.js`.
fn js_debug_server() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("E_JS_DEBUG_ADAPTER") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Some(p);
        }
    }
    scan_extensions(
        |name| name.contains("js-debug"),
        &["src/dapDebugServer.js", "out/src/dapDebugServer.js"],
    )
}

/// Locate the `codelldb` binary (VS Code CodeLLDB extension, or on PATH).
fn codelldb_binary() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("E_CODELLDB") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Some(p);
        }
    }
    if let Some(p) = scan_extensions(
        |name| name.contains("vscode-lldb") || name.contains("codelldb"),
        &["adapter/codelldb"],
    ) {
        return Some(p);
    }
    which("codelldb")
}

/// Search installed VS Code / Cursor extensions for a file at one of `rel`.
fn scan_extensions(matches: impl Fn(&str) -> bool, rel: &[&str]) -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    for ext_dir in [
        ".vscode/extensions",
        ".cursor/extensions",
        ".vscode-insiders/extensions",
    ] {
        let dir = PathBuf::from(&home).join(ext_dir);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let name = e.file_name();
            let name = name.to_string_lossy();
            if matches(&name) {
                for r in rel {
                    let p = e.path().join(r);
                    if p.exists() {
                        return Some(p);
                    }
                }
            }
        }
    }
    None
}

/// Find an executable on `PATH`.
fn which(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        let cand = dir.join(bin);
        cand.is_file().then_some(cand)
    })
}

/// Whether buffer `b` is backed by the file at `path`.
fn buffer_matches(b: &crate::state::Buffer, path: &str) -> bool {
    b.file
        .path
        .as_deref()
        .map(|p| p.to_string_lossy() == path)
        .unwrap_or(false)
}

/// Locate a Node binary managed by Grove (so the user needn't install Node).
/// Falls back to `node` on `PATH` when Grove isn't present.
fn grove_node_binary() -> Option<PathBuf> {
    let file = grove_node_registry()?;
    let raw = std::fs::read_to_string(&file).ok()?;
    let json: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let builds = json.get("builds")?.as_object()?;
    // Prefer the highest major version.
    let mut best: Option<(u64, PathBuf)> = None;
    for (major, build) in builds {
        let Some(bin) = build.get("node_binary").and_then(|b| b.as_str()) else {
            continue;
        };
        let path = PathBuf::from(bin);
        if !path.exists() {
            continue;
        }
        let major_num: u64 = major.parse().unwrap_or(0);
        if best.as_ref().map(|(m, _)| major_num > *m).unwrap_or(true) {
            best = Some((major_num, path));
        }
    }
    best.map(|(_, p)| p)
}

fn grove_node_registry() -> Option<PathBuf> {
    if let Ok(home) = std::env::var("GROVE_HOME") {
        return Some(PathBuf::from(home).join("runtimes/node-builds.json"));
    }
    let home = std::env::var("HOME").ok()?;
    let base = if cfg!(target_os = "macos") {
        PathBuf::from(&home).join("Library/Application Support/Grove")
    } else {
        std::env::var("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(&home).join(".local/share"))
            .join("Grove")
    };
    Some(base.join("runtimes/node-builds.json"))
}

/// Locate the `vscode-php-debug` adapter entry point (`phpDebug.js`). Honours
/// `E_PHP_DEBUG_ADAPTER`, else scans installed VS Code / Cursor extensions.
fn php_debug_adapter_js() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("E_PHP_DEBUG_ADAPTER") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Some(p);
        }
    }
    scan_extensions(|name| name.contains("php-debug"), &["out/phpDebug.js"])
}
