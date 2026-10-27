//! The Agent Client Protocol (ACP): one protocol for many coding agents —
//! Claude Code, Codex, Gemini CLI, Copilot, Cursor, OpenCode, Goose… — so
//! Cascade can run any of them with the same native panel it runs Elyra in.
//!
//! ACP is JSON-RPC 2.0 over stdio, one message per line. The client
//! `initialize`s, opens a session (`session/new`, with the working
//! directory), and sends `session/prompt`; the agent streams
//! `session/update` notifications (text, thoughts, tool calls and their
//! results, usage) until it answers the prompt with a stop reason. It asks
//! before acting with `session/request_permission`. `session/cancel` stops it.
//!
//! Here ACP is translated into the same [`AgentEvent`]s Elyra's RPC produces,
//! so the reducer, the transcript and everything built on them (turn
//! receipts, worktrees, the context meter) work unchanged. What ACP lacks is
//! made up on this side: a prompt sent while the agent works waits in a queue
//! (there is no steering), and the user's message is echoed into the stream
//! when it is sent, as Elyra echoes it.

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex};

use crossbeam_channel::Sender;
use serde_json::{json, Value};

use crate::protocol::{AgentEvent, PermissionChoice};

/// The protocol version spoken (`initialize`'s `protocolVersion`).
pub const PROTOCOL_VERSION: u64 = 1;

// ---- Translating session updates -------------------------------------------

/// What the agent is streaming at the moment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Stream {
    #[default]
    None,
    Message,
    Thought,
}

/// Turns `session/update` notifications into [`AgentEvent`]s. Pure, apart
/// from remembering whether a message is open, so it can be tested on the
/// shapes the specification gives.
#[derive(Debug, Default)]
pub struct Translator {
    stream: Stream,
    /// The session's working directory: paths under it are shown relative.
    cwd: String,
}

/// The text of a content block (`{"type":"text","text":…}`).
fn block_text(block: &Value) -> Option<String> {
    (block.get("type").and_then(Value::as_str) == Some("text"))
        .then(|| block.get("text").and_then(Value::as_str))
        .flatten()
        .map(str::to_string)
}

/// A tool call's content as text: its text blocks, and for a diff, which
/// file changed and by how many lines.
fn tool_content_text(content: &Value) -> Option<String> {
    let items = content.as_array()?;
    let mut out: Vec<String> = Vec::new();
    for item in items {
        match item.get("type").and_then(Value::as_str) {
            Some("content") => {
                if let Some(t) = item.get("content").and_then(block_text) {
                    out.push(t);
                }
            }
            Some("diff") => {
                let path = item.get("path").and_then(Value::as_str).unwrap_or("");
                let old = item.get("oldText").and_then(Value::as_str).unwrap_or("");
                let new = item.get("newText").and_then(Value::as_str).unwrap_or("");
                let (o, n) = (old.lines().count(), new.lines().count());
                out.push(format!("{path}: {n} line(s), was {o}"));
            }
            Some("terminal") => out.push("(terminal output)".into()),
            _ => {}
        }
    }
    (!out.is_empty()).then(|| out.join("\n"))
}

impl Translator {
    pub fn new() -> Self {
        Self::default()
    }

    /// A translator for a session in `cwd`.
    pub fn in_dir(cwd: &str) -> Self {
        Translator {
            cwd: cwd.trim_end_matches('/').to_string(),
            ..Self::default()
        }
    }

    /// `path` relative to the session's directory when it is under it —
    /// also through `/private`, which macOS puts in front of `/tmp`.
    fn relative(&self, path: &str) -> String {
        if self.cwd.is_empty() {
            return path.to_string();
        }
        for base in [self.cwd.clone(), format!("/private{}", self.cwd)] {
            if let Some(rest) = path.strip_prefix(&format!("{base}/")) {
                return rest.to_string();
            }
        }
        path.to_string()
    }

    /// Close a message that is streaming, before something else starts.
    fn close(&mut self, out: &mut Vec<AgentEvent>) {
        if self.stream == Stream::Message {
            out.push(AgentEvent::MessageEnd { text: None });
        }
        self.stream = Stream::None;
    }

