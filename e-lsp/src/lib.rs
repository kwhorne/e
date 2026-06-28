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
use lsp_types::{
    CompletionItem, CompletionResponse, Diagnostic, GotoDefinitionResponse, Hover, HoverContents,
    MarkedString, PublishDiagnosticsParams, TextEdit,
};

/// Active signature info for the signature-help popup.
#[derive(Clone, Debug)]
pub struct SignatureInfo {
    pub label: String,
    /// Character range of the active parameter within `label`, if known.
    pub active: Option<(u32, u32)>,
}

/// A code action with its concrete text edits, grouped by document URI.
#[derive(Clone, Debug)]
pub struct CodeActionItem {
    pub title: String,
    pub edits: Vec<(String, Vec<TextEdit>)>,
}
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
                        "completionItem": { "snippetSupport": false },
                        "contextSupport": true
                    },
                    "hover": { "contentFormat": ["markdown", "plaintext"] },
                    "definition": {},
                    "codeAction": {
                        "dynamicRegistration": false,
                        "codeActionLiteralSupport": {
                            "codeActionKind": {
                                "valueSet": ["quickfix", "refactor", "source", "source.organizeImports"]
                            }
                        }
                    },
                    "formatting": {},
                },
                "workspace": { "configuration": true, "workspaceFolders": true }
            },
            "workspaceFolders": [ { "uri": root_uri, "name": "root" } ],
        });

        self.request("initialize", params, Duration::from_secs(30))?;
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

    /// Request completions at a position. Blocking; call off the UI thread.
    pub fn completion(&self, uri: &str, line: u32, character: u32) -> Result<Vec<CompletionItem>> {
        let params = json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character },
            "context": { "triggerKind": 1 }
        });
        let res = self.request("textDocument/completion", params, Duration::from_secs(5))?;
        if res.is_null() {
            return Ok(Vec::new());
        }
        let parsed: CompletionResponse = serde_json::from_value(res)?;
        Ok(match parsed {
            CompletionResponse::Array(items) => items,
            CompletionResponse::List(list) => list.items,
        })
    }

    /// Request the definition location of the symbol at a position.
    /// Returns `(uri, line, character)`. Blocking; call off the UI thread.
    pub fn definition(
        &self,
        uri: &str,
        line: u32,
        character: u32,
    ) -> Result<Option<(String, u32, u32)>> {
        let params = json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character }
        });
        let res = self.request("textDocument/definition", params, Duration::from_secs(5))?;
        if res.is_null() {
            return Ok(None);
        }
        let resp: GotoDefinitionResponse = serde_json::from_value(res)?;
        let loc = match resp {
            GotoDefinitionResponse::Scalar(l) => Some((l.uri.to_string(), l.range.start)),
            GotoDefinitionResponse::Array(v) => v
                .into_iter()
                .next()
                .map(|l| (l.uri.to_string(), l.range.start)),
            GotoDefinitionResponse::Link(v) => v
                .into_iter()
                .next()
                .map(|l| (l.target_uri.to_string(), l.target_range.start)),
        };
        Ok(loc.map(|(uri, pos)| (uri, pos.line, pos.character)))
    }

    /// Find all references to the symbol at a position.
    /// Returns `(uri, line, character)` per reference.
    pub fn references(
        &self,
        uri: &str,
        line: u32,
        character: u32,
    ) -> Result<Vec<(String, u32, u32)>> {
        let params = json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character },
            "context": { "includeDeclaration": true }
        });
        let res = self.request("textDocument/references", params, Duration::from_secs(5))?;
        Ok(locations_from_value(&res))
    }

    /// Search workspace symbols by name. Returns `(name, uri, line, character)`.
    pub fn workspace_symbol(&self, query: &str) -> Result<Vec<(String, String, u32, u32)>> {
        let params = json!({ "query": query });
        let res = self.request("workspace/symbol", params, Duration::from_secs(5))?;
        let mut out = Vec::new();
        if let Some(arr) = res.as_array() {
            for s in arr {
                let Some(name) = s.get("name").and_then(|n| n.as_str()) else {
                    continue;
                };
                let loc = &s["location"];
                if let (Some(uri), Some(line), Some(ch)) = (
                    loc["uri"].as_str(),
                    loc["range"]["start"]["line"].as_u64(),
                    loc["range"]["start"]["character"].as_u64(),
                ) {
                    out.push((name.to_string(), uri.to_string(), line as u32, ch as u32));
                }
            }
        }
        Ok(out)
    }

    /// Request whole-document formatting. Returns the edits to apply.
    pub fn formatting(
        &self,
        uri: &str,
        tab_size: u32,
        insert_spaces: bool,
    ) -> Result<Vec<TextEdit>> {
        let params = json!({
            "textDocument": { "uri": uri },
            "options": { "tabSize": tab_size, "insertSpaces": insert_spaces }
        });
        let res = self.request("textDocument/formatting", params, Duration::from_secs(8))?;
        if res.is_null() {
            return Ok(Vec::new());
        }
        Ok(serde_json::from_value(res)?)
    }

    /// Request code actions (quick-fixes) for a range. Blocking.
    pub fn code_actions(
        &self,
        uri: &str,
        start_line: u32,
        start_char: u32,
        end_line: u32,
        end_char: u32,
        diagnostics: &[Diagnostic],
    ) -> Result<Vec<CodeActionItem>> {
        let params = json!({
            "textDocument": { "uri": uri },
            "range": {
                "start": { "line": start_line, "character": start_char },
                "end": { "line": end_line, "character": end_char }
            },
            "context": { "diagnostics": serde_json::to_value(diagnostics)? }
        });
        let res = self.request("textDocument/codeAction", params, Duration::from_secs(5))?;
        let mut out = Vec::new();
        if let Some(arr) = res.as_array() {
            for it in arr {
                let Some(title) = it.get("title").and_then(|t| t.as_str()) else {
                    continue;
                };
                let edits = parse_workspace_edit(it.get("edit"));
                out.push(CodeActionItem {
                    title: title.to_string(),
                    edits,
                });
            }
        }
        Ok(out)
    }

    /// Document symbols for `uri` as a flat list `(name, kind, line, char, depth)`.
    pub fn document_symbols(&self, uri: &str) -> Result<Vec<(String, i64, u32, u32, usize)>> {
        let params = json!({ "textDocument": { "uri": uri } });
        let res = self.request(
            "textDocument/documentSymbol",
            params,
            Duration::from_secs(5),
        )?;
        let mut out = Vec::new();
        if let Some(arr) = res.as_array() {
            for s in arr {
                collect_symbol(s, 0, &mut out);
            }
        }
        Ok(out)
    }

    /// Rename the symbol at a position. Returns per-URI edits to apply.
    pub fn rename(
        &self,
        uri: &str,
        line: u32,
        character: u32,
        new_name: &str,
    ) -> Result<Vec<(String, Vec<TextEdit>)>> {
        let params = json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character },
            "newName": new_name
        });
        let res = self.request("textDocument/rename", params, Duration::from_secs(8))?;
        Ok(parse_workspace_edit(Some(&res)))
    }

    /// Request signature help at a position (function call hints).
    pub fn signature_help(
        &self,
        uri: &str,
        line: u32,
        character: u32,
    ) -> Result<Option<SignatureInfo>> {
        let params = json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character }
        });
        let res = self.request("textDocument/signatureHelp", params, Duration::from_secs(5))?;
        if res.is_null() {
            return Ok(None);
        }
        let sigs = match res["signatures"].as_array() {
            Some(s) if !s.is_empty() => s,
            _ => return Ok(None),
        };
        let active_sig = res["activeSignature"].as_u64().unwrap_or(0) as usize;
        let sig = sigs.get(active_sig).or_else(|| sigs.first()).unwrap();
        let label = sig["label"].as_str().unwrap_or("").to_string();

        let active_param = res["activeParameter"]
            .as_u64()
            .or_else(|| sig["activeParameter"].as_u64())
            .map(|v| v as usize);
        let active = active_param
            .and_then(|ap| sig["parameters"].as_array().and_then(|ps| ps.get(ap)))
            .and_then(|p| param_range(&p["label"], &label));

        Ok(Some(SignatureInfo { label, active }))
    }

    /// Request hover text at a position. Blocking; call off the UI thread.
    pub fn hover(&self, uri: &str, line: u32, character: u32) -> Result<Option<String>> {
        let params = json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character }
        });
        let res = self.request("textDocument/hover", params, Duration::from_secs(5))?;
        if res.is_null() {
            return Ok(None);
        }
        let hover: Hover = serde_json::from_value(res)?;
        Ok(Some(hover_to_string(hover.contents)))
    }

    /// Request inlay hints for lines `0..=end_line`. Returns `(line, character,
    /// label)` per hint. Blocking; call off the UI thread.
    pub fn inlay_hints(&self, uri: &str, end_line: u32) -> Result<Vec<(u32, u32, String)>> {
        let params = json!({
            "textDocument": { "uri": uri },
            "range": {
                "start": { "line": 0, "character": 0 },
                "end": { "line": end_line, "character": 0 }
            }
        });
        let res = self.request("textDocument/inlayHint", params, Duration::from_secs(5))?;
        let mut out = Vec::new();
        if let Some(arr) = res.as_array() {
            for h in arr {
                let Some(pos) = h.get("position") else {
                    continue;
                };
                let line = pos.get("line").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let ch = pos.get("character").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let label = match h.get("label") {
                    Some(Value::String(s)) => s.clone(),
                    Some(Value::Array(parts)) => parts
                        .iter()
                        .filter_map(|p| p.get("value").and_then(|v| v.as_str()))
                        .collect::<String>(),
                    _ => continue,
                };
                if !label.is_empty() {
                    out.push((line, ch, label.trim().to_string()));
                }
            }
        }
        Ok(out)
    }
}

