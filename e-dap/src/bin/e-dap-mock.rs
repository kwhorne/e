//! A minimal mock DAP adapter used only by `e-dap`'s integration tests.
//!
//! It speaks just enough of the protocol to drive a full initialize → inspect →
//! disconnect flow, so the client's framing, reader thread and seq-correlation
//! can be exercised deterministically without a real language adapter.

use std::io::{BufRead, BufReader, Write};

use serde_json::{json, Value};

fn main() {
    let stdin = std::io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let mut seq = 1000;

    while let Some(msg) = read_message(&mut reader) {
        if msg["type"].as_str() != Some("request") {
            continue;
        }
        let request_seq = msg["seq"].as_i64().unwrap_or(0);
        let command = msg["command"].as_str().unwrap_or("").to_string();

        let body: Value = match command.as_str() {
            "initialize" => json!({ "supportsConfigurationDoneRequest": true }),
            "setBreakpoints" => {
                let lines = msg["arguments"]["breakpoints"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                let bps: Vec<Value> = lines
                    .iter()
                    .map(|b| json!({ "verified": true, "line": b["line"] }))
                    .collect();
                json!({ "breakpoints": bps })
            }
            "threads" => json!({ "threads": [ { "id": 1, "name": "main" } ] }),
            "stackTrace" => json!({
                "stackFrames": [
                    { "id": 1, "name": "{main}", "line": 3, "column": 1,
                      "source": { "path": "/tmp/app.php" } }
                ],
                "totalFrames": 1
            }),
            _ => json!({}),
        };

        seq += 1;
        send(
            seq,
            &json!({
                "seq": seq,
                "type": "response",
                "request_seq": request_seq,
                "success": true,
                "command": command,
                "body": body,
            }),
        );

        // After initialize, emit the `initialized` event like a real adapter.
        if command == "initialize" {
            seq += 1;
            send(
                seq,
                &json!({ "seq": seq, "type": "event", "event": "initialized" }),
            );
        }

        if command == "disconnect" {
            break;
        }
    }
}

fn send(_seq: i64, msg: &Value) {
    let body = serde_json::to_vec(msg).unwrap();
    let mut out = std::io::stdout();
    let _ = write!(out, "Content-Length: {}\r\n\r\n", body.len());
    let _ = out.write_all(&body);
    let _ = out.flush();
}

fn read_message(reader: &mut impl BufRead) -> Option<Value> {
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).ok()?;
        if n == 0 {
            return None;
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break;
        }
        if let Some(rest) = trimmed.strip_prefix("Content-Length:") {
            content_length = Some(rest.trim().parse().ok()?);
        }
    }
    let len = content_length?;
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).ok()?;
    serde_json::from_slice(&buf).ok()
}
