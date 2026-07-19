//! Golden-file test: replay a recorded elyra JSONL transcript through the reducer
//! and assert the shape of the resulting conversation. This exercises the whole
//! core (parsing + folding) end to end, without spawning a process or a GUI.

use e_agent::{ChatItem, ToolStatus};

#[test]
fn replays_a_two_turn_tool_session() {
    let transcript = include_str!("fixtures/list_files.jsonl");
    let state = e_agent::replay(transcript);

    // The agent finished, so nothing should still be marked as streaming.
    assert!(!state.running, "agent_end should clear the running flag");

    // Expect: assistant intro -> bash tool card (done) -> assistant conclusion.
    assert_eq!(state.items.len(), 3, "got: {:#?}", state.items);

    match &state.items[0] {
        ChatItem::Assistant { text, streaming } => {
            assert_eq!(text, "I'll list the files.");
            assert!(!streaming);
        }
        other => panic!("item 0 should be assistant, got {other:?}"),
    }

    match &state.items[1] {
        ChatItem::Tool(tc) => {
            assert_eq!(tc.name, "bash");
            assert_eq!(tc.summary, "ls -1");
            assert_eq!(tc.status, ToolStatus::Done);
            assert_eq!(tc.result.as_deref(), Some("Cargo.toml\nsrc\nREADME.md"));
        }
        other => panic!("item 1 should be a tool card, got {other:?}"),
    }

    match &state.items[2] {
        ChatItem::Assistant { text, streaming } => {
            assert_eq!(
                text,
                "There are three entries: Cargo.toml, src, and README.md."
            );
            assert!(!streaming);
        }
        other => panic!("item 2 should be assistant, got {other:?}"),
    }
}

#[test]
fn partial_stream_leaves_last_bubble_streaming() {
    // Cut the transcript off mid-second-turn (no agent_end): the final assistant
    // bubble should still be marked streaming so the UI can show a caret/spinner.
    let full = include_str!("fixtures/list_files.jsonl");
    let cut: String = full
        .lines()
        .take_while(|l| !l.contains("\"There are three entries"))
        .collect::<Vec<_>>()
        .join("\n");

    let state = e_agent::replay(&cut);
    assert!(state.running, "no agent_end yet -> still running");
    match state.items.last().unwrap() {
        ChatItem::Assistant { streaming, .. } => assert!(*streaming),
        other => panic!("expected a streaming assistant bubble, got {other:?}"),
    }
}
