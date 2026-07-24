//! File-level Elyra ↔ editor integration helpers (pure, unit-tested).
//!
//! - [`detect_paths`] finds `path`, `path:line` and `path:line:col` references in
//!   agent/terminal output so they can be turned into click-to-open links.
//! - [`format_selection_context`] builds the one-line reference sent to the agent
//!   by the *Send selection to agent* command.

use std::ops::Range;
use std::path::PathBuf;
use std::time::Duration;

use floem::reactive::{SignalGet, SignalUpdate};
use floem::views::editor::core::cursor::CursorMode;
use floem::views::editor::text::Document;

use crate::state::AppState;

/// A file reference found in a run of text.
#[derive(Debug, Clone, PartialEq)]
pub struct PathHit {
    /// Byte range of the whole reference (path + optional `:line:col`) in the text.
    pub range: Range<usize>,
    pub path: String,
    pub line: Option<usize>,
    pub col: Option<usize>,
}

fn is_path_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'/' | b'~' | b'+' | b'@')
}

/// A token looks like a file path if it contains a `/` and its last segment has
/// a short alphanumeric extension (so bare directories and prose are ignored).
fn looks_like_path(tok: &str) -> bool {
    if !tok.contains('/') || tok.starts_with("//") {
        return false;
    }
    let last = tok.rsplit('/').next().unwrap_or("");
    match last.rfind('.') {
        Some(dot) => {
            let name = &last[..dot];
            let ext = &last[dot + 1..];
            !name.is_empty()
                && !ext.is_empty()
                && ext.len() <= 6
                && ext.chars().all(|c| c.is_ascii_alphanumeric())
        }
        None => false,
    }
}

fn parse_uint(bytes: &[u8], at: usize) -> Option<(usize, usize)> {
    let mut i = at;
    let mut val = 0usize;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        val = val * 10 + (bytes[i] - b'0') as usize;
        i += 1;
    }
    if i > at {
        Some((val, i))
    } else {
        None
    }
}

