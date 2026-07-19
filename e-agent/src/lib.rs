//! Native agent client for `e`.
//!
//! Instead of running an agent's full terminal UI inside a PTY (which forces a
//! whole-screen ANSI re-parse on every frame and offers no chat affordances),
//! this crate speaks elyra's structured RPC protocol (`elyra --mode rpc`) and
//! folds the event stream into a renderable [`ChatState`]. The editor then draws
//! the conversation with native views: streaming assistant text, tool-call cards,
//! a real stop/abort, steering, and so on.
//!
//! The design is split so the core is testable without a process or a GUI:
//! - [`protocol`] — decode one JSONL line into an [`AgentEvent`].
//! - [`chat`] — fold [`AgentEvent`]s into [`ChatItem`]s ([`ChatState`]).
//! - [`client`] — spawn the process and stream events over a channel.

pub mod chat;
pub mod client;
pub mod protocol;

pub use chat::{ChatItem, ChatState, ToolCall, ToolStatus};
pub use client::{pump, AgentClient, Streaming};
pub use protocol::{parse_event, AgentEvent};

/// Replay a whole JSONL transcript (as emitted by `elyra --mode json`/`rpc`)
/// through the reducer and return the resulting conversation state. Handy for
/// golden-file tests and for rehydrating a saved session.
pub fn replay(transcript: &str) -> ChatState {
    let mut state = ChatState::new();
    for line in transcript.lines() {
        if let Some(ev) = parse_event(line) {
            state.apply(ev);
        }
    }
    state
}
