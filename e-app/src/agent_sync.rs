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
use floem::reactive::{create_effect, SignalGet, SignalUpdate, SignalWith};
use floem::views::editor::text::Document;
use serde_json::{json, Value};

use crate::state::AppState;

type Pending = Arc<Mutex<VecDeque<(Value, Sender<Value>)>>>;

/// Directory holding the per-process editor sockets.
///
/// Tightened to `0700`: it holds the sockets *and* `databases.json`, and the
/// socket names are unguessable only for as long as nobody else can list them.
fn socket_dir() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME")?;
    let dir = std::path::PathBuf::from(home).join(".config").join("e");
    let _ = std::fs::create_dir_all(&dir);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(&dir) {
            if meta.permissions().mode() & 0o077 != 0 {
                let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
            }
        }
    }
    Some(dir)
}

/// 96 bits of randomness for the socket name, base32-encoded to 20 characters.
///
/// `/dev/urandom` is the whole point — a predictable name would make the
/// capability guessable. If it can't be read we say so and fall back to
/// something merely unique, because a working editor with a weak socket name is
/// still better than no agent sync at all.
///
/// Base32 rather than hex, and 96 bits rather than 128, because the *entire*
/// socket path has to fit in `sun_path` — 104 bytes on macOS, 108 on Linux.
/// Hex-encoded 128 bits costs 32 characters and left too little room for a long
/// `$HOME`; this costs 20 and is still far past guessing.
fn socket_nonce() -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz234567";
    let mut buf = [0u8; 12];
    if let Err(e) = std::fs::File::open("/dev/urandom").and_then(|mut f| {
        use std::io::Read;
        f.read_exact(&mut buf)
    }) {
        eprintln!("e: could not read /dev/urandom ({e}); agent socket name is not secret");
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        buf.copy_from_slice(&nanos.to_le_bytes()[..12]);
    }
    // 96 bits, five at a time.
    let bits = buf.iter().fold(0u128, |acc, &b| (acc << 8) | b as u128);
    (0..20)
        .map(|i| ALPHABET[((bits >> (5 * (19 - i))) & 0x1f) as usize] as char)
        .collect()
}

/// Path of the per-process editor socket.
///
/// The random component is the access control: knowing the path is what lets a
/// caller drive the editor, and the path is handed only to processes the editor
/// spawns, via `$E_EDITOR_SOCK`. Before this, the name was `agent-<pid>.sock` in
/// a world-listable directory, so any local process could find it by globbing
/// and had `run` (arbitrary shell) and `tinker` (arbitrary PHP) for the taking.
fn socket_path() -> Option<std::path::PathBuf> {
    let dir = socket_dir()?;
    let nonce = socket_nonce();
    let full = dir.join(format!("agent-{}-{nonce}.sock", std::process::id()));
    // `sun_path` is 104 bytes on macOS and 108 on Linux, and binding past it
    // fails outright. Under a long `$HOME` the pid is the part worth dropping:
    // nothing reads it back (the sweep tests for a listener, not a pid), it is
    // only there to make the file legible to a human.
    if full.as_os_str().len() < SUN_PATH_BUDGET {
        return Some(full);
    }
    let short = dir.join(format!("agent-{nonce}.sock"));
    if short.as_os_str().len() >= SUN_PATH_BUDGET {
        eprintln!(
            "e: {} is too long for a Unix socket path; agent sync is off",
            dir.display()
        );
        return None;
    }
    Some(short)
}

/// Stay clear of the smaller of the two `sun_path` limits.
const SUN_PATH_BUDGET: usize = 100;

/// Sockets younger than this are left alone, so we can't delete one belonging to
/// an editor that is starting up right now.
#[cfg(unix)]
const SOCKET_GRACE: Duration = Duration::from_secs(30);

