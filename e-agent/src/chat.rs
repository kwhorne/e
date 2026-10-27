//! The reducer: folds a stream of [`AgentEvent`]s into an ordered list of
//! [`ChatItem`]s that a UI can render directly. This is pure and synchronous, so
//! the entire conversation model is unit-testable without a process or a GUI.

use serde_json::Value;

use crate::protocol::AgentEvent;

/// Lifecycle of a single tool call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolStatus {
    Running,
    Done,
    Error,
}

/// One rendered tool call (a card in the transcript).
#[derive(Clone, Debug, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// A short one-line summary of the arguments (e.g. the path being read).
    pub summary: String,
    pub args: Value,
    pub status: ToolStatus,
    /// A trimmed textual preview of the result (or error).
    pub result: Option<String>,
}

/// What one turn of the agent changed in the working tree — the files, with
/// line counts — between two snapshots of it, so it can be reviewed or undone
/// as a unit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TurnReceipt {
    /// The working tree before the turn and after it, as tree objects.
    pub before: String,
    pub after: String,
    /// `(path, lines added, lines removed)`.
    pub files: Vec<(String, u32, u32)>,
    /// How many review flags (warnings and worse) the change raises.
    pub flags: usize,
    /// Once undone: what the undo said.
    pub undone: Option<String>,
}

impl TurnReceipt {
    /// `3 files · +42 −7`.
    pub fn summary(&self) -> String {
        let n = self.files.len();
        let added: u32 = self.files.iter().map(|f| f.1).sum();
        let removed: u32 = self.files.iter().map(|f| f.2).sum();
        format!(
            "{n} {} · +{added} −{removed}",
            if n == 1 { "file" } else { "files" }
        )
    }
}

/// One item in the transcript, in event order.
#[derive(Clone, Debug, PartialEq)]
pub enum ChatItem {
    User {
        text: String,
    },
    Assistant {
        text: String,
        streaming: bool,
    },
    Reasoning {
        text: String,
        streaming: bool,
    },
    Tool(ToolCall),
    Notice {
        text: String,
        error: bool,
    },
    /// What the turn that just ended changed.
    Receipt(TurnReceipt),
    /// The agent asks before going ahead.
    Permission(PermissionAsk),
}

/// A question the agent asked before acting, and how it was answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionAsk {
    /// To answer it by (the request's JSON-RPC id, as JSON).
    pub request_id: String,
    pub title: String,
    pub choices: Vec<crate::protocol::PermissionChoice>,
    /// The choice made (its name), once answered.
    pub answered: Option<String>,
}

/// Maximum characters kept for a tool-result preview.
const RESULT_PREVIEW_LIMIT: usize = 2000;

/// One model the agent can run on, as elyra describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelInfo {
    /// elyra's provider id: `anthropic`, `openai`, `google`, `xai`, …
    pub provider: String,
    pub id: String,
    pub name: String,
    /// Whether the model has a thinking level to set.
    pub reasoning: bool,
}

impl ModelInfo {
    fn from_value(v: &Value) -> Option<ModelInfo> {
        let id = v.get("id")?.as_str()?.to_string();
        let provider = v
            .get("provider")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let name = v
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| id.clone());
        Some(ModelInfo {
            provider,
            id,
            name,
            reasoning: v.get("reasoning").and_then(Value::as_bool).unwrap_or(false),
        })
    }
}

/// How full the model's context window is.
#[derive(Clone, Debug, PartialEq)]
pub struct ContextUsage {
    /// Tokens in the context; unknown right after a compaction, until the
    /// next reply.
    pub tokens: Option<u64>,
    pub window: u64,
    /// Percent of the window in use, when `tokens` is known.
    pub percent: Option<f64>,
}

/// A message sent while the agent was busy, or one waiting for elyra's echo.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Sent {
    /// What went to the agent (an Ask-mode message carries a preamble).
    sent: String,
    /// What the transcript shows.
    shown: String,
    /// Queued behind the running work: shown in the transcript only once the
    /// agent takes it up.
    queued: bool,
}

/// The running conversation state.
#[derive(Clone, Debug, Default)]
pub struct ChatState {
    pub items: Vec<ChatItem>,
    /// True while the agent is actively working on the current prompt.
    pub running: bool,
    /// Every model the agent offers (`get_available_models`).
    pub models: Vec<ModelInfo>,
    /// The model in use, once the agent has told us (`get_state`/`set_model`).
    pub model: Option<ModelInfo>,
    /// The thinking level in use (`off` … `xhigh`), empty until known.
    pub thinking: String,
    pub session_id: String,
    /// The session file on disk, when the session is persisted.
    pub session_file: Option<String>,
    /// Messages waiting to steer the running work (delivered after its current
    /// tool calls), as the transcript would show them.
    pub steering: Vec<String>,
    /// Messages waiting for the agent to finish, to run next.
    pub follow_up: Vec<String>,
    /// Sent messages not yet echoed back by the agent.
    sent: Vec<Sent>,
    /// How full the context window is, as of the last `get_session_stats`.
    pub context: Option<ContextUsage>,
    /// What the session has cost so far, in dollars.
    pub cost: f64,
    /// Index of the assistant item currently receiving text deltas.
    cur_assistant: Option<usize>,
    /// Index of the reasoning item currently receiving deltas.
    cur_reasoning: Option<usize>,
    /// Role of the message elyra is currently emitting. elyra echoes the user's
    /// own message as a `message_start`/`message_end` pair too; that one is
    /// already in the transcript (pushed on send) and must not become a reply.
    cur_role: String,
}

