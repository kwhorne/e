//! A small, synchronous Debug Adapter Protocol (DAP) client over stdio.
//!
//! Sibling to [`e-lsp`](../e-lsp): same architecture (protocol client in its own
//! crate, background reader thread, id-correlated blocking requests) and the
//! *exact same wire framing* as LSP — `Content-Length` headers followed by a
//! JSON body. What differs is the message shape: DAP messages carry a `seq` and
//! a `type` of `request` / `response` / `event`, responses correlate by
//! `request_seq`, the protocol is event-heavy, and the adapter can send *reverse
//! requests* (e.g. `runInTerminal`) back to us that must be answered.
//!
//! This client speaks to a debug *adapter*, not directly to a debug engine. For
//! PHP that adapter is `vscode-php-debug` (run over Node) which translates DAP
//! into Xdebug's DBGp; for JS it's `js-debug`, for Rust `codelldb`, etc. So the
//! same client gets every DAP-capable language.
//!
//! Startup is caller-driven because DAP has a strict sequence:
//! `initialize` → wait for the `initialized` event → `setBreakpoints` /
//! `setExceptionBreakpoints` → `configurationDone` → `launch`/`attach`.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use crossbeam_channel::{bounded, Sender};
use serde_json::{json, Value};

/// An adapter-pushed event (`stopped`, `output`, `terminated`, …).
#[derive(Clone, Debug)]
pub struct DapEvent {
    pub event: String,
    pub body: Value,
}

/// Callback invoked (on the reader thread) for each DAP event.
pub type EventHandler = Box<dyn Fn(DapEvent) + Send>;

/// A frame in the call stack.
#[derive(Clone, Debug, PartialEq)]
pub struct StackFrame {
    pub id: i64,
    pub name: String,
    pub source_path: Option<String>,
    pub line: i64,
    pub column: i64,
}

/// A variable scope for a stack frame (Locals, Superglobals, …).
#[derive(Clone, Debug, PartialEq)]
pub struct Scope {
    pub name: String,
    pub variables_reference: i64,
    pub expensive: bool,
}

/// A single variable (or child of a structured value).
#[derive(Clone, Debug, PartialEq)]
pub struct Variable {
    pub name: String,
    pub value: String,
    pub type_name: Option<String>,
    /// Non-zero when the variable is expandable (fetch children with this ref).
    pub variables_reference: i64,
}

/// A running thread reported by the adapter (PHP has a single pseudo-thread).
#[derive(Clone, Debug, PartialEq)]
pub struct Thread {
    pub id: i64,
    pub name: String,
}

/// The adapter's verdict on a requested breakpoint.
#[derive(Clone, Debug, PartialEq)]
pub struct Breakpoint {
    pub verified: bool,
    pub line: Option<i64>,
    pub message: Option<String>,
}

type Pending = Arc<Mutex<HashMap<i64, Sender<Result<Value, Value>>>>>;

/// Thread-safe writer half of the adapter connection (a pipe or a socket).
type Writer = Arc<Mutex<Box<dyn Write + Send>>>;

pub struct DapClient {
    /// The adapter process, when we spawned one (kept alive; killed on drop).
    child: Option<Child>,
    writer: Writer,
    next_seq: AtomicI64,
    pending: Pending,
}

impl DapClient {
    /// Spawn a debug adapter and start the reader thread. Does *not* perform the
    /// DAP handshake — call [`DapClient::initialize`] and friends in the order
    /// the protocol requires (see the module docs).
    pub fn start(program: &str, args: &[&str], on_event: EventHandler) -> Result<Arc<Self>> {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("spawning debug adapter `{program}`"))?;

        let stdin = child.stdin.take().context("adapter stdin")?;
        let stdout = child.stdout.take().context("adapter stdout")?;
        drain_stderr(&mut child, program);

