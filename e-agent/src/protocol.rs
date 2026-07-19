//! Parsing of elyra's RPC / JSON event stream (`elyra --mode rpc` / `--mode json`).
//!
//! The wire format is JSONL: one JSON object per line, `\n`-delimited. We parse
//! each line into a high-level [`AgentEvent`]. Parsing is deliberately lenient —
//! unknown event types are preserved as [`AgentEvent::Other`] rather than failing,
//! so a newer elyra can add events without breaking the client. We only pull the
//! specific fields we render, and never rely on the full (large) `message` object
//! deserializing cleanly.

use serde_json::Value;

/// A high-level event decoded from a single JSONL line.
#[derive(Clone, Debug, PartialEq)]
pub enum AgentEvent {
    /// Session header (first line): `{"type":"session",...}`.
    Session { id: String, cwd: String },
    /// The agent started working on a prompt.
    AgentStart,
    /// A new assistant turn began.
    TurnStart,
    /// An assistant message began streaming.
    MessageStart { role: String },
    /// A chunk of assistant text.
    TextDelta { delta: String },
    /// A chunk of assistant reasoning / thinking.
    ReasoningDelta { delta: String },
    /// The assistant message finished; `text` is the authoritative final text
    /// (used to reconcile any deltas we may have missed).
    MessageEnd { text: Option<String> },
    /// The assistant turn finished (tool calls, if any, follow).
    TurnEnd,
    /// A tool call started executing.
    ToolStart {
        id: String,
        name: String,
        args: Value,
    },
    /// Progress update for a running tool.
    ToolUpdate {
        id: String,
        name: String,
        partial: Value,
    },
    /// A tool call finished.
    ToolEnd {
        id: String,
        name: String,
        result: Value,
        is_error: bool,
    },
    /// Context compaction started.
    CompactionStart { reason: String },
    /// Context compaction finished.
    CompactionEnd { reason: String, aborted: bool },
    /// An automatic retry (after a transient error) started.
    RetryStart {
        attempt: u32,
        max_attempts: u32,
        delay_ms: u64,
        error: String,
    },
    /// An automatic retry finished.
    RetryEnd {
        success: bool,
        attempt: u32,
        final_error: Option<String>,
    },
    /// The steering / follow-up queues changed.
    QueueUpdate {
        steering: Vec<String>,
        follow_up: Vec<String>,
    },
    /// A command acknowledgement (`{"type":"response",...}`).
    Response {
        id: Option<String>,
        command: String,
        success: bool,
    },
    /// The agent finished the whole prompt (idle again).
    AgentEnd,
    /// A protocol-level error message.
    Error { message: String },
    /// Any event type we don't specifically model, kept verbatim.
    Other { kind: String, raw: Value },
}