impl ChatState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append the user's message immediately (the UI calls this on send, before
    /// the agent has echoed anything).
    pub fn push_user(&mut self, text: impl Into<String>) {
        self.items.push(ChatItem::User { text: text.into() });
        self.cur_assistant = None;
        self.cur_reasoning = None;
        self.running = true;
    }

    /// Record a message sent to the agent: `sent` is what went over the wire,
    /// `shown` what the transcript says. Sent while the agent is idle, it is
    /// shown at once; `queued` behind running work, it waits in
    /// [`ChatState::steering`] or [`ChatState::follow_up`] and joins the
    /// transcript where the agent takes it up.
    pub fn send(&mut self, shown: &str, sent: &str, queued: bool) {
        if !queued {
            self.push_user(shown);
        }
        self.sent.push(Sent {
            sent: sent.to_string(),
            shown: shown.to_string(),
            queued,
        });
    }

    /// Close a turn with what it changed.
    pub fn push_receipt(&mut self, receipt: TurnReceipt) {
        self.items.push(ChatItem::Receipt(receipt));
        self.cur_assistant = None;
        self.cur_reasoning = None;
    }

    /// Record the answer to the permission request at `index`.
    pub fn mark_answered(&mut self, index: usize, choice: String) {
        if let Some(ChatItem::Permission(p)) = self.items.get_mut(index) {
            p.answered = Some(choice);
        }
    }

    /// Mark the receipt at `index` undone, with what the undo said.
    pub fn mark_undone(&mut self, index: usize, said: String) {
        if let Some(ChatItem::Receipt(r)) = self.items.get_mut(index) {
            r.undone = Some(said);
        }
    }

    /// `42% context · $0.14`: how full the context is and what the session
    /// has cost, for beside the composer. `None` before anything is known or
    /// spent.
    pub fn usage_label(&self) -> Option<String> {
        // An empty context says nothing worth showing.
        let context = self
            .context
            .as_ref()
            .and_then(|c| c.percent)
            .filter(|p| *p > 0.0)
            .map(|p| {
                if p < 1.0 {
                    "<1% context".to_string()
                } else {
                    format!("{p:.0}% context")
                }
            });
        let cost = (self.cost > 0.0).then(|| {
            if self.cost < 0.01 {
                "<$0.01".to_string()
            } else {
                format!("${:.2}", self.cost)
            }
        });
        match (context, cost) {
            (Some(c), Some(d)) => Some(format!("{c} · {d}")),
            (one, other) => one.or(other),
        }
    }

    /// How a queued message reads in the transcript.
    fn shown_for(&self, sent: &str) -> String {
        self.sent
            .iter()
            .find(|s| s.sent == sent)
            .map(|s| s.shown.clone())
            .unwrap_or_else(|| sent.to_string())
    }

    /// Fold one event into the transcript.
    pub fn apply(&mut self, ev: AgentEvent) {
        match ev {
            AgentEvent::Session { id, .. } => self.session_id = id,
            AgentEvent::TurnStart | AgentEvent::TurnEnd => {}
            AgentEvent::AgentStart => self.running = true,
            AgentEvent::MessageStart { role } => {
                // A fresh turn: subsequent deltas start a new bubble.
                self.cur_assistant = None;
                self.cur_reasoning = None;
                self.cur_role = role;
            }
            AgentEvent::TextDelta { delta } => {
                let idx = match self.cur_assistant {
                    Some(i) => i,
                    None => {
                        self.items.push(ChatItem::Assistant {
                            text: String::new(),
                            streaming: true,
                        });
                        let i = self.items.len() - 1;
                        self.cur_assistant = Some(i);
                        i
                    }
                };
                if let Some(ChatItem::Assistant { text, .. }) = self.items.get_mut(idx) {
                    text.push_str(&delta);
                }
            }
            AgentEvent::ReasoningDelta { delta } => {
                let idx = match self.cur_reasoning {
                    Some(i) => i,
                    None => {
                        self.items.push(ChatItem::Reasoning {
                            text: String::new(),
                            streaming: true,
                        });
                        let i = self.items.len() - 1;
                        self.cur_reasoning = Some(i);
                        i
                    }
                };
                if let Some(ChatItem::Reasoning { text, .. }) = self.items.get_mut(idx) {
                    text.push_str(&delta);
                }
            }
            AgentEvent::MessageEnd { text } => {
                // The echo of the user's message ends here. One sent while
                // the agent was busy joins the transcript now, where it was
                // taken up; one sent while idle is there already.
                if self.cur_role == "user" {
                    self.cur_role.clear();
                    let text = text.unwrap_or_default();
                    if let Some(at) = self.sent.iter().position(|s| s.sent == text) {
                        let s = self.sent.remove(at);
                        if s.queued {
                            self.items.push(ChatItem::User { text: s.shown });
                            self.cur_assistant = None;
                            self.cur_reasoning = None;
                        }
                    }
                    return;
                }
                // A tool's result arrives as a message of its own (`toolResult`);
                // its card shows it already. Only the assistant's are replies.
                if !self.cur_role.is_empty() && self.cur_role != "assistant" {
                    self.cur_role.clear();
                    return;
                }
                // Reconcile with the authoritative final text and stop streaming.
                if let Some(final_text) = text {
                    match self.cur_assistant {
                        Some(i) => {
                            if let Some(ChatItem::Assistant { text: t, streaming }) =
                                self.items.get_mut(i)
                            {
                                if !final_text.is_empty() {
                                    *t = final_text;
                                }
                                *streaming = false;
                            }
                        }
                        None if !final_text.is_empty() => {
                            // Non-streamed message delivered whole.
                            self.items.push(ChatItem::Assistant {
                                text: final_text,
                                streaming: false,
                            });
                        }
                        None => {}
                    }
                } else if let Some(i) = self.cur_assistant {
                    if let Some(ChatItem::Assistant { streaming, .. }) = self.items.get_mut(i) {
                        *streaming = false;
                    }
                }
                self.stop_reasoning();
                self.cur_assistant = None;
                self.cur_reasoning = None;
            }
            AgentEvent::ToolStart { id, name, args } => {
                let summary = summarize_args(&name, &args);
                self.items.push(ChatItem::Tool(ToolCall {
                    id,
                    name,
                    summary,
                    args,
                    status: ToolStatus::Running,
                    result: None,
                }));
                // Any text bubble is complete once tools begin.
                self.cur_assistant = None;
            }
            AgentEvent::ToolArgs { id, args } => {
                if let Some(ChatItem::Tool(tc)) = self
                    .items
                    .iter_mut()
                    .rev()
                    .find(|it| matches!(it, ChatItem::Tool(tc) if tc.id == id))
                {
                    // Added to what was known: a later update may give the
                    // title alone.
                    if let (Some(old), Some(new)) = (tc.args.as_object_mut(), args.as_object()) {
                        for (k, v) in new {
                            if k != "query" || !old.contains_key("path") {
                                old.insert(k.clone(), v.clone());
                            }
                        }
                    } else {
                        tc.args = args;
                    }
                    tc.summary = summarize_args(&tc.name, &tc.args);
                }
            }
            AgentEvent::ToolUpdate { id, partial, .. } => {
                // Output so far, while the tool runs: the latest of it.
                if let Some(ChatItem::Tool(tc)) = self
                    .items
                    .iter_mut()
                    .rev()
                    .find(|it| matches!(it, ChatItem::Tool(tc) if tc.id == id))
                {
                    if tc.status == ToolStatus::Running {
                        let text = extract_text(&partial).unwrap_or_default();
                        if !text.is_empty() {
                            tc.result = Some(truncate_head(&text, RESULT_PREVIEW_LIMIT));
                        }
                    }
                }
            }
            AgentEvent::ToolEnd {
                id,
                result,
                is_error,
                ..
            } => {
                if let Some(ChatItem::Tool(tc)) = self
                    .items
                    .iter_mut()
                    .rev()
                    .find(|it| matches!(it, ChatItem::Tool(tc) if tc.id == id))
                {
                    tc.status = if is_error {
                        ToolStatus::Error
                    } else {
                        ToolStatus::Done
                    };
                    tc.result = Some(preview_result(&result));
                }
            }
            AgentEvent::CompactionStart { reason } => self.items.push(ChatItem::Notice {
                text: format!("Compacting context ({reason})…"),
                error: false,
            }),
            AgentEvent::CompactionEnd { aborted, .. } => {
                if aborted {
                    self.items.push(ChatItem::Notice {
                        text: "Compaction aborted".into(),
                        error: true,
                    });
                }
            }
            AgentEvent::RetryStart {
                attempt,
                max_attempts,
                error,
                ..
            } => self.items.push(ChatItem::Notice {
                text: format!("Retrying ({attempt}/{max_attempts}): {error}"),
                error: true,
            }),
            AgentEvent::RetryEnd { .. } => {}
            AgentEvent::Permission {
                request_id,
                title,
                choices,
            } => {
                self.items.push(ChatItem::Permission(PermissionAsk {
                    request_id,
                    title,
                    choices,
                    answered: None,
                }));
                self.cur_assistant = None;
                self.cur_reasoning = None;
            }
            AgentEvent::Notice { text, error } => {
                self.items.push(ChatItem::Notice { text, error });
                self.cur_assistant = None;
                self.cur_reasoning = None;
            }
            AgentEvent::QueueUpdate {
                steering,
                follow_up,
            } => {
                self.steering = steering.iter().map(|q| self.shown_for(q)).collect();
                self.follow_up = follow_up.iter().map(|q| self.shown_for(q)).collect();
            }
            AgentEvent::Response {
                command,
                success,
                data,
                ..
            } => {
                if success {
                    self.apply_response(&command, &data);
                }
            }
            AgentEvent::AgentEnd => {
                self.running = false;
                // A question still open when the turn ends (it was stopped)
                // can't be answered any more.
                for it in self.items.iter_mut() {
                    if let ChatItem::Permission(p) = it {
                        if p.answered.is_none() {
                            p.answered = Some("not answered: the turn ended".into());
                        }
                    }
                }
                self.stop_all_streaming();
                self.cur_assistant = None;
                self.cur_reasoning = None;
            }
            AgentEvent::Error { message } => {
                self.running = false;
                self.items.push(ChatItem::Notice {
                    text: message,
                    error: true,
                });
            }
            AgentEvent::Other { .. } => {}
        }
    }

    /// Fold the data a command answered with: the model list, the state, a
    /// model change, or a whole conversation (`get_messages`, for a resumed
    /// session).
    fn apply_response(&mut self, command: &str, data: &Value) {
        match command {
            "get_available_models" => {
                if let Some(list) = data.get("models").and_then(Value::as_array) {
                    self.models = list.iter().filter_map(ModelInfo::from_value).collect();
                }
            }
            "get_state" => {
                if let Some(m) = data.get("model").and_then(ModelInfo::from_value) {
                    self.model = Some(m);
                }
                if let Some(t) = data.get("thinkingLevel").and_then(Value::as_str) {
                    self.thinking = t.to_string();
                }
                if let Some(id) = data.get("sessionId").and_then(Value::as_str) {
                    self.session_id = id.to_string();
                }
                self.session_file = data
                    .get("sessionFile")
                    .and_then(Value::as_str)
                    .map(str::to_string);
            }
            "get_session_stats" => {
                self.cost = data.get("cost").and_then(Value::as_f64).unwrap_or(0.0);
                self.context = data.get("contextUsage").map(|u| ContextUsage {
                    tokens: u.get("tokens").and_then(Value::as_u64),
                    window: u.get("contextWindow").and_then(Value::as_u64).unwrap_or(0),
                    percent: u.get("percent").and_then(Value::as_f64),
                });
            }
            "set_model" => {
                if let Some(m) = ModelInfo::from_value(data) {
                    self.model = Some(m);
                }
            }
            "cycle_model" => {
                if let Some(m) = data.get("model").and_then(ModelInfo::from_value) {
                    self.model = Some(m);
                }
                if let Some(t) = data.get("thinkingLevel").and_then(Value::as_str) {
                    self.thinking = t.to_string();
                }
            }
            "cycle_thinking_level" => {
                if let Some(t) = data.get("level").and_then(Value::as_str) {
                    self.thinking = t.to_string();
                }
            }
            "get_messages" => {
                if let Some(list) = data.get("messages").and_then(Value::as_array) {
                    self.load_messages(list);
                }
            }
            _ => {}
        }
    }

    /// Replace the transcript with a stored conversation (elyra's
    /// `AgentMessage` objects): user text, assistant text and tool calls, and
    /// the tool results that answer them.
    pub fn load_messages(&mut self, messages: &[Value]) {
        self.items.clear();
        self.cur_assistant = None;
        self.cur_reasoning = None;
        for m in messages {
            let role = m.get("role").and_then(Value::as_str).unwrap_or("");
            match role {
                "user" => {
                    let text = extract_text(m).unwrap_or_default();
                    if !text.trim().is_empty() {
                        self.items.push(ChatItem::User { text });
                    }
                }
                "assistant" => {
                    let Some(blocks) = m.get("content").and_then(Value::as_array) else {
                        if let Some(text) = m.get("content").and_then(Value::as_str) {
                            self.items.push(ChatItem::Assistant {
                                text: text.to_string(),
                                streaming: false,
                            });
                        }
                        continue;
                    };
                    let mut text = String::new();
                    for b in blocks {
                        match b.get("type").and_then(Value::as_str) {
                            Some("text") => {
                                text.push_str(b.get("text").and_then(Value::as_str).unwrap_or(""))
                            }
                            Some("toolCall") => {
                                if !text.trim().is_empty() {
                                    self.items.push(ChatItem::Assistant {
                                        text: std::mem::take(&mut text),
                                        streaming: false,
                                    });
                                }
                                let name = b
                                    .get("name")
                                    .and_then(Value::as_str)
                                    .unwrap_or("")
                                    .to_string();
                                let args = b.get("arguments").cloned().unwrap_or(Value::Null);
                                self.items.push(ChatItem::Tool(ToolCall {
                                    id: b
                                        .get("id")
                                        .and_then(Value::as_str)
                                        .unwrap_or("")
                                        .to_string(),
                                    summary: summarize_args(&name, &args),
                                    name,
                                    args,
                                    status: ToolStatus::Done,
                                    result: None,
                                }));
                            }
                            _ => {}
                        }
                    }
                    if !text.trim().is_empty() {
                        self.items.push(ChatItem::Assistant {
                            text,
                            streaming: false,
                        });
                    }
                }
                "toolResult" => {
                    let id = m.get("toolCallId").and_then(Value::as_str).unwrap_or("");
                    let is_error = m.get("isError").and_then(Value::as_bool).unwrap_or(false);
                    if let Some(ChatItem::Tool(tc)) = self
                        .items
                        .iter_mut()
                        .rev()
                        .find(|it| matches!(it, ChatItem::Tool(tc) if tc.id == id))
                    {
                        tc.status = if is_error {
                            ToolStatus::Error
                        } else {
                            ToolStatus::Done
                        };
                        tc.result = Some(preview_result(m));
                    }
                }
                _ => {}
            }
        }
    }

    fn stop_reasoning(&mut self) {
        if let Some(i) = self.cur_reasoning {
            if let Some(ChatItem::Reasoning { streaming, .. }) = self.items.get_mut(i) {
                *streaming = false;
            }
        }
    }

    fn stop_all_streaming(&mut self) {
        for it in &mut self.items {
            match it {
                ChatItem::Assistant { streaming, .. } | ChatItem::Reasoning { streaming, .. } => {
                    *streaming = false
                }
                _ => {}
            }
        }
    }
}

