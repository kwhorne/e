//! A synchronous client for elyra's RPC mode (`elyra --mode rpc`).
//!
//! Architecture mirrors [`e-dap`](../../e-dap): spawn the process, one background
//! reader thread turns stdout JSONL into [`AgentEvent`]s on a channel, and
//! commands are written to stdin as JSON lines. The reader loop is factored out
//! as [`pump`] so it can be exercised with an in-memory reader in tests.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{Context, Result};
use crossbeam_channel::{unbounded, Receiver, Sender};
use serde_json::{json, Value};

use crate::protocol::{parse_event, AgentEvent};

/// How a queued message should be delivered while the agent is streaming.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Streaming {
    /// Deliver after the current turn's tool calls, before the next LLM call.
    Steer,
    /// Deliver only once the agent goes idle.
    FollowUp,
}

impl Streaming {
    fn as_str(self) -> &'static str {
        match self {
            Streaming::Steer => "steer",
            Streaming::FollowUp => "followUp",
        }
    }
}

/// A handle to a running elyra RPC process.
pub struct AgentClient {
    child: Arc<Mutex<Child>>,
    stdin: Arc<Mutex<ChildStdin>>,
}

impl AgentClient {
    /// Spawn `program args...` in `cwd` with `env` overlaid, and return the
    /// client plus a receiver of decoded events. The caller is responsible for
    /// passing the RPC flags (e.g. `--mode rpc`) and, if the process must be
    /// found on the user's `PATH`, for invoking it through a login shell.
    pub fn spawn(
        program: &str,
        args: &[String],
        cwd: &std::path::Path,
        env: &[(String, String)],
    ) -> Result<(Self, Receiver<AgentEvent>)> {
        let mut cmd = Command::new(program);
        cmd.args(args);
        cmd.current_dir(cwd);
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        let mut child = cmd
            .spawn()
            .with_context(|| format!("failed to spawn agent '{program}'"))?;
        let stdout = child.stdout.take().context("agent stdout missing")?;
        let stdin = child.stdin.take().context("agent stdin missing")?;

        let (tx, rx) = unbounded();
        thread::Builder::new()
            .name("e-agent-reader".into())
            .spawn(move || {
                pump(BufReader::new(stdout), &tx);
            })
            .context("failed to start reader thread")?;

        Ok((
            Self {
                child: Arc::new(Mutex::new(child)),
                stdin: Arc::new(Mutex::new(stdin)),
            },
            rx,
        ))
    }

    /// Send a user prompt. If the agent is already streaming, pass `streaming` to
    /// steer / follow-up rather than being rejected.
    pub fn prompt(&self, message: &str, streaming: Option<Streaming>) -> Result<()> {
        let mut cmd = json!({ "type": "prompt", "message": message });
        if let Some(s) = streaming {
            cmd["streamingBehavior"] = Value::String(s.as_str().to_string());
        }
        self.send(&cmd)
    }

    /// Queue a steering message while the agent runs.
    pub fn steer(&self, message: &str) -> Result<()> {
        self.send(&json!({ "type": "steer", "message": message }))
    }

    /// Queue a follow-up message, delivered when the agent goes idle.
    pub fn follow_up(&self, message: &str) -> Result<()> {
        self.send(&json!({ "type": "follow_up", "message": message }))
    }

    /// Abort the current operation.
    pub fn abort(&self) -> Result<()> {
        self.send(&json!({ "type": "abort" }))
    }

    /// Start a fresh session.
    pub fn new_session(&self) -> Result<()> {
        self.send(&json!({ "type": "new_session" }))
    }

    /// Serialize a command and write it as one `\n`-terminated line to stdin.
    fn send(&self, cmd: &Value) -> Result<()> {
        let mut line = serde_json::to_string(cmd)?;
        line.push('\n');
        let mut stdin = self.stdin.lock().expect("agent stdin poisoned");
        stdin.write_all(line.as_bytes())?;
        stdin.flush()?;
        Ok(())
    }

    /// Terminate the process (best-effort).
    pub fn shutdown(&self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for AgentClient {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Read `\n`-delimited JSONL from `reader`, decode each line, and forward the
/// resulting [`AgentEvent`]s on `tx`. Returns when the stream ends (process exit)
/// or the receiver is gone. Strict LF framing: we split on `\n` only and strip a
/// trailing `\r`, matching elyra's RPC framing rules.
pub fn pump<R: BufRead>(mut reader: R, tx: &Sender<AgentEvent>) {
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) => break, // EOF
            Ok(_) => {
                let line = String::from_utf8_lossy(&buf);
                if let Some(ev) = parse_event(&line) {
                    if tx.send(ev).is_err() {
                        break; // receiver dropped
                    }
                }
            }
            Err(_) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn pump_decodes_jsonl_stream() {
        let data = concat!(
            "{\"type\":\"session\",\"id\":\"s1\",\"cwd\":\"/w\"}\n",
            "{\"type\":\"agent_start\"}\n",
            "{\"type\":\"message_update\",\"message\":{},\"assistantMessageEvent\":{\"type\":\"text_delta\",\"delta\":\"hi\"}}\n",
            "{\"type\":\"agent_end\"}\n",
        );
        let (tx, rx) = unbounded();
        pump(Cursor::new(data), &tx);
        drop(tx);
        let got: Vec<_> = rx.iter().collect();
        assert_eq!(got.len(), 4);
        assert_eq!(
            got[0],
            AgentEvent::Session {
                id: "s1".into(),
                cwd: "/w".into()
            }
        );
        assert_eq!(got[2], AgentEvent::TextDelta { delta: "hi".into() });
        assert_eq!(got[3], AgentEvent::AgentEnd);
    }

    #[test]
    fn pump_ignores_blank_and_noise_lines() {
        let data = "\n{\"type\":\"agent_start\"}\nnot json\n\n{\"type\":\"agent_end\"}\n";
        let (tx, rx) = unbounded();
        pump(Cursor::new(data), &tx);
        drop(tx);
        let got: Vec<_> = rx.iter().collect();
        assert_eq!(got, vec![AgentEvent::AgentStart, AgentEvent::AgentEnd]);
    }

    #[test]
    fn pump_handles_crlf_framing() {
        let data = "{\"type\":\"agent_start\"}\r\n{\"type\":\"agent_end\"}\r\n";
        let (tx, rx) = unbounded();
        pump(Cursor::new(data), &tx);
        drop(tx);
        let got: Vec<_> = rx.iter().collect();
        assert_eq!(got, vec![AgentEvent::AgentStart, AgentEvent::AgentEnd]);
    }
}