        Ok(Self::from_transport(
            Some(child),
            stdout,
            Box::new(stdin),
            on_event,
        ))
    }

    /// Connect to a debug adapter already listening on a TCP address (the model
    /// used by `vscode-js-debug` and `codelldb`, which run as DAP servers).
    pub fn connect_tcp(addr: impl ToSocketAddrs, on_event: EventHandler) -> Result<Arc<Self>> {
        let stream = TcpStream::connect(addr).context("connecting to debug adapter")?;
        let reader = stream.try_clone().context("cloning adapter socket")?;
        Ok(Self::from_transport(None, reader, Box::new(stream), on_event))
    }

    /// Spawn an adapter that runs as a DAP *server*, then connect to it over TCP
    /// once it is accepting connections. Keeps the server process alive.
    pub fn spawn_server_and_connect(
        program: &str,
        args: &[&str],
        addr: impl ToSocketAddrs + Clone,
        startup: Duration,
        on_event: EventHandler,
    ) -> Result<Arc<Self>> {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("spawning debug adapter server `{program}`"))?;
        drain_stderr(&mut child, program);
        if let Some(stdout) = child.stdout.take() {
            let name = program.to_string();
            thread::spawn(move || {
                let reader = BufReader::new(stdout);
                for line in reader.lines().map_while(Result::ok) {
                    eprintln!("[dap:{name}] {line}");
                }
            });
        }

        let deadline = Instant::now() + startup;
        let stream = loop {
            match TcpStream::connect(addr.clone()) {
                Ok(s) => break s,
                Err(_) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(50));
                }
                Err(e) => {
                    let _ = child.kill();
                    return Err(anyhow!("adapter server never accepted a connection: {e}"));
                }
            }
        };
        let reader = stream.try_clone().context("cloning adapter socket")?;
        Ok(Self::from_transport(
            Some(child),
            reader,
            Box::new(stream),
            on_event,
        ))
    }

    fn from_transport(
        child: Option<Child>,
        reader: impl Read + Send + 'static,
        writer: Box<dyn Write + Send>,
        on_event: EventHandler,
    ) -> Arc<Self> {
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let writer: Writer = Arc::new(Mutex::new(writer));

        let client = Arc::new(DapClient {
            child,
            writer: writer.clone(),
            next_seq: AtomicI64::new(1),
            pending: pending.clone(),
        });

        thread::spawn(move || {
            read_loop(reader, pending, writer, on_event);
        });

        client
    }

    fn send(&self, msg: &Value) -> Result<()> {
        let body = serde_json::to_vec(msg)?;
        let mut w = self.writer.lock().unwrap();
        write!(w, "Content-Length: {}\r\n\r\n", body.len())?;
        w.write_all(&body)?;
        w.flush()?;
        Ok(())
    }

    /// Send a request and block for the response. Returns the response `body`
    /// (or `Null`). Call off the UI thread.
    pub fn request(&self, command: &str, arguments: Value, timeout: Duration) -> Result<Value> {
        let seq = self.next_seq.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = bounded(1);
        self.pending.lock().unwrap().insert(seq, tx);

        let mut msg = json!({ "seq": seq, "type": "request", "command": command });
        if !arguments.is_null() {
            msg["arguments"] = arguments;
        }
        self.send(&msg)?;

        match rx.recv_timeout(timeout) {
            Ok(Ok(body)) => Ok(body),
            Ok(Err(err)) => {
                let msg = err
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("request failed");
                Err(anyhow!("DAP {command} failed: {msg}"))
            }
            Err(_) => {
                self.pending.lock().unwrap().remove(&seq);
                Err(anyhow!("DAP request `{command}` timed out"))
            }
        }
    }

    // --- Handshake -----------------------------------------------------------

    /// `initialize` — announce client capabilities, get the adapter's back.
    pub fn initialize(&self, adapter_id: &str) -> Result<Value> {
        self.request(
            "initialize",
            json!({
                "clientID": "e",
                "clientName": "e",
                "adapterID": adapter_id,
                "locale": "en",
                "linesStartAt1": true,
                "columnsStartAt1": true,
                "pathFormat": "path",
                "supportsRunInTerminalRequest": true,
                "supportsProgressReporting": false,
            }),
            Duration::from_secs(15),
        )
    }

    /// `launch` with an adapter-specific configuration blob.
    pub fn launch(&self, configuration: Value) -> Result<Value> {
        self.request("launch", configuration, Duration::from_secs(30))
    }

    /// `attach` with an adapter-specific configuration blob.
    pub fn attach(&self, configuration: Value) -> Result<Value> {
        self.request("attach", configuration, Duration::from_secs(30))
    }

    /// `configurationDone` — signals the end of the initial breakpoint setup.
    pub fn configuration_done(&self) -> Result<()> {
        self.request("configurationDone", Value::Null, Duration::from_secs(10))?;
        Ok(())
    }

    // --- Breakpoints ---------------------------------------------------------

    /// Set source breakpoints for a file, replacing any previous ones. Returns
    /// the adapter's verification for each requested line, in order.
    pub fn set_breakpoints(&self, source_path: &str, lines: &[u32]) -> Result<Vec<Breakpoint>> {
        let name = std::path::Path::new(source_path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let bps: Vec<Value> = lines.iter().map(|l| json!({ "line": l })).collect();
        let line_nums: Vec<Value> = lines.iter().map(|l| json!(l)).collect();
        let body = self.request(
            "setBreakpoints",
            json!({
                "source": { "path": source_path, "name": name },
                "breakpoints": bps,
                "lines": line_nums,
            }),
            Duration::from_secs(10),
        )?;
        Ok(parse_breakpoints(&body))
    }

    /// Configure which exception filters break execution (e.g. `["*"]`).
    pub fn set_exception_breakpoints(&self, filters: &[&str]) -> Result<()> {
        self.request(
            "setExceptionBreakpoints",
            json!({ "filters": filters }),
            Duration::from_secs(10),
        )?;
        Ok(())
    }

    // --- Execution control ---------------------------------------------------

    pub fn continue_(&self, thread_id: i64) -> Result<()> {
        self.request(
            "continue",
            json!({ "threadId": thread_id }),
            Duration::from_secs(10),
        )?;
        Ok(())
    }

    pub fn next(&self, thread_id: i64) -> Result<()> {
        self.request("next", json!({ "threadId": thread_id }), Duration::from_secs(10))?;
        Ok(())
    }

    pub fn step_in(&self, thread_id: i64) -> Result<()> {
        self.request("stepIn", json!({ "threadId": thread_id }), Duration::from_secs(10))?;
        Ok(())
    }

    pub fn step_out(&self, thread_id: i64) -> Result<()> {
        self.request("stepOut", json!({ "threadId": thread_id }), Duration::from_secs(10))?;
        Ok(())
    }

    pub fn pause(&self, thread_id: i64) -> Result<()> {
        self.request("pause", json!({ "threadId": thread_id }), Duration::from_secs(10))?;
        Ok(())
    }

    // --- Inspection ----------------------------------------------------------

    pub fn threads(&self) -> Result<Vec<Thread>> {
        let body = self.request("threads", Value::Null, Duration::from_secs(10))?;
        Ok(body["threads"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|t| {
                        Some(Thread {
                            id: t["id"].as_i64()?,
                            name: t["name"].as_str().unwrap_or("").to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    pub fn stack_trace(&self, thread_id: i64) -> Result<Vec<StackFrame>> {
        let body = self.request(
            "stackTrace",
            json!({ "threadId": thread_id, "startFrame": 0, "levels": 0 }),
            Duration::from_secs(10),
        )?;
        Ok(parse_stack_frames(&body))
    }

    pub fn scopes(&self, frame_id: i64) -> Result<Vec<Scope>> {
        let body = self.request(
            "scopes",
            json!({ "frameId": frame_id }),
            Duration::from_secs(10),
        )?;
        Ok(body["scopes"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|s| {
                        Some(Scope {
                            name: s["name"].as_str()?.to_string(),
                            variables_reference: s["variablesReference"].as_i64().unwrap_or(0),
                            expensive: s["expensive"].as_bool().unwrap_or(false),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    pub fn variables(&self, variables_reference: i64) -> Result<Vec<Variable>> {
        let body = self.request(
            "variables",
            json!({ "variablesReference": variables_reference }),
            Duration::from_secs(10),
        )?;
        Ok(parse_variables(&body))
    }

    /// Evaluate an expression in the context of a stack frame (REPL/watch).
    pub fn evaluate(&self, expression: &str, frame_id: Option<i64>) -> Result<String> {
        let mut args = json!({ "expression": expression, "context": "repl" });
        if let Some(fid) = frame_id {
            args["frameId"] = json!(fid);
        }
        let body = self.request("evaluate", args, Duration::from_secs(10))?;
        Ok(body["result"].as_str().unwrap_or("").to_string())
    }

    /// `disconnect` — ask the adapter to detach/terminate. Best-effort.
    pub fn disconnect(&self, terminate_debuggee: bool) {
        let _ = self.request(
            "disconnect",
            json!({ "terminateDebuggee": terminate_debuggee }),
            Duration::from_secs(5),
        );
    }
}

impl Drop for DapClient {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Forward an adapter's stderr to our stderr — invaluable when it misbehaves
/// (bad Node version, missing Xdebug, …).
fn drain_stderr(child: &mut Child, program: &str) {
    if let Some(stderr) = child.stderr.take() {
        let name = program.to_string();
        thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines().map_while(Result::ok) {
                eprintln!("[dap:{name}] {line}");
            }
        });
    }
}

fn parse_breakpoints(body: &Value) -> Vec<Breakpoint> {
    body["breakpoints"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|b| Breakpoint {
                    verified: b["verified"].as_bool().unwrap_or(false),
                    line: b["line"].as_i64(),
                    message: b["message"].as_str().map(|s| s.to_string()),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_stack_frames(body: &Value) -> Vec<StackFrame> {
    body["stackFrames"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|f| {
                    Some(StackFrame {
                        id: f["id"].as_i64()?,
                        name: f["name"].as_str().unwrap_or("").to_string(),
                        source_path: f["source"]["path"].as_str().map(|s| s.to_string()),
                        line: f["line"].as_i64().unwrap_or(0),
                        column: f["column"].as_i64().unwrap_or(0),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_variables(body: &Value) -> Vec<Variable> {
    body["variables"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| {
                    Some(Variable {
                        name: v["name"].as_str()?.to_string(),
                        value: v["value"].as_str().unwrap_or("").to_string(),
                        type_name: v["type"].as_str().map(|s| s.to_string()),
                        variables_reference: v["variablesReference"].as_i64().unwrap_or(0),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn read_loop(reader: impl Read, pending: Pending, writer: Writer, on_event: EventHandler) {
    let mut reader = BufReader::new(reader);
    let mut reply_seq = 1_000_000_i64;
    loop {
        let msg = match read_message(&mut reader) {
            Ok(Some(msg)) => msg,
            // EOF or error: the adapter exited. Fail pending requests fast
            // instead of letting them wait for their timeouts.
            Ok(None) | Err(_) => {
                let mut map = pending.lock().unwrap();
                for (_, tx) in map.drain() {
                    let _ = tx.send(Err(json!({ "message": "adapter exited" })));
                }
                break;
            }
        };

        match msg.get("type").and_then(|t| t.as_str()) {
            Some("response") => {
                let request_seq = msg["request_seq"].as_i64().unwrap_or(-1);
                if let Some(tx) = pending.lock().unwrap().remove(&request_seq) {
                    if msg["success"].as_bool().unwrap_or(false) {
                        let _ = tx.send(Ok(msg.get("body").cloned().unwrap_or(Value::Null)));
                    } else {
                        let err = json!({
                            "message": msg.get("message").cloned().unwrap_or(Value::Null),
                            "body": msg.get("body").cloned().unwrap_or(Value::Null),
                        });
                        let _ = tx.send(Err(err));
                    }
                }
            }
            Some("event") => {
                let event = msg["event"].as_str().unwrap_or("").to_string();
                let body = msg.get("body").cloned().unwrap_or(Value::Null);
                on_event(DapEvent { event, body });
            }
            Some("request") => {
                // Reverse request from the adapter (e.g. `runInTerminal`,
                // `startDebugging`). Reply so it doesn't stall; sensible empty
                // defaults are enough for step-debugging.
                reply_seq += 1;
                respond_to_reverse_request(&msg, reply_seq, &writer);
            }
            _ => {}
        }
    }
}

fn respond_to_reverse_request(msg: &Value, seq: i64, writer: &Writer) {
    let request_seq = msg["seq"].as_i64().unwrap_or(0);
    let command = msg["command"].as_str().unwrap_or("");
    let reply = json!({
        "seq": seq,
        "type": "response",
        "request_seq": request_seq,
        "success": true,
        "command": command,
        "body": {},
    });
    if let Ok(body) = serde_json::to_vec(&reply) {
        if let Ok(mut w) = writer.lock() {
            let _ = write!(w, "Content-Length: {}\r\n\r\n", body.len());
            let _ = w.write_all(&body);
            let _ = w.flush();
        }
    }
}

/// Read one DAP message (headers + JSON body) from `reader`. Framing is
/// identical to LSP: `Content-Length` header, blank line, then the body.
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

/// Frame a value the way [`DapClient::send`] does (for tests / tooling).
pub fn frame_message(msg: &Value) -> Vec<u8> {
    let body = serde_json::to_vec(msg).unwrap_or_default();
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend_from_slice(&body);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn frame_and_read_roundtrip() {
        let msg = json!({ "seq": 7, "type": "request", "command": "threads" });
        let bytes = frame_message(&msg);
        let mut cur = Cursor::new(bytes);
        let got = read_message(&mut cur).unwrap().unwrap();
        assert_eq!(got, msg);
    }

    #[test]
    fn reads_two_messages_from_one_stream() {
        let a = json!({ "type": "event", "event": "stopped", "body": { "threadId": 1 } });
        let b = json!({ "type": "response", "request_seq": 1, "success": true });
        let mut bytes = frame_message(&a);
        bytes.extend(frame_message(&b));
        let mut cur = Cursor::new(bytes);
        assert_eq!(read_message(&mut cur).unwrap().unwrap(), a);
        assert_eq!(read_message(&mut cur).unwrap().unwrap(), b);
        assert!(read_message(&mut cur).unwrap().is_none());
    }

    #[test]
    fn parses_stack_frames() {
        let body = json!({
            "stackFrames": [
                { "id": 3, "name": "{main}", "line": 12, "column": 1,
                  "source": { "path": "/app/index.php" } },
                { "id": 4, "name": "handle", "line": 5, "column": 2 }
            ]
        });
        let frames = parse_stack_frames(&body);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].source_path.as_deref(), Some("/app/index.php"));
        assert_eq!(frames[0].line, 12);
        assert_eq!(frames[1].source_path, None);
    }

    #[test]
    fn parses_variables_and_breakpoints() {
        let vars = parse_variables(&json!({
            "variables": [
                { "name": "$x", "value": "42", "type": "int", "variablesReference": 0 },
                { "name": "$arr", "value": "array(2)", "type": "array", "variablesReference": 9 }
            ]
        }));
        assert_eq!(vars.len(), 2);
        assert_eq!(vars[1].variables_reference, 9);
        assert_eq!(vars[0].type_name.as_deref(), Some("int"));

        let bps = parse_breakpoints(&json!({
            "breakpoints": [ { "verified": true, "line": 10 }, { "verified": false } ]
        }));
        assert_eq!(bps.len(), 2);
        assert!(bps[0].verified);
        assert_eq!(bps[0].line, Some(10));
        assert!(!bps[1].verified);
    }
}