/// Delete `agent-*.sock` files left behind by editors that are no longer running.
///
/// Every process created one at startup and nothing ever removed anyone else's,
/// so `~/.config/e` accumulated one file per editor launch, indefinitely.
///
/// The liveness test is "does anything answer on this socket", not "is this pid
/// alive": pids are recycled, so a pid check can point at an unrelated process,
/// whereas a refused connection means nothing is serving the socket and the file
/// is litter. This also subsumes clearing our own stale socket before binding.
#[cfg(unix)]
fn sweep_stale_sockets(dir: &std::path::Path) {
    sweep_stale_sockets_with_grace(dir, SOCKET_GRACE)
}

/// [`sweep_stale_sockets`] with the grace window injected, so tests can drive
/// both sides of it without backdating a socket's mtime (which can't be opened
/// for writing anyway).
#[cfg(unix)]
fn sweep_stale_sockets_with_grace(dir: &std::path::Path, grace: Duration) {
    use std::os::unix::net::UnixStream;

    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut removed = 0usize;
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with("agent-") || !name.ends_with(".sock") {
            continue;
        }
        let path = entry.path();
        let fresh = entry
            .metadata()
            .and_then(|m| m.modified())
            .map(|t| t.elapsed().map(|e| e < grace).unwrap_or(true))
            .unwrap_or(false);
        if fresh {
            continue;
        }
        if UnixStream::connect(&path).is_ok() {
            continue; // a live editor is serving this one
        }
        if std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    if removed > 0 {
        eprintln!("e: cleaned up {removed} stale agent socket(s)");
    }
}

/// Start the agent-sync server. Safe to call once at startup; a no-op on
/// platforms without Unix sockets.
#[cfg(unix)]
pub fn start(state: AppState) {
    use std::os::unix::net::UnixListener;

    let Some(path) = socket_path() else {
        return;
    };
    if let Some(dir) = socket_dir() {
        sweep_stale_sockets(&dir);
    }
    let _ = std::fs::remove_file(&path); // our own, if the sweep's grace window kept it
    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("e: agent sync socket failed: {e}");
            return;
        }
    };
    // Owner-only. macOS honours socket permissions; several Linux filesystems
    // do too, and where they don't the 0700 directory is what carries it. Both
    // are belt and braces over the unguessable name.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)) {
            eprintln!("e: could not restrict agent socket permissions: {e}");
        }
    }
    // Let spawned agents discover the socket. This variable *is* the capability:
    // anything that inherits it can drive the editor, including `run`.
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
            dispatch(state, &req, reply);
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
        // Generous timeout: some methods (running tests, LSP, DB schema) take a
        // while. Interactive queries reply near-instantly.
        let resp = rx
            .recv_timeout(Duration::from_secs(300))
            .unwrap_or_else(|_| json!({"ok": false, "error": "editor did not respond"}));
        if writeln!(writer, "{resp}").is_err() {
            break;
        }
    }
}

