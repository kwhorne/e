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

/// The running conversation state.
#[derive(Clone, Debug, Default)]
pub struct ChatState {
    pub items: Vec<ChatItem>,
    /// True while the agent is actively working on the current prompt.
    pub running: bool,
    /// Index of the assistant item currently receiving text deltas.
    cur_assistant: Option<usize>,
    /// Index of the reasoning item currently receiving deltas.
    cur_reasoning: Option<usize>,
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
            AgentEvent::Session { .. } | AgentEvent::TurnStart | AgentEvent::TurnEnd => {}
            AgentEvent::AgentStart => self.running = true,
            AgentEvent::MessageStart { .. } => {
                // A fresh assistant turn: subsequent deltas start a new bubble.
                self.cur_assistant = None;
                self.cur_reasoning = None;
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
            AgentEvent::QueueUpdate { .. } | AgentEvent::Response { .. } => {}
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

/// Turn a tool result (string, or `{output|content|stdout: ...}`, or arbitrary
/// JSON) into a trimmed textual preview.
fn preview_result(result: &Value) -> String {
    let raw = if let Some(s) = result.as_str() {
        s.to_string()
    } else if let Some(s) = result
        .get("output")
        .or_else(|| result.get("content"))
        .or_else(|| result.get("stdout"))
        .and_then(Value::as_str)
    {
        s.to_string()
    } else if result.is_null() {
        String::new()
    } else {
        serde_json::to_string_pretty(result).unwrap_or_default()
    };
    truncate(&raw, RESULT_PREVIEW_LIMIT)
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
}