/// Scan `text` for file references. Conservative — a match must contain a `/`
/// and a file extension, so code like `preg_replace('/:\d+$/', …)` is ignored.
pub fn detect_paths(text: &str) -> Vec<PathHit> {
    let bytes = text.as_bytes();
    let n = bytes.len();
    let mut hits = Vec::new();
    let mut i = 0;
    while i < n {
        if !is_path_char(bytes[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < n && is_path_char(bytes[i]) {
            i += 1;
        }
        // Trim trailing punctuation that tends to follow a path in prose.
        let mut end = i;
        while end > start
            && matches!(
                bytes[end - 1],
                b'.' | b',' | b';' | b')' | b']' | b'}' | b'>'
            )
        {
            end -= 1;
        }
        let token = &text[start..end];
        if !looks_like_path(token) {
            continue;
        }
        // Optional :line:col immediately after the (untrimmed) token.
        let mut line = None;
        let mut col = None;
        let mut ref_end = end;
        if end == i && i < n && bytes[i] == b':' {
            if let Some((l, ni)) = parse_uint(bytes, i + 1) {
                line = Some(l);
                ref_end = ni;
                i = ni;
                if i < n && bytes[i] == b':' {
                    if let Some((c, ni2)) = parse_uint(bytes, i + 1) {
                        col = Some(c);
                        ref_end = ni2;
                        i = ni2;
                    }
                }
            }
        }
        hits.push(PathHit {
            range: start..ref_end,
            path: token.to_string(),
            line,
            col,
        });
    }
    hits
}

/// The [`PathHit`] whose range contains byte `offset`, if any.
pub fn hit_at(hits: &[PathHit], offset: usize) -> Option<&PathHit> {
    hits.iter().find(|h| h.range.contains(&offset))
}

/// One-line reference the *Send selection to agent* command types into the
/// agent, so it can read the exact spot (and you can add your question after).
pub fn format_selection_context(rel_path: &str, start_line: usize, end_line: usize) -> String {
    if start_line >= end_line {
        format!("Regarding `{rel_path}` line {start_line}: ")
    } else {
        format!("Regarding `{rel_path}` lines {start_line}-{end_line}: ")
    }
}

/// Rebuild the exact plain text a terminal/agent screen renders (rows joined by
/// `\n`), so a click offset lines up with what [`detect_paths`] sees.
pub(crate) fn runs_text(runs: &[Vec<e_term::Run>]) -> String {
    let mut text = String::new();
    for (li, line) in runs.iter().enumerate() {
        if li > 0 {
            text.push('\n');
        }
        for (seg, _, _) in line {
            text.push_str(seg);
        }
    }
    text
}

/// 1-based line number of byte `offset` in `text`.
fn line_of(text: &str, offset: usize) -> usize {
    let end = offset.min(text.len());
    text[..end].bytes().filter(|&b| b == b'\n').count() + 1
}

impl AppState {
    /// Click at `offset` in the agent output — open a file link if one was clicked.
    pub fn agent_open_link_at(&self, offset: usize) {
        if !self.settings.get_untracked().editor_integration {
            return;
        }
        let text = runs_text(&self.agent_runs());
        self.open_link_in_text(&text, offset);
    }

    /// Click at `offset` in a terminal pane — open a file link if one was clicked.
    pub fn term_open_link_at(&self, id: Option<u64>, offset: usize) {
        if !self.settings.get_untracked().editor_integration {
            return;
        }
        let text = runs_text(&self.term_runs_of(id));
        self.open_link_in_text(&text, offset);
    }

    fn open_link_in_text(&self, text: &str, offset: usize) {
        let hits = detect_paths(text);
        let Some(hit) = hit_at(&hits, offset) else {
            return;
        };
        let p = PathBuf::from(&hit.path);
        let abs = if p.is_absolute() {
            p
        } else {
            self.root.get_untracked().join(&p)
        };
        if !abs.is_file() {
            return;
        }
        let uri = format!("file://{}", abs.display());
        self.jump_to(
            &uri,
            hit.line.unwrap_or(1).max(1),
            hit.col.unwrap_or(1).max(1),
        );
    }

    /// Send the active editor selection (or caret line) to the agent as a
    /// one-line reference, then focus it so you can add your question.
    pub fn send_selection_to_agent(&self) {
        if !self.settings.get_untracked().editor_integration {
            return;
        }
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let Some(path) = buf.file.path.clone() else {
            Self::notify("Save the file before sending it to the agent");
            return;
        };
        let root = self.root.get_untracked();
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .display()
            .to_string();
        let doc_text = buf.doc.text().to_string();
        let (start_off, end_off) = buf
            .editor
            .get_untracked()
            .and_then(|editor| {
                let cursor = editor.cursor.get_untracked();
                if let CursorMode::Insert(sel) = &cursor.mode {
                    sel.regions()
                        .iter()
                        .find(|r| r.min() != r.max())
                        .or_else(|| sel.regions().first())
                        .map(|r| (r.min(), r.max()))
                } else {
                    None
                }
            })
            .unwrap_or((0, 0));
        let msg = format_selection_context(
            &rel,
            line_of(&doc_text, start_off),
            line_of(&doc_text, end_off),
        );
        self.insert_into_agent(&msg);
    }

    /// Type `msg` into the agent input without submitting, so the user can add
    /// their question, and focus the panel.
    fn insert_into_agent(&self, msg: &str) {
        if !self.agent_open.get_untracked() {
            self.agent_open.set(true);
        }
        if self.use_native_agent() {
            self.agent_composer.update(|c| c.push_str(msg));
            self.agent_focus_pulse.update(|x| *x += 1);
            return;
        }
        let just_started = self.agent_term.get_untracked().is_none();
        if just_started {
            self.start_agent();
        }
        let bytes = msg.as_bytes().to_vec();
        let state = *self;
        let delay = if just_started { 700 } else { 60 };
        floem::action::exec_after(Duration::from_millis(delay), move |_| {
            state.agent_input(&bytes);
            state.agent_focus_pulse.update(|x| *x += 1);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_bare_path() {
        let hits = detect_paths("edit packages/server-laravel/src/GridEngine.php");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "packages/server-laravel/src/GridEngine.php");
        assert_eq!(hits[0].line, None);
    }

    #[test]
    fn finds_path_with_line_and_col() {
        let hits = detect_paths("see app/Models/User.php:42:9 now");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "app/Models/User.php");
        assert_eq!(hits[0].line, Some(42));
        assert_eq!(hits[0].col, Some(9));
        // The range covers the whole `path:line:col`.
        assert_eq!(
            &"see app/Models/User.php:42:9 now"[hits[0].range.clone()],
            "app/Models/User.php:42:9"
        );
    }

    #[test]
    fn ignores_code_without_extension() {
        // The regex literal has a slash but no file extension.
        let hits = detect_paths("$host = preg_replace('/:\\d+$/', '', $host);");
        assert!(hits.is_empty(), "got {hits:?}");
    }

    #[test]
    fn ignores_bare_word_and_urls() {
        assert!(detect_paths("hello.world is fine").is_empty());
        assert!(detect_paths("https://example.com/index.php").is_empty());
    }

    #[test]
    fn trims_trailing_period() {
        let text = "config at packages/x/phpstan.neon.";
        let hits = detect_paths(text);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "packages/x/phpstan.neon");
    }

    #[test]
    fn hit_lookup_by_offset() {
        let text = "a app/X.php b";
        let hits = detect_paths(text);
        let off = text.find("X.php").unwrap();
        assert_eq!(hit_at(&hits, off).unwrap().path, "app/X.php");
        assert!(hit_at(&hits, 0).is_none());
    }

    #[test]
    fn selection_context_singular_and_range() {
        assert_eq!(
            format_selection_context("app/User.php", 5, 5),
            "Regarding `app/User.php` line 5: "
        );
        assert_eq!(
            format_selection_context("app/User.php", 5, 9),
            "Regarding `app/User.php` lines 5-9: "
        );
    }
}
