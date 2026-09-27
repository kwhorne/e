//! Writing Markdown: the editor keeps lists going, toggles emphasis and
//! checkboxes, and makes links — the small things that make a Markdown editor
//! feel like one. The rules are pure functions on lines and selections, tested
//! below; `AppState` applies them to the active buffer.

use floem::reactive::{SignalGet, SignalUpdate};
use floem::views::editor::core::cursor::{Cursor, CursorMode};
use floem::views::editor::core::editor::EditType;
use floem::views::editor::core::selection::Selection;
use floem::views::editor::text::Document;

use e_core::language::Language;

use crate::state::AppState;

/// What Enter does at the end of a list or quote line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Continuation {
    /// Start the next item: insert a newline and this prefix.
    Next(String),
    /// The item was empty: end the list by clearing the marker (`len` bytes
    /// from the start of the line) instead of adding another.
    End { marker_len: usize },
}

/// The prefix a line starts with, if it is a list item, a task or a quote:
/// `(prefix as written, indent, is the rest of the line empty)`.
fn list_prefix(line: &str) -> Option<(String, bool)> {
    let indent_len = line.len() - line.trim_start().len();
    let (indent, rest) = line.split_at(indent_len);
    // Quote: `> `
    if let Some(after) = rest.strip_prefix("> ") {
        return Some((format!("{indent}> "), after.trim().is_empty()));
    }
    if rest == ">" {
        return Some((format!("{indent}> "), true));
    }
    // Task or bullet: `- [ ] `, `- [x] `, `- `, `* `, `+ `
    for bullet in ['-', '*', '+'] {
        let Some(after) = rest.strip_prefix(bullet).and_then(|r| r.strip_prefix(' ')) else {
            continue;
        };
        if let Some(task_rest) = after
            .strip_prefix("[ ] ")
            .or_else(|| after.strip_prefix("[x] "))
            .or_else(|| after.strip_prefix("[X] "))
            .or_else(|| {
                if after == "[ ]" || after == "[x]" || after == "[X]" {
                    Some("")
                } else {
                    None
                }
            })
        {
            return Some((
                format!("{indent}{bullet} [ ] "),
                task_rest.trim().is_empty(),
            ));
        }
        return Some((format!("{indent}{bullet} "), after.trim().is_empty()));
    }
    // Ordered: `1. ` or `1) `
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if !digits.is_empty() {
        let after_digits = &rest[digits.len()..];
        for sep in ['.', ')'] {
            if let Some(after) = after_digits
                .strip_prefix(sep)
                .and_then(|r| r.strip_prefix(' '))
            {
                let n: u64 = digits.parse().unwrap_or(0);
                return Some((format!("{indent}{}{sep} ", n + 1), after.trim().is_empty()));
            }
        }
    }
    None
}

/// Enter on `line` (the caret is at its end): continue the list, end it, or
/// `None` for a plain newline.
pub fn continue_list(line: &str) -> Option<Continuation> {
    let (prefix, empty) = list_prefix(line)?;
    if empty {
        // The marker as written on this line, not the next one's.
        let indent_len = line.len() - line.trim_start().len();
        Some(Continuation::End {
            marker_len: line.len().max(indent_len),
        })
    } else {
        Some(Continuation::Next(prefix))
    }
}

/// Wrap `selected` in `marker` … `marker`, or unwrap it when it already is.
/// Returns the replacement and where the caret/selection should land inside it
/// (`start..end`, relative to the replacement).
pub fn toggle_wrap(selected: &str, marker: &str) -> (String, usize, usize) {
    if selected.len() >= 2 * marker.len()
        && selected.starts_with(marker)
        && selected.ends_with(marker)
    {
        let inner = &selected[marker.len()..selected.len() - marker.len()];
        return (inner.to_string(), 0, inner.len());
    }
    let out = format!("{marker}{selected}{marker}");
    (out, marker.len(), marker.len() + selected.len())
}

/// `- [ ] x` ↔ `- [x] x`; a plain bullet gets a box; a plain line becomes a task.
pub fn toggle_task(line: &str) -> String {
    let indent_len = line.len() - line.trim_start().len();
    let (indent, rest) = line.split_at(indent_len);
    for bullet in ['-', '*', '+'] {
        if let Some(after) = rest.strip_prefix(bullet).and_then(|r| r.strip_prefix(' ')) {
            if let Some(r) = after.strip_prefix("[ ] ") {
                return format!("{indent}{bullet} [x] {r}");
            }
            if let Some(r) = after
                .strip_prefix("[x] ")
                .or_else(|| after.strip_prefix("[X] "))
            {
                return format!("{indent}{bullet} [ ] {r}");
            }
            return format!("{indent}{bullet} [ ] {after}");
        }
    }
    format!("{indent}- [ ] {rest}")
}