/// Recursively flatten a `DocumentSymbol` (or `SymbolInformation`).
fn collect_symbol(s: &Value, depth: usize, out: &mut Vec<(String, i64, u32, u32, usize)>) {
    let Some(name) = s.get("name").and_then(|n| n.as_str()) else {
        return;
    };
    let kind = s.get("kind").and_then(|k| k.as_i64()).unwrap_or(0);
    // DocumentSymbol uses selectionRange/range; SymbolInformation uses location.range.
    let pos = s
        .get("selectionRange")
        .or_else(|| s.get("range"))
        .or_else(|| s.pointer("/location/range"));
    let (line, ch) = pos
        .map(|r| {
            (
                r["start"]["line"].as_u64().unwrap_or(0) as u32,
                r["start"]["character"].as_u64().unwrap_or(0) as u32,
            )
        })
        .unwrap_or((0, 0));
    out.push((name.to_string(), kind, line, ch, depth));
    if let Some(children) = s.get("children").and_then(|c| c.as_array()) {
        for c in children {
            collect_symbol(c, depth + 1, out);
        }
    }
}

/// Resolve a parameter label (string or `[start,end]` offsets) to a char range.
fn param_range(plabel: &Value, sig_label: &str) -> Option<(u32, u32)> {
    if let Some(arr) = plabel.as_array() {
        let a = arr.first()?.as_u64()? as u32;
        let b = arr.get(1)?.as_u64()? as u32;
        return Some((a, b));
    }
    if let Some(s) = plabel.as_str() {
        let idx = sig_label.find(s)? as u32;
        return Some((idx, idx + s.chars().count() as u32));
    }
    None
}