/// Execute one request against the editor (runs on the UI thread).
fn dispatch(state: AppState, req: &Value, reply: Sender<Value>) {
    let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let line = || req.get("line").and_then(|l| l.as_u64()).unwrap_or(1).max(1) as u32 - 1;
    let col = || req.get("col").and_then(|c| c.as_u64()).unwrap_or(1).max(1) as u32 - 1;

    // Audit everything the agent does.
    state.agent_log_push(method, log_summary(req));

    let sync: Value = match method {
        "context" => context(state),
        "diagnostics" => json!({ "ok": true, "diagnostics": diagnostics(state) }),
        // Hand the editor a written summary of this session's changes; it becomes
        // the description when you ship the changeset from the review panel.
        "review_summary" => {
            let Some(text) = req.get("text").and_then(|t| t.as_str()) else {
                let _ = reply.send(json!({"ok": false, "error": "missing text"}));
                return;
            };
            state.review_summary.set(Some(text.to_string()));
            json!({"ok": true})
        }
        "open" => {
            let Some(path) = req.get("path").and_then(|p| p.as_str()) else {
                let _ = reply.send(json!({"ok": false, "error": "missing path"}));
                return;
            };
            state.jump_to(&path_to_uri(path), line() as usize, col() as usize);
            state.set_agent_mark(std::path::PathBuf::from(path), line() as usize);
            json!({"ok": true})
        }
        // Ghost marker: show where the agent is looking.
        "mark" => {
            let Some(path) = req.get("path").and_then(|p| p.as_str()) else {
                let _ = reply.send(json!({"ok": false, "error": "missing path"}));
                return;
            };
            state.set_agent_mark(std::path::PathBuf::from(path), line() as usize);
            json!({"ok": true})
        }
        // Propose replacing a file's contents; the user reviews hunks and the
        // reply is answered on apply/cancel.
        "propose_edit" => {
            let Some(path) = req.get("path").and_then(|p| p.as_str()).map(String::from) else {
                let _ = reply.send(json!({"ok": false, "error": "missing path"}));
                return;
            };
            let Some(content) = req
                .get("content")
                .and_then(|c| c.as_str())
                .map(String::from)
            else {
                let _ = reply.send(json!({"ok": false, "error": "missing content"}));
                return;
            };
            state.agent_propose_edit(std::path::PathBuf::from(path), content, reply);
            return;
        }
        "focus" => {
            match req
                .get("target")
                .and_then(|t| t.as_str())
                .unwrap_or("editor")
            {
                "terminal" if !state.terminal_open.get_untracked() => state.toggle_terminal(),
                "agent" if !state.agent_open.get_untracked() => state.toggle_agent(),
                "editor" => {
                    if let Some(id) = state.focused_active_id() {
                        state.focus_buffer(id);
                    }
                }
                _ => {}
            }
            json!({"ok": true})
        }
        "notify" => {
            AppState::notify(req.get("message").and_then(|m| m.as_str()).unwrap_or(""));
            json!({"ok": true})
        }

        // ---- LSP co-op (async) --------------------------------------------
        "lsp_definition" | "lsp_references" | "lsp_hover" => {
            let Some(path) = req.get("path").and_then(|p| p.as_str()) else {
                let _ = reply.send(json!({"ok": false, "error": "missing path"}));
                return;
            };
            let (client, uri) = resolve_lsp(state, path);
            let Some(client) = client else {
                let _ = reply.send(json!({"ok": false, "error": "no language server for file"}));
                return;
            };
            state.set_agent_mark(std::path::PathBuf::from(path), line() as usize);
            let (m, l, c) = (method.to_string(), line(), col());
            std::thread::spawn(move || {
                let resp = match m.as_str() {
                    "lsp_definition" => match client.definition(&uri, l, c) {
                        Ok(Some((u, ln, ch))) => {
                            json!({"ok": true, "uri": u, "line": ln + 1, "col": ch + 1})
                        }
                        Ok(None) => json!({"ok": true, "result": null}),
                        Err(e) => json!({"ok": false, "error": e.to_string()}),
                    },
                    "lsp_references" => match client.references(&uri, l, c) {
                        Ok(refs) => json!({"ok": true, "references": refs.into_iter()
                            .map(|(u, ln, ch)| json!({"uri": u, "line": ln + 1, "col": ch + 1}))
                            .collect::<Vec<_>>()}),
                        Err(e) => json!({"ok": false, "error": e.to_string()}),
                    },
                    _ => match client.hover(&uri, l, c) {
                        Ok(h) => json!({"ok": true, "hover": h}),
                        Err(e) => json!({"ok": false, "error": e.to_string()}),
                    },
                };
                let _ = reply.send(resp);
            });
            return;
        }
        "lsp_symbols" => {
            let query = req
                .get("query")
                .and_then(|q| q.as_str())
                .unwrap_or("")
                .to_string();
            let Some(client) = state.lsp_for_active() else {
                let _ = reply.send(json!({"ok": false, "error": "no active language server"}));
                return;
            };
            std::thread::spawn(move || {
                let resp = match client.workspace_symbol(&query) {
                    Ok(syms) => json!({"ok": true, "symbols": syms.into_iter()
                        .map(|(name, uri, ln, ch)| json!({"name": name, "uri": uri, "line": ln + 1, "col": ch + 1}))
                        .collect::<Vec<_>>()}),
                    Err(e) => json!({"ok": false, "error": e.to_string()}),
                };
                let _ = reply.send(resp);
            });
            return;
        }

        // ---- Database schema (async, read-only) ---------------------------
        "db_schema" => {
            let name = req.get("connection").and_then(|c| c.as_str());
            let picked = state.db_conns.with_untracked(|conns| {
                conns
                    .iter()
                    .filter(|e| e.conn.get_untracked().is_some())
                    .find(|e| name.is_none_or(|n| e.config.display_name() == n))
                    .and_then(|e| e.conn.get_untracked())
            });
            let Some(conn) = picked else {
                let _ = reply.send(json!({"ok": false, "error": "no connected database"}));
                return;
            };
            std::thread::spawn(move || {
                let _ = reply.send(db_schema(&conn));
            });
            return;
        }

        // ---- Database query (async, user-consented) -----------------------
        "db_query" => {
            let Some(sql) = req.get("sql").and_then(|s| s.as_str()).map(String::from) else {
                let _ = reply.send(json!({"ok": false, "error": "missing sql"}));
                return;
            };
            let name = req.get("connection").and_then(|c| c.as_str());
            let picked = state.db_conns.with_untracked(|conns| {
                conns
                    .iter()
                    .filter(|e| e.conn.get_untracked().is_some())
                    .find(|e| name.is_none_or(|n| e.config.display_name() == n))
                    .map(|e| (e.config.display_name(), e.conn.get_untracked()))
            });
            let Some((db_name, Some(conn))) = picked else {
                let _ = reply.send(json!({"ok": false, "error": "no connected database"}));
                return;
            };
            // Ask the user before touching their database.
            state.db_consent.set(Some(crate::state::DbConsent {
                sql,
                db_name,
                conn,
                reply,
            }));
            return;
        }

        // ---- Laravel Tinker (async) ---------------------------------------
        "tinker" => {
            let Some(code) = req.get("code").and_then(|c| c.as_str()).map(String::from) else {
                let _ = reply.send(json!({"ok": false, "error": "missing code"}));
                return;
            };
            let root = state.root.get_untracked().to_string_lossy().into_owned();
            std::thread::spawn(move || {
                let tmp =
                    std::env::temp_dir().join(format!("e-tinker-agent-{}.php", std::process::id()));
                let _ = std::fs::write(&tmp, code);
                let resp = run_command(
                    &format!(
                        "php -d error_reporting=0 -d display_errors=0 artisan tinker < {}",
                        tmp.display()
                    ),
                    &root,
                );
                let _ = std::fs::remove_file(&tmp);
                let _ = reply.send(resp);
            });
            return;
        }

        // ---- Run a shell command (async) ----------------------------------
        "run" => {
            let Some(command) = req
                .get("command")
                .and_then(|c| c.as_str())
                .map(String::from)
            else {
                let _ = reply.send(json!({"ok": false, "error": "missing command"}));
                return;
            };
            let cwd = req
                .get("cwd")
                .and_then(|c| c.as_str())
                .map(String::from)
                .unwrap_or_else(|| state.root.get_untracked().to_string_lossy().into_owned());
            std::thread::spawn(move || {
                let _ = reply.send(run_command(&command, &cwd));
            });
            return;
        }

        other => json!({"ok": false, "error": format!("unknown method: {other}")}),
    };
    let _ = reply.send(sync);
}

