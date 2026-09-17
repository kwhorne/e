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

/// One item in the transcript, in event order.
#[derive(Clone, Debug, PartialEq)]
pub enum ChatItem {
    User { text: String },
    Assistant { text: String, streaming: bool },
    Reasoning { text: String, streaming: bool },
    Tool(ToolCall),
    Notice { text: String, error: bool },
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
                // The echo of the user's message ends here; nothing to draw.
                if self.cur_role == "user" {
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
            AgentEvent::ToolUpdate { .. } => {}
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
            AgentEvent::QueueUpdate { .. } => {}
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
