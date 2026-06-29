//! Bidirectional AI agent workspace sync.
//!
//! The editor exposes a local Unix-domain socket that a CLI agent (Elyra,
//! Claude Code, Codex …) — or any tool — can talk to with line-delimited JSON.
//! It lets the agent both *read* what the developer is doing (current file,
//! cursor, selection, diagnostics) and *drive* the editor (open a file at a
//! line, focus a panel, post a notification).
//!
//! Protocol (one JSON object per line, one JSON response per line):
//! - `{"method":"context"}` → current file, cursor, selection, open files,
//!   diagnostics and workspace root.
//! - `{"method":"open","path":"…","line":45,"col":1}` → open + jump.
//! - `{"method":"diagnostics"}` → all problems.
//! - `{"method":"focus","target":"terminal|editor|agent"}`.
//! - `{"method":"notify","message":"…"}`.
//!
//! The socket path is exported to spawned agents as `$E_EDITOR_SOCK`, so an
//! agent can e.g. `printf '{"method":"context"}\n' | nc -U "$E_EDITOR_SOCK"`.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use floem::ext_event::create_signal_from_channel;
use floem::reactive::{create_effect, SignalGet, SignalWith};
use floem::views::editor::text::Document;
use serde_json::{json, Value};

use crate::state::AppState;

type Pending = Arc<Mutex<VecDeque<(Value, Sender<Value>)>>>;

/// Path of the per-process editor socket.
fn socket_path() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME")?;
    let dir = std::path::PathBuf::from(home).join(".config").join("e");
    let _ = std::fs::create_dir_all(&dir);
    Some(dir.join(format!("agent-{}.sock", std::process::id())))
}

/// Start the agent-sync server. Safe to call once at startup; a no-op on
/// platforms without Unix sockets.
#[cfg(unix)]
pub fn start(state: AppState) {
    use std::os::unix::net::UnixListener;

    let Some(path) = socket_path() else {
        return;
    };
    let _ = std::fs::remove_file(&path); // clear any stale socket
    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("e: agent sync socket failed: {e}");
            return;
        }
    };
    // Let spawned agents discover the socket.
    std::env::set_var("E_EDITOR_SOCK", &path);

    let pending: Pending = Arc::new(Mutex::new(VecDeque::new()));
    let (wake_tx, wake_rx) = mpsc::channel::<u64>();
    let counter = Arc::new(AtomicU64::new(0));

    // Accept loop: one reader thread per connection.
    {
        let pending = pending.clone();
        let counter = counter.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let pending = pending.clone();
                let wake_tx = wake_tx.clone();
                let counter = counter.clone();
                std::thread::spawn(move || {
                    handle_conn(stream, pending, wake_tx, counter);
                });
            }
        });
    }

    // UI-thread bridge: drain pending requests and reply.
    let notif = create_signal_from_channel(wake_rx);
    create_effect(move |_| {
        if notif.get().is_none() {
            return;
        }
        loop {
            let item = pending.lock().ok().and_then(|mut q| q.pop_front());
            let Some((req, reply)) = item else { break };
            let resp = dispatch(state, &req);
            let _ = reply.send(resp);
        }
    });
}

#[cfg(not(unix))]
pub fn start(_state: AppState) {}

#[cfg(unix)]
fn handle_conn(
    stream: std::os::unix::net::UnixStream,
    pending: Pending,
    wake_tx: Sender<u64>,
    counter: Arc<AtomicU64>,
) {
    let Ok(read_half) = stream.try_clone() else {
        return;
    };
    let mut writer = stream;
    let reader = BufReader::new(read_half);
    for line in reader.lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(&line)
            .unwrap_or_else(|_| json!({"method": "", "error": "bad json"}));
        let (tx, rx) = mpsc::channel();
        if let Ok(mut q) = pending.lock() {
            q.push_back((value, tx));
        }
        let _ = wake_tx.send(counter.fetch_add(1, Ordering::Relaxed));
        let resp = rx
            .recv_timeout(Duration::from_secs(3))
            .unwrap_or_else(|_| json!({"ok": false, "error": "editor did not respond"}));
        if writeln!(writer, "{resp}").is_err() {
            break;
        }
    }
}