/// A link from the selection and (when it looks like one) the clipboard URL:
/// the replacement and the caret's range inside it — on the URL when there is
/// none to fill in, after the link otherwise.
pub fn make_link(selected: &str, clipboard: Option<&str>) -> (String, usize, usize) {
    let url = clipboard
        .map(str::trim)
        .filter(|c| {
            c.starts_with("http://") || c.starts_with("https://") || c.starts_with("mailto:")
        })
        .unwrap_or("");
    let text = if selected.is_empty() {
        "link text"
    } else {
        selected
    };
    let out = format!("[{text}]({url})");
    if url.is_empty() {
        let at = 1 + text.len() + 2; // inside the ()
        (out, at, at)
    } else if selected.is_empty() {
        (out, 1, 1 + text.len())
    } else {
        let end = out.len();
        (out, end, end)
    }
}

impl AppState {
    fn markdown_buffer(&self) -> Option<crate::state::Buffer> {
        let buf = self.active_buffer()?;
        (buf.file.language == Language::Markdown).then_some(buf)
    }

    /// Enter in a Markdown buffer: continue or end a list. `false` leaves the
    /// newline to the editor.
    pub fn markdown_newline(&self) -> bool {
        let Some(buf) = self.markdown_buffer() else {
            return false;
        };
        let Some(editor) = buf.editor.get_untracked() else {
            return false;
        };
        let cursor = editor.cursor.get_untracked();
        let CursorMode::Insert(sel) = cursor.mode.clone() else {
            return false;
        };
        let regions = sel.regions();
        if regions.len() != 1 || regions[0].min() != regions[0].max() {
            return false;
        }
        let off = regions[0].min();
        let text = buf.doc.text().to_string();
        let line_start = text[..off].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let line_end = text[off..]
            .find('\n')
            .map(|i| off + i)
            .unwrap_or(text.len());
        // Only at the end of the line: mid-line Enter is a plain split.
        if off != line_end {
            return false;
        }
        let line = &text[line_start..line_end];
        match continue_list(line) {
            Some(Continuation::Next(prefix)) => {
                let insert = format!("\n{prefix}");
                let mut it = std::iter::once((Selection::caret(off), insert.as_str()));
                buf.doc.edit(&mut it, EditType::InsertNewline);
                let at = off + insert.len();
                editor.cursor.set(Cursor::new(
                    CursorMode::Insert(Selection::caret(at)),
                    None,
                    None,
                ));
                true
            }
            Some(Continuation::End { marker_len }) => {
                let indent_len = line.len() - line.trim_start().len();
                let indent = line[..indent_len].to_string();
                let mut it = std::iter::once((
                    Selection::region(line_start, line_start + marker_len),
                    indent.as_str(),
                ));
                buf.doc.edit(&mut it, EditType::Delete);
                editor.cursor.set(Cursor::new(
                    CursorMode::Insert(Selection::caret(line_start + indent.len())),
                    None,
                    None,
                ));
                true
            }
            None => false,
        }
    }

    /// Replace the selection (or insert at the caret) and place the caret or
    /// selection at `start..end` inside the replacement.
    fn markdown_replace_selection(&self, f: impl FnOnce(&str) -> (String, usize, usize)) {
        let Some(buf) = self.markdown_buffer() else {
            return;
        };
        let Some(editor) = buf.editor.get_untracked() else {
            return;
        };
        let cursor = editor.cursor.get_untracked();
        let CursorMode::Insert(sel) = cursor.mode.clone() else {
            return;
        };
        let regions = sel.regions();
        if regions.len() != 1 {
            return;
        }
        let (s, e) = (regions[0].min(), regions[0].max());
        let text = buf.doc.text().to_string();
        let (replacement, start, end) = f(&text[s..e]);
        let mut it = std::iter::once((Selection::region(s, e), replacement.as_str()));
        buf.doc.edit(&mut it, EditType::InsertChars);
        let mode = if start == end {
            CursorMode::Insert(Selection::caret(s + start))
        } else {
            CursorMode::Insert(Selection::region(s + start, s + end))
        };
        editor.cursor.set(Cursor::new(mode, None, None));
    }