    /// The events for one `session/update`'s `update`.
    pub fn update(&mut self, u: &Value) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        match u.get("sessionUpdate").and_then(Value::as_str).unwrap_or("") {
            "agent_message_chunk" => {
                if let Some(text) = u.get("content").and_then(block_text) {
                    if self.stream != Stream::Message {
                        self.stream = Stream::Message;
                        out.push(AgentEvent::MessageStart {
                            role: "assistant".into(),
                        });
                    }
                    out.push(AgentEvent::TextDelta { delta: text });
                }
            }
            "agent_thought_chunk" => {
                if let Some(text) = u.get("content").and_then(block_text) {
                    if self.stream == Stream::Message {
                        out.push(AgentEvent::MessageEnd { text: None });
                    }
                    self.stream = Stream::Thought;
                    out.push(AgentEvent::ReasoningDelta { delta: text });
                }
            }
            "tool_call" => {
                self.close(&mut out);
                let id = u
                    .get("toolCallId")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let kind = u
                    .get("kind")
                    .and_then(Value::as_str)
                    .unwrap_or("tool")
                    .to_string();
                let title = u
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                // What the card shows beside the kind: the file, the
                // command, or else the agent's own title for the call.
                let mut args = serde_json::Map::new();
                if let Some(path) = u
                    .get("locations")
                    .and_then(Value::as_array)
                    .and_then(|l| l.first())
                    .and_then(|l| l.get("path"))
                    .and_then(Value::as_str)
                {
                    args.insert("path".into(), Value::String(self.relative(path)));
                }
                if let Some(cmd) = u.get("rawInput").and_then(|r| r.get("command")) {
                    args.insert("command".into(), cmd.clone());
                }
                if args.is_empty() && !title.is_empty() {
                    args.insert("query".into(), Value::String(title.clone()));
                }
                args.insert("title".into(), Value::String(title));
                out.push(AgentEvent::ToolStart {
                    id: id.clone(),
                    name: kind.clone(),
                    args: Value::Object(args),
                });
                out.extend(self.tool_progress(&id, &kind, u));
            }
            "tool_call_update" => {
                let id = u
                    .get("toolCallId")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let kind = u
                    .get("kind")
                    .and_then(Value::as_str)
                    .unwrap_or("tool")
                    .to_string();
                out.extend(self.tool_progress(&id, &kind, u));
            }
            "usage_update" => {
                let used = u.get("used").and_then(Value::as_u64).unwrap_or(0);
                let size = u.get("size").and_then(Value::as_u64).unwrap_or(0);
                let cost = u
                    .get("cost")
                    .and_then(|c| c.get("amount"))
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0);
                let percent = (size > 0).then(|| used as f64 * 100.0 / size as f64);
                // As Elyra's session stats, which the reducer reads already.
                out.push(AgentEvent::Response {
                    id: None,
                    command: "get_session_stats".into(),
                    success: true,
                    data: json!({
                        "contextUsage": {"tokens": used, "contextWindow": size, "percent": percent},
                        "cost": cost,
                    }),
                });
            }
            "compaction_update" => match u.get("status").and_then(Value::as_str).unwrap_or("") {
                "started" | "in_progress" => out.push(AgentEvent::CompactionStart {
                    reason: "the agent".into(),
                }),
                "completed" => out.push(AgentEvent::CompactionEnd {
                    reason: String::new(),
                    aborted: false,
                }),
                "failed" | "cancelled" => out.push(AgentEvent::CompactionEnd {
                    reason: String::new(),
                    aborted: true,
                }),
                _ => {}
            },
            "notice" => {
                let title = u.get("title").and_then(Value::as_str).unwrap_or("");
                let more = u.get("description").and_then(Value::as_str).unwrap_or("");
                let error = u.get("severity").and_then(Value::as_str) == Some("error");
                let text = if more.is_empty() {
                    title.to_string()
                } else {
                    format!("{title}: {more}")
                };
                out.push(AgentEvent::Notice { text, error });
            }
            "plan" => {
                let entries: Vec<String> = u
                    .get("entries")
                    .and_then(Value::as_array)
                    .map(|e| {
                        e.iter()
                            .map(|x| {
                                let mark = match x.get("status").and_then(Value::as_str) {
                                    Some("completed") => "✓",
                                    Some("in_progress") => "▸",
                                    _ => "○",
                                };
                                let what = x.get("content").and_then(Value::as_str).unwrap_or("");
                                format!("{mark} {what}")
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                if !entries.is_empty() {
                    self.close(&mut out);
                    out.push(AgentEvent::Notice {
                        text: format!("Plan\n{}", entries.join("\n")),
                        error: false,
                    });
                }
            }
            // The user's own messages are echoed by the client when sent;
            // modes, commands and config options aren't shown yet.
            other => out.push(AgentEvent::Other {
                kind: other.to_string(),
                raw: u.clone(),
            }),
        }
        out
    }

    /// A tool call's status and output: running with output so far, or done.
    fn tool_progress(&self, id: &str, kind: &str, u: &Value) -> Vec<AgentEvent> {
        let text = u.get("content").and_then(tool_content_text).or_else(|| {
            u.get("rawOutput").map(|o| match o.as_str() {
                Some(s) => s.to_string(),
                None => o.to_string(),
            })
        });
        match u.get("status").and_then(Value::as_str) {
            Some(s @ ("completed" | "failed")) => vec![AgentEvent::ToolEnd {
                id: id.to_string(),
                name: kind.to_string(),
                result: Value::String(text.unwrap_or_default()),
                is_error: s == "failed",
            }],
            _ => match text {
                Some(t) => vec![AgentEvent::ToolUpdate {
                    id: id.to_string(),
                    name: kind.to_string(),
                    partial: json!({"content": [{"type": "text", "text": t}]}),
                }],
                None => Vec::new(),
            },
        }
    }

    /// The turn is over: close a message still open.
    pub fn finish(&mut self) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        self.close(&mut out);
        out
    }
}

/// A permission request's title and choices.
pub fn permission(params: &Value) -> (String, Vec<PermissionChoice>) {
    let title = params
        .get("toolCall")
        .and_then(|t| t.get("title"))
        .and_then(Value::as_str)
        .unwrap_or("The agent wants to go ahead")
        .to_string();
    let choices = params
        .get("options")
        .and_then(Value::as_array)
        .map(|o| {
            o.iter()
                .filter_map(|c| {
                    Some(PermissionChoice {
                        id: c.get("optionId")?.as_str()?.to_string(),
                        name: c.get("name")?.as_str()?.to_string(),
                        kind: c
                            .get("kind")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    (title, choices)
}

// ---- The connection ---------------------------------------------------------

/// What a request the client sent was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pending {
    Initialize,
    NewSession,
    Prompt,
}

#[derive(Default)]
struct State {
    next_id: u64,
    pending: HashMap<u64, Pending>,
    session: Option<String>,
    /// A prompt is being answered.
    busy: bool,
    /// Prompts waiting for the agent: sent while it worked, or before the
    /// session was open.
    queue: VecDeque<String>,
    /// Permission requests not yet answered, by their id as JSON.
    asking: Vec<String>,
}

/// One ACP connection: what both the client handle and the reader thread
/// share.
pub(crate) struct Acp {
    /// The agent's stdin (anything written to, in tests).
    stdin: Arc<Mutex<dyn Write + Send>>,
    tx: Sender<AgentEvent>,
    state: Mutex<State>,
    cwd: String,
    translator: Mutex<Translator>,
}

impl Acp {
    pub(crate) fn new(
        stdin: Arc<Mutex<dyn Write + Send>>,
        tx: Sender<AgentEvent>,
        cwd: String,
    ) -> Self {
        Acp {
            stdin,
            tx,
            state: Mutex::new(State::default()),
            translator: Mutex::new(Translator::in_dir(&cwd)),
            cwd,
        }
    }

    fn write(&self, msg: &Value) {
        let mut line = msg.to_string();
        line.push('\n');
        if let Ok(mut w) = self.stdin.lock() {
            let _ = w.write_all(line.as_bytes());
            let _ = w.flush();
        }
    }

    fn emit(&self, ev: AgentEvent) {
        let _ = self.tx.send(ev);
    }

    fn request(&self, method: &str, params: Value, kind: Pending) {
        let id = {
            let mut st = self.state.lock().expect("acp state");
            st.next_id += 1;
            let id = st.next_id;
            st.pending.insert(id, kind);
            id
        };
        self.write(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
    }

    /// Start the conversation: `initialize`; the reader opens the session
    /// when it answers.
    pub(crate) fn start(&self) {
        self.request(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                // The agent reads and writes files and runs commands itself.
                "clientCapabilities": {"fs": {"readTextFile": false, "writeTextFile": false}, "terminal": false},
                "clientInfo": {"name": "e", "version": env!("CARGO_PKG_VERSION")},
            }),
            Pending::Initialize,
        );
    }

    pub(crate) fn new_session(&self) {
        self.request(
            "session/new",
            json!({"cwd": self.cwd, "mcpServers": []}),
            Pending::NewSession,
        );
    }

    fn queue_update(&self, queue: &VecDeque<String>) {
        self.emit(AgentEvent::QueueUpdate {
            steering: Vec::new(),
            follow_up: queue.iter().cloned().collect(),
        });
    }

    /// Send `text` now if the agent is free, or queue it.
    pub(crate) fn prompt(&self, text: &str) {
        let mut st = self.state.lock().expect("acp state");
        if st.session.is_none() || st.busy {
            st.queue.push_back(text.to_string());
            let q = st.queue.clone();
            drop(st);
            self.queue_update(&q);
            return;
        }
        drop(st);
        self.dispatch(text.to_string());
    }

    fn dispatch(&self, text: String) {
        let session = {
            let mut st = self.state.lock().expect("acp state");
            st.busy = true;
            st.session.clone().unwrap_or_default()
        };
        self.emit(AgentEvent::AgentStart);
        // The user's message, as Elyra echoes it: a queued one joins the
        // transcript here, where the agent takes it up.
        self.emit(AgentEvent::MessageStart {
            role: "user".into(),
        });
        self.emit(AgentEvent::MessageEnd {
            text: Some(text.clone()),
        });
        self.request(
            "session/prompt",
            json!({"sessionId": session, "prompt": [{"type": "text", "text": text}]}),
            Pending::Prompt,
        );
    }

    /// The next queued prompt, if the agent is free for it.
    fn next(&self) {
        let text = {
            let mut st = self.state.lock().expect("acp state");
            if st.busy || st.session.is_none() {
                return;
            }
            st.queue.pop_front()
        };
        if let Some(text) = text {
            let q = self.state.lock().expect("acp state").queue.clone();
            self.queue_update(&q);
            self.dispatch(text);
        }
    }

    /// Stop the turn: `session/cancel`, and every question still open
    /// answered "cancelled", as the protocol asks. Queued prompts are dropped.
    pub(crate) fn cancel(&self) {
        let (session, asking) = {
            let mut st = self.state.lock().expect("acp state");
            st.queue.clear();
            (st.session.clone(), std::mem::take(&mut st.asking))
        };
        for id in asking {
            if let Ok(id) = serde_json::from_str::<Value>(&id) {
                self.write(&json!({"jsonrpc": "2.0", "id": id, "result": {"outcome": {"outcome": "cancelled"}}}));
            }
        }
        if let Some(session) = session {
            self.write(&json!({"jsonrpc": "2.0", "method": "session/cancel", "params": {"sessionId": session}}));
        }
        self.queue_update(&VecDeque::new());
    }

    /// Answer a permission request with one of its choices.
    pub(crate) fn answer(&self, request_id: &str, choice: &str) {
        self.state
            .lock()
            .expect("acp state")
            .asking
            .retain(|a| a != request_id);
        if let Ok(id) = serde_json::from_str::<Value>(request_id) {
            self.write(&json!({"jsonrpc": "2.0", "id": id, "result": {"outcome": {"outcome": "selected", "optionId": choice}}}));
        }
    }

    /// Handle one line from the agent.
    pub(crate) fn receive(&self, line: &str) {
        let Ok(msg) = serde_json::from_str::<Value>(line.trim()) else {
            return;
        };
        let method = msg.get("method").and_then(Value::as_str);
        let id = msg.get("id").cloned();
        match (method, id) {
            // A response to one of ours.
            (None, Some(id)) => {
                let kind = id
                    .as_u64()
                    .and_then(|n| self.state.lock().expect("acp state").pending.remove(&n));
                let error = msg.get("error").map(|e| {
                    e.get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("error")
                        .to_string()
                });
                match kind {
                    Some(Pending::Initialize) => match error {
                        Some(e) => self.emit(AgentEvent::Error {
                            message: format!("The agent didn't start: {e}"),
                        }),
                        None => self.new_session(),
                    },
                    Some(Pending::NewSession) => match error {
                        Some(e) => self.emit(AgentEvent::Error {
                            message: format!("The agent couldn't open a session: {e}"),
                        }),
                        None => {
                            let sid = msg
                                .pointer("/result/sessionId")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string();
                            self.state.lock().expect("acp state").session = Some(sid.clone());
                            self.emit(AgentEvent::Session {
                                id: sid,
                                cwd: self.cwd.clone(),
                            });
                            self.next();
                        }
                    },
                    Some(Pending::Prompt) => {
                        for ev in self.translator.lock().expect("acp translator").finish() {
                            self.emit(ev);
                        }
                        if let Some(e) = error {
                            self.emit(AgentEvent::Error { message: e });
                        } else if let Some(reason) =
                            msg.pointer("/result/stopReason").and_then(Value::as_str)
                        {
                            let said = match reason {
                                "max_tokens" => Some("The agent stopped: it reached its token limit"),
                                "max_turn_requests" => Some("The agent stopped: it reached its limit of steps for one prompt"),
                                "refusal" => Some("The agent declined to go on"),
                                _ => None,
                            };
                            if let Some(said) = said {
                                self.emit(AgentEvent::Notice {
                                    text: said.into(),
                                    error: true,
                                });
                            }
                        }
                        self.state.lock().expect("acp state").busy = false;
                        self.emit(AgentEvent::AgentEnd);
                        self.next();
                    }
                    None => {}
                }
            }
            // A request from the agent.
            (Some(method), Some(id)) => {
                if method == "session/request_permission" {
                    let params = msg.get("params").cloned().unwrap_or(Value::Null);
                    let (title, choices) = permission(&params);
                    let request_id = id.to_string();
                    self.state
                        .lock()
                        .expect("acp state")
                        .asking
                        .push(request_id.clone());
                    for ev in self.translator.lock().expect("acp translator").finish() {
                        self.emit(ev);
                    }
                    self.emit(AgentEvent::Permission {
                        request_id,
                        title,
                        choices,
                    });
                } else {
                    // Files and terminals stay with the agent: not offered.
                    self.write(&json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {"code": -32601, "message": format!("{method} is not supported")},
                    }));
                }
            }
            // A notification.
            (Some("session/update"), None) => {
                if let Some(u) = msg.pointer("/params/update") {
                    let events = self.translator.lock().expect("acp translator").update(u);
                    for ev in events {
                        self.emit(ev);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Read the agent's stdout line by line into `acp`.
pub(crate) fn pump<R: BufRead>(mut reader: R, acp: &Acp) {
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => acp.receive(&String::from_utf8_lossy(&buf)),
        }
    }
    acp.emit(AgentEvent::Error {
        message: "The agent has stopped".into(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ChatItem;

    fn t(v: Value) -> Vec<AgentEvent> {
        Translator::new().update(&v)
    }

    #[test]
    fn message_chunks_become_one_streaming_reply() {
        let mut tr = Translator::new();
        let mut out = tr.update(&json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "Hel"}}));
        out.extend(tr.update(&json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "lo"}})));
        out.extend(tr.finish());
        assert_eq!(
            out,
            vec![
                AgentEvent::MessageStart {
                    role: "assistant".into()
                },
                AgentEvent::TextDelta {
                    delta: "Hel".into()
                },
                AgentEvent::TextDelta { delta: "lo".into() },
                AgentEvent::MessageEnd { text: None },
            ]
        );
        // Nothing open, nothing to close.
        assert!(tr.finish().is_empty());
    }

    #[test]
    fn a_tool_call_closes_the_reply_and_ends_with_its_result() {
        let mut tr = Translator::new();
        tr.update(&json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "Let me edit it."}}));
        let start = tr.update(&json!({
            "sessionUpdate": "tool_call", "toolCallId": "c1", "title": "Edit notes.txt",
            "kind": "edit", "status": "pending", "locations": [{"path": "/w/notes.txt"}]
        }));
        assert_eq!(start[0], AgentEvent::MessageEnd { text: None });
        assert_eq!(
            start[1],
            AgentEvent::ToolStart {
                id: "c1".into(),
                name: "edit".into(),
                args: json!({"path": "/w/notes.txt", "title": "Edit notes.txt"}),
            }
        );
        let done = tr.update(&json!({
            "sessionUpdate": "tool_call_update", "toolCallId": "c1", "status": "completed",
            "content": [{"type": "diff", "path": "/w/notes.txt", "oldText": "hello\n", "newText": "hello\nworld\n"}]
        }));
        assert_eq!(
            done,
            vec![AgentEvent::ToolEnd {
                id: "c1".into(),
                name: "tool".into(),
                result: Value::String("/w/notes.txt: 2 line(s), was 1".into()),
                is_error: false,
            }]
        );
    }

    #[test]
    fn a_command_shows_its_output_while_it_runs_and_fails_as_an_error() {
        let start = t(json!({
            "sessionUpdate": "tool_call", "toolCallId": "c2", "title": "`php artisan test`",
            "kind": "execute", "status": "in_progress", "rawInput": {"command": "php artisan test"}
        }));
        assert!(
            matches!(&start[0], AgentEvent::ToolStart { args, .. } if args["command"] == "php artisan test")
        );
        let mut tr = Translator::new();
        let running = tr.update(&json!({
            "sessionUpdate": "tool_call_update", "toolCallId": "c2", "status": "in_progress",
            "content": [{"type": "content", "content": {"type": "text", "text": "PASS 3 tests"}}]
        }));
        assert!(
            matches!(&running[0], AgentEvent::ToolUpdate { partial, .. } if partial["content"][0]["text"] == "PASS 3 tests")
        );
        let failed = tr.update(&json!({"sessionUpdate": "tool_call_update", "toolCallId": "c2", "status": "failed", "rawOutput": "exit 1"}));
        assert!(
            matches!(&failed[0], AgentEvent::ToolEnd { is_error: true, result, .. } if result == "exit 1")
        );
    }

    #[test]
    fn usage_becomes_the_context_meter_and_a_plan_a_notice() {
        let usage = t(
            json!({"sessionUpdate": "usage_update", "used": 50000, "size": 200000, "cost": {"amount": 0.42, "currency": "USD"}}),
        );
        let mut chat = crate::ChatState::new();
        for ev in usage {
            chat.apply(ev);
        }
        assert_eq!(chat.usage_label().as_deref(), Some("25% context · $0.42"));
        let plan = t(json!({"sessionUpdate": "plan", "entries": [
            {"content": "Read the controller", "priority": "high", "status": "completed"},
            {"content": "Add the rule", "priority": "high", "status": "in_progress"}
        ]}));
        assert_eq!(
            plan,
            vec![AgentEvent::Notice {
                text: "Plan\n✓ Read the controller\n▸ Add the rule".into(),
                error: false
            }]
        );
    }

    #[test]
    fn a_permission_request_gives_its_title_and_choices() {
        let (title, choices) = permission(&json!({
            "sessionId": "s",
            "toolCall": {"toolCallId": "c3", "title": "Run `git push`"},
            "options": [
                {"optionId": "a", "name": "Allow once", "kind": "allow_once"},
                {"optionId": "r", "name": "Reject", "kind": "reject_once"}
            ]
        }));
        assert_eq!(title, "Run `git push`");
        assert_eq!(choices.len(), 2);
        assert_eq!(choices[1].kind, "reject_once");
    }

    /// What e wrote to the agent, one JSON message per line.
    #[derive(Clone, Default)]
    struct Wire(Arc<Mutex<Vec<u8>>>);
    impl Write for Wire {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl Wire {
        fn sent(&self) -> Vec<Value> {
            let text = String::from_utf8(std::mem::take(&mut *self.0.lock().unwrap())).unwrap();
            text.lines()
                .map(|l| serde_json::from_str(l).unwrap())
                .collect()
        }
    }

    #[test]
    fn a_session_opens_a_prompt_is_asked_answered_and_the_next_one_waits_its_turn() {
        let wire = Wire::default();
        let (tx, rx) = crossbeam_channel::unbounded();
        let acp = Acp::new(Arc::new(Mutex::new(wire.clone())), tx, "/w".into());
        assert_eq!(
            Translator::in_dir("/tmp/w").relative("/private/tmp/w/app/X.php"),
            "app/X.php"
        );
        assert_eq!(
            Translator::in_dir("/tmp/w").relative("/etc/hosts"),
            "/etc/hosts"
        );
        acp.start();
        let init = wire.sent();
        assert_eq!(init[0]["method"], "initialize");
        assert_eq!(init[0]["params"]["protocolVersion"], 1);
        // A prompt before the session is open waits for it.
        acp.prompt("first");
        acp.receive(
            &json!({"jsonrpc": "2.0", "id": init[0]["id"], "result": {"protocolVersion": 1}})
                .to_string(),
        );
        let new = wire.sent();
        assert_eq!(new[0]["method"], "session/new");
        assert_eq!(new[0]["params"]["cwd"], "/w");
        acp.receive(
            &json!({"jsonrpc": "2.0", "id": new[0]["id"], "result": {"sessionId": "s1"}})
                .to_string(),
        );
        let prompt = wire.sent();
        assert_eq!(prompt[0]["method"], "session/prompt");
        assert_eq!(prompt[0]["params"]["sessionId"], "s1");
        assert_eq!(prompt[0]["params"]["prompt"][0]["text"], "first");

        // While it works, a second prompt queues.
        acp.prompt("second");
        assert!(wire.sent().is_empty());
        // It asks before acting; the answer goes back on the request's id.
        acp.receive(
            &json!({"jsonrpc": "2.0", "id": 7, "method": "session/request_permission", "params": {
                "sessionId": "s1", "toolCall": {"toolCallId": "c1", "title": "Edit notes.txt"},
                "options": [{"optionId": "ok", "name": "Allow once", "kind": "allow_once"}]
            }})
            .to_string(),
        );
        acp.answer("7", "ok");
        let answer = wire.sent();
        assert_eq!(answer[0]["id"], 7);
        assert_eq!(answer[0]["result"]["outcome"]["optionId"], "ok");
        // A request e doesn't offer is refused, not left hanging.
        acp.receive(
            &json!({"jsonrpc": "2.0", "id": 8, "method": "fs/read_text_file", "params": {}})
                .to_string(),
        );
        assert_eq!(wire.sent()[0]["error"]["code"], -32601);

        // The turn ends: the queued prompt goes out.
        acp.receive(&json!({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": "s1",
            "update": {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "Done."}}}}).to_string());
        acp.receive(
            &json!({"jsonrpc": "2.0", "id": prompt[0]["id"], "result": {"stopReason": "end_turn"}})
                .to_string(),
        );
        let next = wire.sent();
        assert_eq!(next[0]["params"]["prompt"][0]["text"], "second");

        // What the panel was told, folded as the panel folds it.
        let mut chat = crate::ChatState::new();
        // As the panel does: the first shown when sent, the second when the
        // agent takes it up.
        chat.send("first", "first", false);
        chat.send("second", "second", true);
        for ev in rx.try_iter() {
            chat.apply(ev);
        }
        let kinds: Vec<&str> = chat
            .items
            .iter()
            .map(|i| match i {
                ChatItem::User { .. } => "user",
                ChatItem::Assistant { .. } => "reply",
                ChatItem::Permission(_) => "permission",
                _ => "other",
            })
            .collect();
        assert_eq!(kinds, ["user", "permission", "reply", "user"]);
        assert!(chat.running, "the second prompt is under way");

        // Stopping cancels the session's turn.
        acp.cancel();
        let cancel = wire.sent();
        assert_eq!(cancel[0]["method"], "session/cancel");
        assert_eq!(cancel[0]["params"]["sessionId"], "s1");
    }
}