/// A short, human one-liner describing a tool call's arguments.
fn summarize_args(name: &str, args: &Value) -> String {
    let first_str = |keys: &[&str]| -> Option<String> {
        keys.iter()
            .find_map(|k| args.get(*k).and_then(Value::as_str))
            .map(str::to_string)
    };
    match name {
        "read" | "write" | "edit" => first_str(&["path", "file", "file_path"]).unwrap_or_default(),
        "bash" | "shell" => first_str(&["command", "cmd"]).unwrap_or_default(),
        _ => first_str(&["path", "file", "command", "query", "pattern", "url"]).unwrap_or_default(),
    }
}

/// Turn a tool result into a trimmed textual preview. Handles plain strings,
/// `{output|stdout|text: "..."}`, and the nested `{content: [{type:"text",
/// text:"..."}]}` shape (so we don't dump a giant raw-JSON blob for e.g. a bash
/// result). Falls back to pretty JSON only when no text can be extracted.
fn preview_result(result: &Value) -> String {
    let raw = extract_text(result).unwrap_or_else(|| {
        if result.is_null() {
            String::new()
        } else {
            serde_json::to_string_pretty(result).unwrap_or_default()
        }
    });
    truncate(&raw, RESULT_PREVIEW_LIMIT)
}

/// Best-effort extraction of human-readable text from a tool result value.
fn extract_text(v: &Value) -> Option<String> {
    if let Some(s) = v.as_str() {
        return Some(s.to_string());
    }
    for key in ["output", "stdout", "text", "message"] {
        if let Some(s) = v.get(key).and_then(Value::as_str) {
            return Some(s.to_string());
        }
    }
    // `content` may be a string or an array of text blocks / strings.
    match v.get("content") {
        Some(Value::String(s)) => return Some(s.clone()),
        Some(Value::Array(arr)) => {
            let mut out = String::new();
            for block in arr {
                if let Some(s) = block.as_str() {
                    out.push_str(s);
                } else if let Some(s) = block.get("text").and_then(Value::as_str) {
                    out.push_str(s);
                }
            }
            if !out.is_empty() {
                return Some(out);
            }
        }
        _ => {}
    }
    None
}