    pub fn markdown_bold(&self) {
        self.markdown_replace_selection(|sel| toggle_wrap(sel, "**"));
    }

    pub fn markdown_italic(&self) {
        self.markdown_replace_selection(|sel| toggle_wrap(sel, "_"));
    }

    pub fn markdown_code(&self) {
        self.markdown_replace_selection(|sel| toggle_wrap(sel, "`"));
    }

    pub fn markdown_link(&self) {
        let clipboard = floem::Clipboard::get_contents().ok();
        self.markdown_replace_selection(move |sel| make_link(sel, clipboard.as_deref()));
    }

    /// Toggle the task box on the caret's line.
    pub fn markdown_toggle_task(&self) {
        let Some(buf) = self.markdown_buffer() else {
            return;
        };
        let Some(editor) = buf.editor.get_untracked() else {
            return;
        };
        let off = editor.cursor.get_untracked().offset();
        let text = buf.doc.text().to_string();
        let line_start = text[..off].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let line_end = text[off..]
            .find('\n')
            .map(|i| off + i)
            .unwrap_or(text.len());
        let line = &text[line_start..line_end];
        let new_line = toggle_task(line);
        let delta = new_line.len() as isize - line.len() as isize;
        let mut it = std::iter::once((Selection::region(line_start, line_end), new_line.as_str()));
        buf.doc.edit(&mut it, EditType::InsertChars);
        let at = ((off as isize) + delta).max(line_start as isize) as usize;
        editor.cursor.set(Cursor::new(
            CursorMode::Insert(Selection::caret(at)),
            None,
            None,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_continue_and_end() {
        assert_eq!(
            continue_list("- one"),
            Some(Continuation::Next("- ".into()))
        );
        assert_eq!(
            continue_list("  * two"),
            Some(Continuation::Next("  * ".into()))
        );
        assert_eq!(
            continue_list("3. three"),
            Some(Continuation::Next("4. ".into()))
        );
        assert_eq!(
            continue_list("1) x"),
            Some(Continuation::Next("2) ".into()))
        );
        assert_eq!(
            continue_list("- [x] done"),
            Some(Continuation::Next("- [ ] ".into()))
        );
        assert_eq!(
            continue_list("- [ ] todo"),
            Some(Continuation::Next("- [ ] ".into()))
        );
        assert_eq!(
            continue_list("> quoted"),
            Some(Continuation::Next("> ".into()))
        );
        assert_eq!(
            continue_list("- "),
            Some(Continuation::End { marker_len: 2 })
        );
        assert_eq!(
            continue_list("  2. "),
            Some(Continuation::End { marker_len: 5 })
        );
        assert_eq!(
            continue_list("- [ ] "),
            Some(Continuation::End { marker_len: 6 })
        );
        assert_eq!(continue_list("plain text"), None);
        assert_eq!(continue_list("-not a list"), None);
        assert_eq!(continue_list("2024 was a year"), None);
    }

    #[test]
    fn wrapping_toggles() {
        assert_eq!(toggle_wrap("word", "**"), ("**word**".into(), 2, 6));
        assert_eq!(toggle_wrap("**word**", "**"), ("word".into(), 0, 4));
        assert_eq!(toggle_wrap("", "_"), ("__".into(), 1, 1));
        assert_eq!(toggle_wrap("`", "`"), ("```".into(), 1, 2));
    }

    #[test]
    fn tasks_toggle() {
        assert_eq!(toggle_task("- [ ] buy milk"), "- [x] buy milk");
        assert_eq!(toggle_task("- [x] buy milk"), "- [ ] buy milk");
        assert_eq!(toggle_task("  * item"), "  * [ ] item");
        assert_eq!(toggle_task("plain"), "- [ ] plain");
    }

    #[test]
    fn links_use_the_clipboard_when_it_is_a_url() {
        assert_eq!(
            make_link("docs", Some("https://e.dev/docs")),
            ("[docs](https://e.dev/docs)".into(), 26, 26)
        );
        assert_eq!(
            make_link("docs", Some("not a url")),
            ("[docs]()".into(), 7, 7)
        );
        assert_eq!(
            make_link("", Some("https://x.y")),
            ("[link text](https://x.y)".into(), 1, 10)
        );
    }
}