/// Parse a `WorkspaceEdit` (`changes` or `documentChanges`) into per-URI edits.
fn parse_workspace_edit(edit: Option<&Value>) -> Vec<(String, Vec<TextEdit>)> {
    let Some(edit) = edit else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if let Some(changes) = edit.get("changes").and_then(|c| c.as_object()) {
        for (uri, edits) in changes {
            if let Ok(te) = serde_json::from_value::<Vec<TextEdit>>(edits.clone()) {
                out.push((uri.clone(), te));
            }
        }
    } else if let Some(dc) = edit.get("documentChanges").and_then(|d| d.as_array()) {
        for change in dc {
            if let (Some(uri), Some(edits)) =
                (change["textDocument"]["uri"].as_str(), change.get("edits"))
            {
                if let Ok(te) = serde_json::from_value::<Vec<TextEdit>>(edits.clone()) {
                    out.push((uri.to_string(), te));
                }
            }
        }
    }
    out
}

/// Extract `(uri, line, character)` from a `Location` or `Location[]` value.
fn locations_from_value(res: &Value) -> Vec<(String, u32, u32)> {
    fn one(v: &Value) -> Option<(String, u32, u32)> {
        let uri = v["uri"].as_str()?;
        let line = v["range"]["start"]["line"].as_u64()? as u32;
        let ch = v["range"]["start"]["character"].as_u64()? as u32;
        Some((uri.to_string(), line, ch))
    }
    match res {
        Value::Array(arr) => arr.iter().filter_map(one).collect(),
        Value::Null => Vec::new(),
        v => one(v).into_iter().collect(),
    }
}