/// The last `limit` characters of `s`: for output still arriving, where the
/// newest is what matters.
fn truncate_head(s: &str, limit: usize) -> String {
    let n = s.chars().count();
    if n <= limit {
        return s.to_string();
    }
    let kept: String = s.chars().skip(n - limit).collect();
    format!("…\n{kept}")
}

fn truncate(s: &str, limit: usize) -> String {
    if s.chars().count() <= limit {
        return s.to_string();
    }
    let kept: String = s.chars().take(limit).collect();
    format!("{kept}\n… (truncated)")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(events: &[AgentEvent]) -> ChatState {
        let mut st = ChatState::new();
        for e in events {
            st.apply(e.clone());
        }
        st
    }

    fn users(st: &ChatState) -> Vec<String> {
        st.items
            .iter()
            .filter_map(|it| match it {
                ChatItem::User { text } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    fn echo(text: &str) -> [AgentEvent; 2] {
        [
            AgentEvent::MessageStart {
                role: "user".into(),
            },
            AgentEvent::MessageEnd {
                text: Some(text.into()),
            },
        ]
    }

    #[test]
    fn session_stats_say_how_full_the_context_is_and_what_it_cost() {
        let stats = |v: serde_json::Value| AgentEvent::Response {
            id: None,
            command: "get_session_stats".into(),
            success: true,
            data: v,
        };
        let mut st = ChatState::new();
        // A new session: nothing to say yet.
        st.apply(stats(serde_json::json!({
            "contextUsage": {"tokens": 0, "contextWindow": 1000000, "percent": 0},
            "cost": 0
        })));
        assert_eq!(st.usage_label(), None);
        // As elyra 0.9.55 reported it after one short reply on Opus 5.5.
        st.apply(stats(serde_json::json!({
            "contextUsage": {"tokens": 22973, "contextWindow": 1000000, "percent": 2.2973},
            "cost": 0.114921
        })));
        assert_eq!(st.usage_label().as_deref(), Some("2% context · $0.11"));
        assert_eq!(st.context.as_ref().unwrap().tokens, Some(22973));
        st.apply(stats(serde_json::json!({
            "contextUsage": {"tokens": 4000, "contextWindow": 1000000, "percent": 0.4},
            "cost": 0.004
        })));
        assert_eq!(st.usage_label().as_deref(), Some("<1% context · <$0.01"));
        // Right after a compaction the size is unknown; the cost still counts.
        st.apply(stats(serde_json::json!({
            "contextUsage": {"tokens": null, "contextWindow": 200000, "percent": null},
            "cost": 1.5
        })));
        assert_eq!(st.usage_label().as_deref(), Some("$1.50"));
    }

    #[test]
    fn a_question_left_open_when_the_turn_ends_is_closed() {
        let st = feed(&[
            AgentEvent::AgentStart,
            AgentEvent::Permission {
                request_id: "7".into(),
                title: "Run `git push`".into(),
                choices: vec![],
            },
            AgentEvent::AgentEnd,
        ]);
        assert!(
            matches!(&st.items[0], ChatItem::Permission(p) if p.answered.as_deref() == Some("not answered: the turn ended"))
        );
    }

    #[test]
    fn a_tool_result_message_is_not_a_reply() {
        let st = feed(&[
            AgentEvent::ToolStart {
                id: "t1".into(),
                name: "read".into(),
                args: serde_json::json!({"path": "app/Order.php"}),
            },
            AgentEvent::ToolEnd {
                id: "t1".into(),
                name: "read".into(),
                result: serde_json::json!("<?php class Order {}"),
                is_error: false,
            },
            // As elyra sends it: the result again, as a message of its own.
            AgentEvent::MessageStart {
                role: "toolResult".into(),
            },
            AgentEvent::MessageEnd {
                text: Some("<?php class Order {}".into()),
            },
            AgentEvent::MessageStart {
                role: "assistant".into(),
            },
            AgentEvent::TextDelta {
                delta: "It is an empty class.".into(),
            },
            AgentEvent::MessageEnd {
                text: Some("It is an empty class.".into()),
            },
        ]);
        let replies: Vec<&str> = st
            .items
            .iter()
            .filter_map(|it| match it {
                ChatItem::Assistant { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(replies, ["It is an empty class."]);
    }

    #[test]
    fn a_message_sent_while_idle_shows_at_once_and_its_echo_adds_nothing() {
        let mut st = ChatState::new();
        st.send("fix the N+1", "fix the N+1", false);
        assert_eq!(users(&st), ["fix the N+1"]);
        assert!(st.running);
        for e in echo("fix the N+1") {
            st.apply(e);
        }
        assert_eq!(users(&st), ["fix the N+1"]);
        assert!(st.sent.is_empty());
    }

    #[test]
    fn a_steer_waits_in_the_queue_and_joins_the_transcript_where_it_is_taken_up() {
        let mut st = ChatState::new();
        st.send("add tests", "add tests", false);
        st.apply(AgentEvent::ToolStart {
            id: "t1".into(),
            name: "bash".into(),
            args: serde_json::json!({"command": "php artisan test"}),
        });
        // Sent while the tool runs: not in the transcript yet, in the queue.
        st.send("use Pest, not PHPUnit", "use Pest, not PHPUnit", true);
        assert_eq!(users(&st), ["add tests"]);
        st.apply(AgentEvent::QueueUpdate {
            steering: vec!["use Pest, not PHPUnit".into()],
            follow_up: vec![],
        });
        assert_eq!(st.steering, ["use Pest, not PHPUnit"]);
        st.apply(AgentEvent::ToolEnd {
            id: "t1".into(),
            name: "bash".into(),
            result: serde_json::json!("ok"),
            is_error: false,
        });
        // The agent takes it up: it joins after the tool, the queue empties.
        for e in echo("use Pest, not PHPUnit") {
            st.apply(e);
        }
        st.apply(AgentEvent::QueueUpdate {
            steering: vec![],
            follow_up: vec![],
        });
        assert_eq!(users(&st), ["add tests", "use Pest, not PHPUnit"]);
        assert!(matches!(st.items.last(), Some(ChatItem::User { .. })));
        assert!(st.steering.is_empty());
    }

    #[test]
    fn a_queued_ask_shows_what_was_typed_not_the_preamble() {
        let mut st = ChatState::new();
        st.send("start", "start", false);
        st.send("why?", "[Ask mode] Answer only.\n\nwhy?", true);
        st.apply(AgentEvent::QueueUpdate {
            steering: vec![],
            follow_up: vec!["[Ask mode] Answer only.\n\nwhy?".into()],
        });
        assert_eq!(st.follow_up, ["why?"]);
        for e in echo("[Ask mode] Answer only.\n\nwhy?") {
            st.apply(e);
        }
        assert_eq!(users(&st), ["start", "why?"]);
    }

    #[test]
    fn a_running_tool_shows_its_latest_output() {
        let mut st = ChatState::new();
        st.apply(AgentEvent::ToolStart {
            id: "t1".into(),
            name: "bash".into(),
            args: serde_json::json!({"command": "php artisan test"}),
        });
        let partial = |text: &str| AgentEvent::ToolUpdate {
            id: "t1".into(),
            name: "bash".into(),
            partial: serde_json::json!({"content": [{"type": "text", "text": text}]}),
        };
        st.apply(partial("PASS  Tests\\Unit\\ExampleTest"));
        let result = |st: &ChatState| match st.items.last() {
            Some(ChatItem::Tool(tc)) => tc.result.clone(),
            _ => None,
        };
        assert_eq!(
            result(&st).as_deref(),
            Some("PASS  Tests\\Unit\\ExampleTest")
        );
        let long = "x".repeat(RESULT_PREVIEW_LIMIT + 10) + "LAST";
        st.apply(partial(&long));
        assert!(result(&st).unwrap().ends_with("LAST"));
        // Once it has ended, an update that arrives late changes nothing.
        st.apply(AgentEvent::ToolEnd {
            id: "t1".into(),
            name: "bash".into(),
            result: serde_json::json!("done"),
            is_error: false,
        });
        st.apply(partial("late"));
        assert_eq!(result(&st).as_deref(), Some("done"));
    }

    #[test]
    fn streams_assistant_text_into_one_bubble() {
        let st = feed(&[
            AgentEvent::AgentStart,
            AgentEvent::MessageStart {
                role: "assistant".into(),
            },
            AgentEvent::TextDelta {
                delta: "Hel".into(),
            },
            AgentEvent::TextDelta { delta: "lo".into() },
            AgentEvent::MessageEnd {
                text: Some("Hello".into()),
            },
            AgentEvent::AgentEnd,
        ]);
        assert_eq!(st.items.len(), 1);
        assert_eq!(
            st.items[0],
            ChatItem::Assistant {
                text: "Hello".into(),
                streaming: false
            }
        );
        assert!(!st.running);
    }

    #[test]
    fn running_flag_tracks_lifecycle() {
        let mut st = ChatState::new();
        assert!(!st.running);
        st.push_user("hi");
        assert!(st.running);
        st.apply(AgentEvent::AgentEnd);
        assert!(!st.running);
    }

    #[test]
    fn tool_card_transitions_running_to_done() {
        let st = feed(&[
            AgentEvent::ToolStart {
                id: "t1".into(),
                name: "read".into(),
                args: serde_json::json!({"path": "src/main.rs"}),
            },
            AgentEvent::ToolEnd {
                id: "t1".into(),
                name: "read".into(),
                result: Value::String("fn main() {}".into()),
                is_error: false,
            },
        ]);
        match &st.items[0] {
            ChatItem::Tool(tc) => {
                assert_eq!(tc.status, ToolStatus::Done);
                assert_eq!(tc.summary, "src/main.rs");
                assert_eq!(tc.result.as_deref(), Some("fn main() {}"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn tool_error_is_flagged() {
        let st = feed(&[
            AgentEvent::ToolStart {
                id: "t1".into(),
                name: "bash".into(),
                args: serde_json::json!({"command": "false"}),
            },
            AgentEvent::ToolEnd {
                id: "t1".into(),
                name: "bash".into(),
                result: serde_json::json!({"output": "boom"}),
                is_error: true,
            },
        ]);
        match &st.items[0] {
            ChatItem::Tool(tc) => {
                assert_eq!(tc.status, ToolStatus::Error);
                assert_eq!(tc.result.as_deref(), Some("boom"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn interleaves_text_then_tool_then_text() {
        let st = feed(&[
            AgentEvent::MessageStart {
                role: "assistant".into(),
            },
            AgentEvent::TextDelta {
                delta: "Let me look".into(),
            },
            AgentEvent::MessageEnd {
                text: Some("Let me look".into()),
            },
            AgentEvent::ToolStart {
                id: "t1".into(),
                name: "read".into(),
                args: serde_json::json!({"path": "a"}),
            },
            AgentEvent::ToolEnd {
                id: "t1".into(),
                name: "read".into(),
                result: Value::Null,
                is_error: false,
            },
            AgentEvent::MessageStart {
                role: "assistant".into(),
            },
            AgentEvent::TextDelta {
                delta: "Done".into(),
            },
            AgentEvent::MessageEnd {
                text: Some("Done".into()),
            },
        ]);
        assert!(
            matches!(st.items[0], ChatItem::Assistant { ref text, .. } if text == "Let me look")
        );
        assert!(matches!(st.items[1], ChatItem::Tool(_)));
        assert!(matches!(st.items[2], ChatItem::Assistant { ref text, .. } if text == "Done"));
    }

    #[test]
    fn reasoning_and_text_are_separate_bubbles() {
        let st = feed(&[
            AgentEvent::MessageStart {
                role: "assistant".into(),
            },
            AgentEvent::ReasoningDelta {
                delta: "hmm".into(),
            },
            AgentEvent::TextDelta {
                delta: "answer".into(),
            },
            AgentEvent::MessageEnd {
                text: Some("answer".into()),
            },
        ]);
        assert!(matches!(st.items[0], ChatItem::Reasoning { ref text, .. } if text == "hmm"));
        assert!(matches!(st.items[1], ChatItem::Assistant { ref text, .. } if text == "answer"));
    }

    #[test]
    fn retry_and_compaction_become_notices() {
        let st = feed(&[
            AgentEvent::RetryStart {
                attempt: 1,
                max_attempts: 3,
                delay_ms: 500,
                error: "overloaded".into(),
            },
            AgentEvent::CompactionStart {
                reason: "threshold".into(),
            },
        ]);
        assert!(
            matches!(&st.items[0], ChatItem::Notice { error: true, text } if text.contains("1/3"))
        );
        assert!(
            matches!(&st.items[1], ChatItem::Notice { error: false, text } if text.contains("Compacting"))
        );
    }

    #[test]
    fn extracts_text_from_nested_content_array() {
        let st = feed(&[
            AgentEvent::ToolStart {
                id: "t".into(),
                name: "bash".into(),
                args: serde_json::json!({"command": "ls"}),
            },
            AgentEvent::ToolEnd {
                id: "t".into(),
                name: "bash".into(),
                result: serde_json::json!({
                    "content": [{"type": "text", "text": "a.rs\nb.rs"}]
                }),
                is_error: false,
            },
        ]);
        if let ChatItem::Tool(tc) = &st.items[0] {
            assert_eq!(tc.result.as_deref(), Some("a.rs\nb.rs"));
        } else {
            panic!();
        }
    }

    #[test]
    fn long_result_is_truncated() {
        let big = "x".repeat(5000);
        let st = feed(&[
            AgentEvent::ToolStart {
                id: "t".into(),
                name: "read".into(),
                args: Value::Null,
            },
            AgentEvent::ToolEnd {
                id: "t".into(),
                name: "read".into(),
                result: Value::String(big),
                is_error: false,
            },
        ]);
        if let ChatItem::Tool(tc) = &st.items[0] {
            let r = tc.result.as_ref().unwrap();
            assert!(r.contains("truncated"));
            assert!(r.chars().count() < 2100);
        } else {
            panic!();
        }
    }

    #[test]
    fn responses_fill_models_state_and_model_changes() {
        let mut st = ChatState::new();
        let models = serde_json::json!({"models": [
            {"id": "claude-opus-5", "name": "Claude Opus 5", "provider": "anthropic", "reasoning": true},
            {"id": "gpt-5", "name": "GPT-5", "provider": "openai"}
        ]});
        st.apply(AgentEvent::Response {
            id: None,
            command: "get_available_models".into(),
            success: true,
            data: models,
        });
        assert_eq!(st.models.len(), 2);
        assert!(st.models[0].reasoning && !st.models[1].reasoning);
        st.apply(AgentEvent::Response {
            id: None,
            command: "get_state".into(),
            success: true,
            data: serde_json::json!({
                "model": {"id": "gpt-5", "name": "GPT-5", "provider": "openai"},
                "thinkingLevel": "high", "sessionId": "s9", "sessionFile": "/tmp/s9.jsonl"
            }),
        });
        assert_eq!(st.model.as_ref().map(|m| m.id.as_str()), Some("gpt-5"));
        assert_eq!(st.thinking, "high");
        assert_eq!(st.session_file.as_deref(), Some("/tmp/s9.jsonl"));
        st.apply(AgentEvent::Response {
            id: None,
            command: "set_model".into(),
            success: true,
            data: serde_json::json!({"id": "claude-opus-5", "provider": "anthropic"}),
        });
        assert_eq!(
            st.model.as_ref().map(|m| m.name.as_str()),
            Some("claude-opus-5")
        );
        // A failed command changes nothing.
        st.apply(AgentEvent::Response {
            id: None,
            command: "set_model".into(),
            success: false,
            data: serde_json::json!({"id": "nope"}),
        });
        assert_eq!(
            st.model.as_ref().map(|m| m.id.as_str()),
            Some("claude-opus-5")
        );
    }

    #[test]
    fn a_stored_conversation_is_replayed_into_items() {
        let mut st = ChatState::new();
        let messages = serde_json::json!([
            {"role": "user", "content": "Fix the bug"},
            {"role": "assistant", "content": [
                {"type": "text", "text": "Reading the file."},
                {"type": "toolCall", "id": "t1", "name": "read", "arguments": {"path": "a.php"}}
            ]},
            {"role": "toolResult", "toolCallId": "t1", "toolName": "read", "content": [{"type": "text", "text": "<?php"}], "isError": false},
            {"role": "assistant", "content": [{"type": "text", "text": "Done."}]}
        ]);
        st.apply(AgentEvent::Response {
            id: None,
            command: "get_messages".into(),
            success: true,
            data: serde_json::json!({"messages": messages}),
        });
        assert_eq!(st.items.len(), 4);
        assert_eq!(
            st.items[0],
            ChatItem::User {
                text: "Fix the bug".into()
            }
        );
        assert!(
            matches!(&st.items[1], ChatItem::Assistant { text, streaming: false } if text == "Reading the file.")
        );
        match &st.items[2] {
            ChatItem::Tool(tc) => {
                assert_eq!((tc.name.as_str(), tc.summary.as_str()), ("read", "a.php"));
                assert_eq!(tc.status, ToolStatus::Done);
                assert_eq!(tc.result.as_deref(), Some("<?php"));
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(&st.items[3], ChatItem::Assistant { text, .. } if text == "Done."));
    }

    #[test]
    fn the_echoed_user_message_is_not_drawn_as_a_reply() {
        let mut st = ChatState::new();
        st.push_user("Say hi");
        for ev in [
            AgentEvent::AgentStart,
            AgentEvent::TurnStart,
            AgentEvent::MessageStart {
                role: "user".into(),
            },
            AgentEvent::MessageEnd {
                text: Some("Say hi".into()),
            },
            AgentEvent::MessageStart {
                role: "assistant".into(),
            },
            AgentEvent::TextDelta { delta: "hi".into() },
            AgentEvent::MessageEnd {
                text: Some("hi".into()),
            },
            AgentEvent::TurnEnd,
            AgentEvent::AgentEnd,
        ] {
            st.apply(ev);
        }
        assert_eq!(
            st.items,
            vec![
                ChatItem::User {
                    text: "Say hi".into()
                },
                ChatItem::Assistant {
                    text: "hi".into(),
                    streaming: false
                },
            ]
        );
    }
}