/// A short human summary of a request for the audit timeline.
fn log_summary(req: &Value) -> String {
    let s = |k: &str| req.get(k).and_then(|v| v.as_str()).unwrap_or("");
    let path = s("path");
    let short = path.rsplit('/').next().unwrap_or(path);
    match req.get("method").and_then(|m| m.as_str()).unwrap_or("") {
        "open" | "mark" | "lsp_definition" | "lsp_references" | "lsp_hover" => {
            let line = req.get("line").and_then(|l| l.as_u64()).unwrap_or(0);
            format!("{short}:{line}")
        }
        "propose_edit" => short.to_string(),
        "run" => s("command").chars().take(60).collect(),
        "tinker" => s("code").chars().take(60).collect(),
        "db_query" => s("sql").chars().take(60).collect(),
        "lsp_symbols" => s("query").to_string(),
        _ => String::new(),
    }
}

/// Resolve the language server + document URI for a path.
fn resolve_lsp(state: AppState, path: &str) -> (Option<std::sync::Arc<e_lsp::LspClient>>, String) {
    let pb = std::path::PathBuf::from(path);
    let open = state.buffers.with_untracked(|bs| {
        bs.iter()
            .find(|b| b.file.path.as_deref() == Some(pb.as_path()))
            .map(|b| (b.file.language, b.uri.clone()))
    });
    let (lang, uri) = match open {
        Some((l, Some(u))) => (l, u),
        Some((l, None)) => (l, path_to_uri(path)),
        None => (
            e_core::buffer::FileInfo::for_path(pb).language,
            path_to_uri(path),
        ),
    };
    (state.lsp_for_language(lang), uri)
}