/// Execute one request against the editor (runs on the UI thread).
fn dispatch(state: AppState, req: &Value) -> Value {
    let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
    match method {
        "context" => context(state),
        "diagnostics" => json!({ "ok": true, "diagnostics": diagnostics(state) }),
        "open" => {
            let Some(path) = req.get("path").and_then(|p| p.as_str()) else {
                return json!({"ok": false, "error": "missing path"});
            };
            let line = req.get("line").and_then(|l| l.as_u64()).unwrap_or(1).max(1) as usize - 1;
            let col = req.get("col").and_then(|c| c.as_u64()).unwrap_or(1).max(1) as usize - 1;
            let uri = path_to_uri(path);
            state.jump_to(&uri, line, col);
            json!({"ok": true})
        }
        "focus" => {
            match req
                .get("target")
                .and_then(|t| t.as_str())
                .unwrap_or("editor")
            {
                "terminal" => {
                    if !state.terminal_open.get_untracked() {
                        state.toggle_terminal();
                    }
                }
                "agent" => {
                    if !state.agent_open.get_untracked() {
                        state.toggle_agent();
                    }
                }
                _ => {
                    if let Some(id) = state.focused_active_id() {
                        state.focus_buffer(id);
                    }
                }
            }
            json!({"ok": true})
        }
        "notify" => {
            let msg = req.get("message").and_then(|m| m.as_str()).unwrap_or("");
            AppState::notify(msg);
            json!({"ok": true})
        }
        other => json!({"ok": false, "error": format!("unknown method: {other}")}),
    }
}

fn context(state: AppState) -> Value {
    let root = state.root.get_untracked().to_string_lossy().into_owned();
    let open_files: Vec<String> = state.buffers.with_untracked(|bs| {
        bs.iter()
            .filter_map(|b| {
                b.file
                    .path
                    .as_ref()
                    .map(|p| p.to_string_lossy().into_owned())
            })
            .collect()
    });

    let mut file = Value::Null;
    let mut line = Value::Null;
    let mut col = Value::Null;
    let mut selection = Value::Null;
    let mut language = Value::Null;
    let mut dirty = Value::Null;
    let mut doc_len = Value::Null;

    if let Some(buf) = state.active_buffer() {
        doc_len = json!(buf.doc.text().len());
        if let Some(p) = buf.file.path.as_ref() {
            file = json!(p.to_string_lossy());
        }
        language = json!(format!("{:?}", buf.file.language));
        dirty = json!(buf.dirty.get_untracked());
        if let Some(editor) = buf.editor.get_untracked() {
            let cursor = editor.cursor.get_untracked();
            let offset = cursor.offset();
            let (l, c) = editor.offset_to_line_col(offset);
            line = json!(l + 1);
            col = json!(c + 1);
            let text = buf.doc.text().to_string();
            if let floem::views::editor::core::cursor::CursorMode::Insert(sel) = &cursor.mode {
                if let Some(region) = sel.regions().iter().find(|r| r.min() != r.max()) {
                    let s = region.min().min(text.len());
                    let e = region.max().min(text.len());
                    selection = json!(&text[s..e]);
                }
            }
        }
    }

    json!({
        "ok": true,
        "root": root,
        "file": file,
        "line": line,
        "col": col,
        "selection": selection,
        "language": language,
        "dirty": dirty,
        "doc_len": doc_len,
        "open_files": open_files,
        "diagnostics": diagnostics(state),
    })
}

fn diagnostics(state: AppState) -> Vec<Value> {
    let mut out = Vec::new();
    state.diagnostics.with_untracked(|map| {
        for (uri, diags) in map {
            let path = uri_to_path_str(uri);
            for d in diags {
                let severity = match d.severity {
                    Some(lsp_types::DiagnosticSeverity::ERROR) => "error",
                    Some(lsp_types::DiagnosticSeverity::WARNING) => "warning",
                    Some(lsp_types::DiagnosticSeverity::INFORMATION) => "info",
                    Some(lsp_types::DiagnosticSeverity::HINT) => "hint",
                    _ => "info",
                };
                out.push(json!({
                    "file": path,
                    "line": d.range.start.line + 1,
                    "col": d.range.start.character + 1,
                    "severity": severity,
                    "message": d.message,
                }));
            }
        }
    });
    out
}

fn path_to_uri(path: &str) -> String {
    if path.starts_with("file://") {
        path.to_string()
    } else {
        format!("file://{path}")
    }
}

fn uri_to_path_str(uri: &str) -> String {
    uri.strip_prefix("file://").unwrap_or(uri).to_string()
}