fn str_field(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Extract the plain text from an assistant `message` object. `content` may be a
/// bare string or an array of typed blocks (`{"type":"text","text":"..."}`).
fn message_text(msg: &Value) -> Option<String> {
    let content = msg.get("content")?;
    if let Some(s) = content.as_str() {
        return Some(s.to_string());
    }
    let arr = content.as_array()?;
    let mut out = String::new();
    for block in arr {
        if block.get("type").and_then(Value::as_str) == Some("text") {
            if let Some(t) = block.get("text").and_then(Value::as_str) {
                out.push_str(t);
            }
        }
    }
    Some(out)
}

/// Parse one JSONL line into an [`AgentEvent`]. Returns `None` for blank lines or
/// non-JSON noise (elyra may print the occasional plain-text diagnostic).
pub fn parse_event(line: &str) -> Option<AgentEvent> {
    let line = line.trim_end_matches(['\r', '\n']);
    if line.trim().is_empty() {
        return None;
    }
    let v: Value = serde_json::from_str(line).ok()?;
    let kind = v.get("type").and_then(Value::as_str)?.to_string();

    let ev = match kind.as_str() {
        "session" => AgentEvent::Session {
            id: str_field(&v, "id"),
            cwd: str_field(&v, "cwd"),
        },
        "agent_start" => AgentEvent::AgentStart,
        "turn_start" => AgentEvent::TurnStart,
        "message_start" => AgentEvent::MessageStart {
            role: v
                .get("message")
                .map(|m| str_field(m, "role"))
                .unwrap_or_default(),
        },
        "message_update" => {
            let ame = v.get("assistantMessageEvent");
            let sub = ame
                .and_then(|e| e.get("type"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let delta = ame
                .and_then(|e| e.get("delta"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            match sub {
                "text_delta" => AgentEvent::TextDelta { delta },
                "reasoning_delta" | "thinking_delta" | "reasoning" => {
                    AgentEvent::ReasoningDelta { delta }
                }
                _ => AgentEvent::Other { kind, raw: v },
            }
        }
        "message_end" => AgentEvent::MessageEnd {
            text: v.get("message").and_then(message_text),
        },
        "turn_end" => AgentEvent::TurnEnd,
        "tool_execution_start" => AgentEvent::ToolStart {
            id: str_field(&v, "toolCallId"),
            name: str_field(&v, "toolName"),
            args: v.get("args").cloned().unwrap_or(Value::Null),
        },
        "tool_execution_update" => AgentEvent::ToolUpdate {
            id: str_field(&v, "toolCallId"),
            name: str_field(&v, "toolName"),
            partial: v.get("partialResult").cloned().unwrap_or(Value::Null),
        },
        "tool_execution_end" => AgentEvent::ToolEnd {
            id: str_field(&v, "toolCallId"),
            name: str_field(&v, "toolName"),
            result: v.get("result").cloned().unwrap_or(Value::Null),
            is_error: v.get("isError").and_then(Value::as_bool).unwrap_or(false),
        },
        "compaction_start" => AgentEvent::CompactionStart {
            reason: str_field(&v, "reason"),
        },
        "compaction_end" => AgentEvent::CompactionEnd {
            reason: str_field(&v, "reason"),
            aborted: v.get("aborted").and_then(Value::as_bool).unwrap_or(false),
        },
        "auto_retry_start" => AgentEvent::RetryStart {
            attempt: v.get("attempt").and_then(Value::as_u64).unwrap_or(0) as u32,
            max_attempts: v.get("maxAttempts").and_then(Value::as_u64).unwrap_or(0) as u32,
            delay_ms: v.get("delayMs").and_then(Value::as_u64).unwrap_or(0),
            error: str_field(&v, "errorMessage"),
        },
        "auto_retry_end" => AgentEvent::RetryEnd {
            success: v.get("success").and_then(Value::as_bool).unwrap_or(false),
            attempt: v.get("attempt").and_then(Value::as_u64).unwrap_or(0) as u32,
            final_error: v
                .get("finalError")
                .and_then(Value::as_str)
                .map(str::to_string),
        },
        "queue_update" => AgentEvent::QueueUpdate {
            steering: str_array(&v, "steering"),
            follow_up: str_array(&v, "followUp"),
        },
        "response" => AgentEvent::Response {
            id: v.get("id").and_then(Value::as_str).map(str::to_string),
            command: str_field(&v, "command"),
            success: v.get("success").and_then(Value::as_bool).unwrap_or(false),
        },
        "agent_end" => AgentEvent::AgentEnd,
        "error" => AgentEvent::Error {
            message: str_field(&v, "message"),
        },
        _ => AgentEvent::Other { kind, raw: v },
    };
    Some(ev)
}

fn str_array(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_session_header() {
        let e = parse_event(r#"{"type":"session","version":3,"id":"abc","cwd":"/x"}"#);
        assert_eq!(
            e,
            Some(AgentEvent::Session {
                id: "abc".into(),
                cwd: "/x".into()
            })
        );
    }

    #[test]
    fn parses_text_delta_from_message_update() {
        let line = r#"{"type":"message_update","message":{},"assistantMessageEvent":{"type":"text_delta","delta":"Hello"}}"#;
        assert_eq!(
            parse_event(line),
            Some(AgentEvent::TextDelta {
                delta: "Hello".into()
            })
        );
    }

    #[test]
    fn message_update_without_text_is_other() {
        let line = r#"{"type":"message_update","message":{},"assistantMessageEvent":{"type":"tool_call_delta"}}"#;
        assert!(matches!(parse_event(line), Some(AgentEvent::Other { .. })));
    }

    #[test]
    fn message_end_extracts_text_blocks() {
        let line = r#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"Hi "},{"type":"tool_use"},{"type":"text","text":"there"}]}}"#;
        assert_eq!(
            parse_event(line),
            Some(AgentEvent::MessageEnd {
                text: Some("Hi there".into())
            })
        );
    }

    #[test]
    fn parses_tool_lifecycle() {
        let start = r#"{"type":"tool_execution_start","toolCallId":"t1","toolName":"read","args":{"path":"a.rs"}}"#;
        match parse_event(start).unwrap() {
            AgentEvent::ToolStart { id, name, .. } => {
                assert_eq!(id, "t1");
                assert_eq!(name, "read");
            }
            other => panic!("{other:?}"),
        }
        let end = r#"{"type":"tool_execution_end","toolCallId":"t1","toolName":"read","result":"ok","isError":false}"#;
        assert_eq!(
            parse_event(end),
            Some(AgentEvent::ToolEnd {
                id: "t1".into(),
                name: "read".into(),
                result: Value::String("ok".into()),
                is_error: false,
            })
        );
    }

    #[test]
    fn unknown_type_is_preserved() {
        let line = r#"{"type":"some_future_event","x":1}"#;
        match parse_event(line).unwrap() {
            AgentEvent::Other { kind, .. } => assert_eq!(kind, "some_future_event"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn blank_and_noise_lines_are_ignored() {
        assert_eq!(parse_event(""), None);
        assert_eq!(parse_event("   "), None);
        assert_eq!(parse_event("not json"), None);
    }

    #[test]
    fn strips_trailing_cr() {
        assert_eq!(
            parse_event("{\"type\":\"agent_start\"}\r"),
            Some(AgentEvent::AgentStart)
        );
    }
}