fn db_schema(conn: &e_db::Conn) -> Value {
    let tables = match e_db::tables(conn) {
        Ok(t) => t,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let mut out = Vec::new();
    for t in tables.iter().take(300) {
        let cols = e_db::columns(conn, t).unwrap_or_default();
        out.push(json!({
            "table": t,
            "columns": cols.iter().map(|c| json!({
                "name": c.name, "type": c.data_type, "nullable": c.nullable, "key": c.key
            })).collect::<Vec<_>>(),
        }));
    }
    json!({"ok": true, "tables": out})
}

fn run_command(command: &str, cwd: &str) -> Value {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    match std::process::Command::new(shell)
        .arg("-ilc")
        .arg(command)
        .current_dir(cwd)
        .output()
    {
        Ok(o) => json!({
            "ok": true,
            "code": o.status.code(),
            "stdout": String::from_utf8_lossy(&o.stdout),
            "stderr": String::from_utf8_lossy(&o.stderr),
        }),
        Err(e) => json!({"ok": false, "error": e.to_string()}),
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    use std::sync::atomic::AtomicU32;

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn scratch_dir() -> std::path::PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        // Not `temp_dir()`: on macOS that is a ~48-character path, and the
        // whole socket path has to fit in `sun_path`.
        let dir = std::path::PathBuf::from("/tmp").join(format!("e-sk-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A socket file with nothing serving it — what a dead editor leaves behind.
    fn orphan(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        let listener = UnixListener::bind(&path).unwrap();
        drop(listener); // the listener goes, the file stays
        path
    }

    /// `HOME` is process-global; serialise the tests that repoint it.
    static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct HomeGuard {
        previous: Option<std::ffi::OsString>,
        dir: std::path::PathBuf,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match &self.previous {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn fake_home(name: &str) -> HomeGuard {
        let lock = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::path::PathBuf::from("/tmp").join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let previous = std::env::var_os("HOME");
        std::env::set_var("HOME", &dir);
        HomeGuard {
            previous,
            dir,
            _lock: lock,
        }
    }

    #[test]
    fn the_config_dir_is_made_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let home = fake_home(&format!("e-home-{}", std::process::id()));
        let inner = home.dir.join(".config").join("e");
        std::fs::create_dir_all(&inner).unwrap();
        std::fs::set_permissions(&inner, std::fs::Permissions::from_mode(0o755)).unwrap();

        let dir = socket_dir().unwrap();
        assert_eq!(dir, inner);
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o700,
            "the socket names are only secret if nobody can list them"
        );
    }

    #[test]
    fn the_socket_path_stays_within_sun_path() {
        let home = fake_home(&format!("e-home-len-{}", std::process::id()));
        let path = socket_path().unwrap();
        assert!(path.as_os_str().len() < SUN_PATH_BUDGET);
        let name = path.file_name().unwrap().to_str().unwrap();
        assert!(
            name.starts_with("agent-") && name.ends_with(".sock"),
            "{name}"
        );
        // Must still bind for real — the length arithmetic is the whole point.
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        assert!(std::os::unix::net::UnixStream::connect(&path).is_ok());
        drop(listener);
        let _ = std::fs::remove_file(&path);
        let _ = home;
    }

    #[test]
    fn a_long_home_drops_the_pid_rather_than_the_randomness() {
        // Pick a $HOME length where "agent-<pid>-<nonce>.sock" overflows the
        // budget but "agent-<nonce>.sock" still fits, computed rather than
        // guessed so it holds whatever this process's pid happens to be.
        let tail = "/.config/e/".len() + "agent-".len() + 20 + ".sock".len();
        let with_pid = tail + 1 + std::process::id().to_string().len();
        let home_len = SUN_PATH_BUDGET - with_pid + 1;
        let name_len = home_len - "/tmp/".len();
        let name = format!("e-home-{}", "d".repeat(name_len - "e-home-".len()));

        let home = fake_home(&name);
        assert_eq!(home.dir.as_os_str().len(), home_len);

        let path = socket_path().unwrap();
        let file = path.file_name().unwrap().to_str().unwrap();
        assert!(
            path.as_os_str().len() < SUN_PATH_BUDGET,
            "{}",
            path.display()
        );
        assert!(
            !file.contains(&std::process::id().to_string()),
            "the pid is the disposable part, the randomness is not: {file}"
        );
        assert_eq!(file.len(), "agent-".len() + 20 + ".sock".len());
        // And it has to actually bind.
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        drop(listener);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn an_impossibly_long_home_turns_agent_sync_off_rather_than_binding_badly() {
        let name = format!("e-home-x-{}", "d".repeat(90));
        let _home = fake_home(&name);
        assert!(socket_path().is_none());
    }

    #[test]
    fn the_socket_name_carries_unguessable_randomness() {
        let a = socket_nonce();
        let b = socket_nonce();
        assert_eq!(a.len(), 20, "96 bits, base32-encoded");
        assert!(
            a.chars()
                .all(|c| c.is_ascii_lowercase() || ('2'..='7').contains(&c)),
            "unexpected character in {a}"
        );
        assert_ne!(a, b, "a fixed name would make the capability guessable");
    }

    #[test]
    fn the_sweep_still_matches_the_randomised_name() {
        // The name gained a nonce for access control; the cleanup that keys off
        // `agent-*.sock` has to keep recognising it.
        let dir = scratch_dir();
        let name = format!("agent-{}-{}.sock", 4242, socket_nonce());
        let dead = orphan(&dir, &name);
        assert!(dead.exists());
        sweep_stale_sockets_with_grace(&dir, Duration::ZERO);
        assert!(
            !dead.exists(),
            "randomised socket names must still be swept"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn removes_orphans_and_keeps_sockets_that_answer() {
        let dir = scratch_dir();

        // A live editor: a listener still bound and accepting.
        let live = dir.join("agent-11111.sock");
        let _listener = UnixListener::bind(&live).unwrap();

        let dead = orphan(&dir, "agent-22222.sock");
        let also_dead = orphan(&dir, "agent-33333.sock");

        // Unrelated files in ~/.config/e must not be collateral.
        let config = dir.join("config.json");
        std::fs::write(&config, "{}").unwrap();
        let db = dir.join("databases.json");
        std::fs::write(&db, "{}").unwrap();

        sweep_stale_sockets_with_grace(&dir, Duration::ZERO);

        assert!(live.exists(), "a socket with a live listener must survive");
        assert!(!dead.exists(), "an unserved socket is litter and must go");
        assert!(!also_dead.exists());
        assert!(config.exists(), "non-socket files must not be touched");
        assert!(db.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_grace_window_protects_a_starting_editor() {
        // Guards the (tiny) race against another editor between its bind() and
        // its listen(), where a connect would be refused even though the
        // process is very much alive.
        let dir = scratch_dir();
        let fresh = orphan(&dir, "agent-44444.sock");
        sweep_stale_sockets_with_grace(&dir, Duration::from_secs(3600));
        assert!(
            fresh.exists(),
            "a socket younger than the grace window must be left alone"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_directory_is_not_an_error() {
        sweep_stale_sockets_with_grace(
            &std::env::temp_dir().join("e-sock-sweep-does-not-exist"),
            Duration::ZERO,
        );
    }
}