fn hover_to_string(contents: HoverContents) -> String {
    fn marked(m: MarkedString) -> String {
        match m {
            MarkedString::String(s) => s,
            MarkedString::LanguageString(ls) => ls.value,
        }
    }
    match contents {
        HoverContents::Scalar(m) => marked(m),
        HoverContents::Array(arr) => arr.into_iter().map(marked).collect::<Vec<_>>().join("\n\n"),
        HoverContents::Markup(mk) => mk.value,
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
            // EOF or error: the server exited. Fail any pending requests fast
            // instead of letting them wait for their timeouts.
            Ok(None) | Err(_) => {
                let mut map = pending.lock().unwrap();
                for (_, tx) in map.drain() {
                    let _ = tx.send(Err(Value::String("server exited".into())));
                }
                break;
            }
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
                    if let Ok(p) =
                        serde_json::from_value::<PublishDiagnosticsParams>(params.clone())
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
            let n = msg["params"]["items"]
                .as_array()
                .map(|a| a.len())
                .unwrap_or(0);
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

/// Convert a `file://` URI back to a filesystem path.
pub fn uri_to_path(uri: &str) -> std::path::PathBuf {
    let s = uri.strip_prefix("file://").unwrap_or(uri);
    std::path::PathBuf::from(s.replace("%20", " "))
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

#[cfg(test)]
mod tests {
    use super::{locations_from_value, path_to_uri, uri_to_path};
    use serde_json::json;
    use std::path::Path;

    #[test]
    fn uri_roundtrip() {
        let p = Path::new("/tmp/my project/main.rs");
        let uri = path_to_uri(p);
        assert_eq!(uri, "file:///tmp/my%20project/main.rs");
        assert_eq!(uri_to_path(&uri), p.to_path_buf());
    }

    #[test]
    fn parse_location_array() {
        let v = json!([
            { "uri": "file:///a.rs", "range": { "start": { "line": 3, "character": 5 }, "end": {"line":3,"character":9} } }
        ]);
        assert_eq!(
            locations_from_value(&v),
            vec![("file:///a.rs".to_string(), 3, 5)]
        );
    }

    #[test]
    fn parse_location_null() {
        assert!(locations_from_value(&serde_json::Value::Null).is_empty());
    }
}
