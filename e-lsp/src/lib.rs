//! A small, synchronous-handshake LSP client over stdio.
//!
//! Spawns a language server (e.g. `intelephense --stdio`), performs the
//! `initialize`/`initialized` handshake, and streams text document
//! notifications. Server-pushed diagnostics are delivered to a callback.
//!
//! Outgoing requests are correlated by id; a background reader thread
//! dispatches responses and notifications.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use crossbeam_channel::{bounded, Sender};
use lsp_types::PublishDiagnosticsParams;
use serde_json::{json, Value};

/// Callback invoked (on the reader thread) for each `publishDiagnostics`.
pub type DiagnosticsHandler = Box<dyn Fn(PublishDiagnosticsParams) + Send>;

type Pending = Arc<Mutex<HashMap<i64, Sender<Result<Value, Value>>>>>;

pub struct LspClient {
    child: Child,
    stdin: Arc<Mutex<ChildStdin>>,
    next_id: AtomicI64,
    pending: Pending,
}

impl LspClient {
    /// Spawn a server and complete the initialize handshake.
    pub fn start(
        program: &str,
        args: &[&str],
        root: &Path,
        on_diagnostics: DiagnosticsHandler,
    ) -> Result<Arc<Self>> {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("spawning language server `{program}`"))?;

        let stdin = child.stdin.take().context("server stdin")?;
        let stdout = child.stdout.take().context("server stdout")?;

        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let stdin = Arc::new(Mutex::new(stdin));

        let client = Arc::new(LspClient {
            child,
            stdin: stdin.clone(),
            next_id: AtomicI64::new(1),
            pending: pending.clone(),
        });

        // Reader thread: dispatch responses / notifications / server requests.
        {
            let pending = pending.clone();
            let stdin = stdin.clone();
            thread::spawn(move || {
                read_loop(stdout, pending, stdin, on_diagnostics);
            });
        }

