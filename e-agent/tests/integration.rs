//! Opt-in smoke test against a real `elyra --mode rpc` process.
//!
//! Ignored by default (not run in CI, makes no paid LLM call). Run locally with:
//!
//! ```bash
//! cargo test -p e-agent --test integration -- --ignored --nocapture
//! ```
//!
//! It only verifies process spawn + JSONL framing + command round-trip using
//! `abort` (which does not invoke the model), so it is free to run.

use std::time::Duration;

use e_agent::{AgentClient, AgentEvent};

#[test]
#[ignore = "requires elyra on PATH; run with --ignored"]
fn rpc_spawn_and_abort_roundtrip() {
    let cwd = std::env::current_dir().unwrap();
    let (client, rx) = AgentClient::spawn(
        "elyra",
        &["--mode".to_string(), "rpc".to_string()],
        &cwd,
        &[],
    )
    .expect("failed to spawn `elyra --mode rpc` (is elyra installed?)");

    // Nudge the process, then ask it to abort. Neither calls the LLM.
    client.abort().expect("send abort");

    // Within a few seconds we should observe *some* structured event: either the
    // session header elyra emits on startup, or the ack for our abort command.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let mut saw_structured = false;
    while std::time::Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(ev) => {
                eprintln!("event: {ev:?}");
                if matches!(ev, AgentEvent::Session { .. } | AgentEvent::Response { .. }) {
                    saw_structured = true;
                    break;
                }
            }
            Err(_) => continue,
        }
    }

    client.shutdown();
    assert!(
        saw_structured,
        "expected a session header or command response from elyra rpc"
    );
}
