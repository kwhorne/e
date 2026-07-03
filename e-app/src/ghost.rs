//! Inline AI completion ("ghost text").
//!
//! After a short idle in a code buffer we ask a local code model (via Ollama's
//! fill-in-the-middle endpoint) for a one-line continuation and render it as
//! grey phantom text at the cursor. `Tab` accepts it; typing or moving away
//! dismisses it. Entirely local and opt-in — nothing runs unless the
//! `ai_completion` setting is on and Ollama is reachable.

use std::time::Duration;

use floem::ext_event::create_ext_action;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use floem::views::editor::core::editor::EditType;
use floem::views::editor::core::selection::Selection;
use floem::views::editor::text::Document;

use e_core::language::Language;

use crate::state::{AppState, Buffer};

/// A pending inline suggestion: the single-line `text` to insert at `offset`.
#[derive(Clone, Debug)]
pub struct GhostText {
    pub offset: usize,
    pub text: String,
}

/// The code model used for completions. A small, fast FIM model by default.
fn model() -> String {
    std::env::var("E_COMPLETION_MODEL").unwrap_or_else(|_| "qwen2.5-coder".to_string())
}

/// Ask Ollama to fill in the gap between `prefix` and `suffix`. Blocking.
fn fetch_completion(prefix: &str, suffix: &str, model: &str) -> Option<String> {
    let body = serde_json::json!({
        "model": model,
        "prompt": prefix,
        "suffix": suffix,
        "stream": false,
        "options": { "temperature": 0.1, "num_predict": 96, "stop": ["\n\n"] },
    });
    let resp = ureq::post("http://localhost:11434/api/generate")
        .timeout(Duration::from_secs(10))
        .send_json(body)
        .ok()?;
    let v: serde_json::Value = resp.into_json().ok()?;
    let text = v.get("response")?.as_str()?;
    // Render a single line inline; keep it trimmed of trailing whitespace.
    let line = text
        .trim_end_matches(['\r', '\n'])
        .lines()
        .next()?
        .to_string();
    if line.trim().is_empty() {
        None
    } else {
        Some(line)
    }
}

/// Take the last ~2 KB of `s` on a char boundary (completion context window).
fn tail(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut start = s.len() - max;
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    &s[start..]
}

/// Take the first ~`max` bytes of `s` on a char boundary.
fn head(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

impl AppState {
    /// Request an inline suggestion for `buffer_id` after a debounce. No-op
    /// unless enabled; supersedes any in-flight request.
    pub fn request_ghost(&self, buffer_id: u64) {
        if !self.settings.get_untracked().ai_completion {
            return;
        }
        // Don't compete with the LSP completion popup.
        if self.completion.open.get_untracked() {
            self.clear_ghost();
            return;
        }
        let gen = self.ghost_gen.get_untracked().wrapping_add(1);
        self.ghost_gen.set(gen);
        self.clear_ghost();

        let state = *self;
        floem::action::exec_after(Duration::from_millis(350), move |_| {
            // Superseded by a newer keystroke? Skip.
            if state.ghost_gen.get_untracked() == gen {
                state.fire_ghost(buffer_id, gen);
            }
        });
    }

    /// Actually fetch a completion (on a worker) and store it if still current.
    fn fire_ghost(&self, buffer_id: u64, gen: u64) {
        let Some(buf) = self.buffer(buffer_id) else {
            return;
        };
        if buf.file.language == Language::PlainText {
            return;
        }
        let Some(editor) = buf.editor.get_untracked() else {
            return;
        };
        let offset = editor.cursor.get_untracked().offset();
        let text = buf.doc.text().to_string();
        if offset > text.len() || !text.is_char_boundary(offset) {
            return;
        }
        let prefix = tail(&text[..offset], 2000).to_string();
        let suffix = head(&text[offset..], 500).to_string();
        let model = model();

        let state = *self;
        let store = create_ext_action(self.cx, move |line: Option<String>| {
            // Only apply if nothing newer happened and the cursor hasn't moved.
            if state.ghost_gen.get_untracked() != gen {
                return;
            }
            let Some(line) = line else {
                return;
            };
            let Some(buf) = state.buffer(buffer_id) else {
                return;
            };
            let still_here = buf
                .editor
                .get_untracked()
                .map(|e| e.cursor.get_untracked().offset() == offset)
                .unwrap_or(false);
            if !still_here {
                return;
            }
            buf.ghost.set(Some(GhostText { offset, text: line }));
            buf.doc.cache_rev().update(|r| *r += 1);
        });
        std::thread::spawn(move || store(fetch_completion(&prefix, &suffix, &model)));
    }

    /// `Tab`: accept the active buffer's ghost suggestion, if any. Returns
    /// whether it consumed the key.
    pub fn accept_ghost(&self) -> bool {
        let Some(buf) = self.active_buffer() else {
            return false;
        };
        let Some(ghost) = buf.ghost.get_untracked() else {
            return false;
        };
        let Some(editor) = buf.editor.get_untracked() else {
            return false;
        };
        buf.ghost.set(None);
        // Stale (cursor moved since it arrived) → don't insert, just dismiss.
        if editor.cursor.get_untracked().offset() != ghost.offset {
            buf.doc.cache_rev().update(|r| *r += 1);
            return false;
        }
        let mut it = std::iter::once((Selection::caret(ghost.offset), ghost.text.as_str()));
        buf.doc.edit(&mut it, EditType::InsertChars);
        buf.dirty.set(true);
        true
    }

    /// Dismiss any ghost suggestion in the active buffer.
    pub fn clear_ghost(&self) {
        if let Some(buf) = self.active_buffer() {
            if buf.ghost.get_untracked().is_some() {
                buf.ghost.set(None);
                buf.doc.cache_rev().update(|r| *r += 1);
            }
        }
    }

    fn buffer(&self, id: u64) -> Option<Buffer> {
        self.buffers
            .with_untracked(|bs| bs.iter().find(|b| b.id == id).cloned())
    }
}