        client.handshake(root)?;
        Ok(client)
    }

    fn handshake(&self, root: &Path) -> Result<()> {
        let root_uri = path_to_uri(root);
        let params = json!({
            "processId": std::process::id(),
            "rootUri": root_uri,
            "capabilities": {
                "textDocument": {
                    "synchronization": { "didSave": true, "dynamicRegistration": false },
                    "publishDiagnostics": { "relatedInformation": true },
                    "completion": {
                        "completionItem": { "snippetSupport": true },
                        "contextSupport": true
                    },
                    "hover": { "contentFormat": ["markdown", "plaintext"] },
                    "definition": {},
                },
                "workspace": { "configuration": true, "workspaceFolders": true }
            },
            "workspaceFolders": [ { "uri": root_uri, "name": "root" } ],
        });

        self.request("initialize", params, Duration::from_secs(15))?;
        self.notify("initialized", json!({}));
        Ok(())
    }

    fn send(&self, msg: &Value) -> Result<()> {
        let body = serde_json::to_vec(msg)?;
        let mut stdin = self.stdin.lock().unwrap();
        write!(stdin, "Content-Length: {}\r\n\r\n", body.len())?;
        stdin.write_all(&body)?;
        stdin.flush()?;
        Ok(())
    }

    /// Send a request and block for the response.
    pub fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = bounded(1);
        self.pending.lock().unwrap().insert(id, tx);

        self.send(&json!({
            "jsonrpc": "2.0", "id": id, "method": method, "params": params
        }))?;

        match rx.recv_timeout(timeout) {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(err)) => Err(anyhow!("LSP error for {method}: {err}")),
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                Err(anyhow!("LSP request `{method}` timed out"))
            }
        }
    }

    /// Send a notification (fire and forget).
    pub fn notify(&self, method: &str, params: Value) {
        let _ = self.send(&json!({
            "jsonrpc": "2.0", "method": method, "params": params
        }));
    }

    pub fn did_open(&self, uri: &str, language_id: &str, version: i64, text: &str) {
        self.notify(
            "textDocument/didOpen",
            json!({ "textDocument": {
                "uri": uri, "languageId": language_id, "version": version, "text": text
            }}),
        );
    }

    pub fn did_change_full(&self, uri: &str, version: i64, text: &str) {
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": { "uri": uri, "version": version },
                "contentChanges": [ { "text": text } ]
            }),
        );
    }

    pub fn did_save(&self, uri: &str, text: &str) {
        self.notify(
            "textDocument/didSave",
            json!({ "textDocument": { "uri": uri }, "text": text }),
        );
    }

    pub fn did_close(&self, uri: &str) {
        self.notify(
            "textDocument/didClose",
            json!({ "textDocument": { "uri": uri } }),
        );
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn read_loop(
    stdout: impl Read,
    pending: Pending,
    stdin: Arc<Mutex<ChildStdin>>,
    on_diagnostics: DiagnosticsHandler,
) {
    let mut reader = BufReader::new(stdout);
    loop {
        let msg = match read_message(&mut reader) {
            Ok(Some(msg)) => msg,
            Ok(None) => break, // EOF: server exited
            Err(_) => break,
        };

        let has_id = msg.get("id").map(|v| !v.is_null()).unwrap_or(false);
        let has_method = msg.get("method").is_some();

        if has_id && !has_method {
            // Response to one of our requests.
            let id = msg["id"].as_i64().unwrap_or(-1);
            if let Some(tx) = pending.lock().unwrap().remove(&id) {
                if let Some(err) = msg.get("error") {
                    let _ = tx.send(Err(err.clone()));
                } else {
                    let _ = tx.send(Ok(msg.get("result").cloned().unwrap_or(Value::Null)));
                }
            }
        } else if has_id && has_method {
            // Server -> client request: must reply or the server may stall.
            respond_to_server_request(&msg, &stdin);
        } else if has_method {
            // Notification.
            let method = msg["method"].as_str().unwrap_or("");
            if method == "textDocument/publishDiagnostics" {
                if let Some(params) = msg.get("params") {
                    if let Ok(p) = serde_json::from_value::<PublishDiagnosticsParams>(params.clone())
                    {
                        on_diagnostics(p);
                    }
                }
            }
        }
    }
}

fn respond_to_server_request(msg: &Value, stdin: &Arc<Mutex<ChildStdin>>) {
    let id = msg.get("id").cloned().unwrap_or(Value::Null);
    let method = msg["method"].as_str().unwrap_or("");

    // Reply with sensible defaults so the server proceeds.
    let result = match method {
        "workspace/configuration" => {
            let n = msg["params"]["items"].as_array().map(|a| a.len()).unwrap_or(0);
            Value::Array(vec![Value::Null; n])
        }
        _ => Value::Null,
    };

    let reply = json!({ "jsonrpc": "2.0", "id": id, "result": result });
    if let Ok(body) = serde_json::to_vec(&reply) {
        if let Ok(mut s) = stdin.lock() {
            let _ = write!(s, "Content-Length: {}\r\n\r\n", body.len());
            let _ = s.write_all(&body);
            let _ = s.flush();
        }
    }
}

/// Read one LSP message (headers + JSON body) from `reader`.
fn read_message(reader: &mut impl BufRead) -> Result<Option<Value>> {
    let mut content_length: Option<usize> = None;

    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            return Ok(None); // EOF
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break; // end of headers
        }
        if let Some(rest) = trimmed.strip_prefix("Content-Length:") {
            content_length = Some(rest.trim().parse().context("bad Content-Length")?);
        }
    }

    let len = content_length.ok_or_else(|| anyhow!("message without Content-Length"))?;
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf)?;
    Ok(Some(serde_json::from_slice(&buf)?))
}

/// Convert a filesystem path to a `file://` URI.
pub fn path_to_uri(path: &Path) -> String {
    let mut s = String::from("file://");
    let p = path.to_string_lossy();
    // Percent-encode the few characters that matter; keep it simple.
    for ch in p.chars() {
        match ch {
            ' ' => s.push_str("%20"),
            '\\' => s.push('/'),
            _ => s.push(ch),
        }
    }
    s
}
