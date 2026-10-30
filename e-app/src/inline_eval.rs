//! `⌘⏎` on a PHP expression — or a selection — evaluates it in the running
//! app and shows the value at the end of its line, with the queries it ran
//! (see [`e_laravel::inline_eval`]). `⌘.` there shows it all in the Tinker
//! panel, or clears it. A result stays with its line as the code moves, and
//! goes when the line changes.

use e_laravel::inline_eval::{self, Evaluated};
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use floem::views::editor::core::cursor::CursorMode;
use floem::views::editor::text::Document;

use crate::state::AppState;

/// A result at the end of a line.
#[derive(Clone, Debug, PartialEq)]
pub struct EvalMark {
    /// 0-based line it shows on.
    pub line: usize,
    /// The line's text when evaluated: the mark follows it, and goes when it
    /// changes.
    pub anchor: String,
    /// What the line end says.
    pub label: String,
    pub error: bool,
    /// Everything, for the Tinker panel; empty while it runs.
    pub report: String,
}

/// Where the line `anchor`, last on `line`, is in `text` now: there, else the
/// nearest line with the same text. `None` when it is gone.
fn follow(text: &str, line: usize, anchor: &str) -> Option<usize> {
    let lines: Vec<&str> = text.lines().collect();
    if lines.get(line) == Some(&anchor) {
        return Some(line);
    }
    (1..=200).find_map(|d| {
        [line.checked_sub(d), Some(line + d)]
            .into_iter()
            .flatten()
            .find(|&l| lines.get(l) == Some(&anchor))
    })
}

impl AppState {
    /// `⌘⏎`: the SQL string under the caret runs as SQL; anywhere else in a
    /// Laravel project's PHP, the expression there is evaluated.
    pub fn run_at_caret(&self) {
        let in_sql = self.active_buffer().is_some_and(|buf| {
            buf.file.language == e_core::language::Language::Php
                && buf.editor.get_untracked().is_some_and(|ed| {
                    let text = buf.doc.text().to_string();
                    e_core::syntax::php_sql_range_at(&text, ed.cursor.get_untracked().offset())
                        .is_some()
                })
        });
        let laravel = self.root.get_untracked().join("artisan").is_file();
        if in_sql || !laravel {
            self.run_sql_under_cursor();
        } else {
            self.evaluate_at_caret();
        }
    }

    /// Evaluate the selection, or the statement the caret is in, and show
    /// the value at the end of its last line.
    pub fn evaluate_at_caret(&self) {
        let Some(buf) = self.active_buffer() else {
            return;
        };
        if buf.file.language != e_core::language::Language::Php {
            Self::notify("Evaluate: put the caret on a PHP expression");
            return;
        }
        let root = self.root.get_untracked();
        if !root.join("artisan").is_file() {
            Self::notify("Evaluate: not a Laravel project (no artisan)");
            return;
        }
        let Some(editor) = buf.editor.get_untracked() else {
            return;
        };
        let text = buf.doc.text().to_string();
        let cursor = editor.cursor.get_untracked();
        let selected = match &cursor.mode {
            CursorMode::Insert(sel) => sel
                .regions()
                .iter()
                .find(|r| r.min() != r.max())
                .map(|r| r.min().min(text.len())..r.max().min(text.len())),
            _ => None,
        };
        let Some(range) = selected.or_else(|| inline_eval::statement_at(&text, cursor.offset()))
        else {
            Self::notify("Evaluate: put the caret on a PHP expression");
            return;
        };
        let code = match inline_eval::expression(&text[range.clone()]) {
            Ok(code) => code,
            Err(e) => {
                Self::notify(&format!("Evaluate: {e}"));
                return;
            }
        };
        let line = text[..range.end].matches('\n').count();
        let anchor = text.lines().nth(line).unwrap_or("").to_string();
        let marks = buf.evals;
        let pending = EvalMark {
            line,
            anchor: anchor.clone(),
            label: "⋯ evaluating".to_string(),
            error: false,
            report: String::new(),
        };
        marks.update(|m| {
            m.retain(|e| e.line != line);
            m.push(pending);
        });
        let doc = buf.doc.clone();
        self.spawn_bg(
            move || inline_eval::run(&root, &text, &code).map(|e| (e.report(&code), e)),
            move |res: Result<(String, Evaluated), String>| {
                let (label, error, report) = match res {
                    Ok((report, e)) => (e.label(), e.error.is_some(), report),
                    Err(e) => (format!("✖ {e}"), true, e),
                };
                marks.update(|m| {
                    // Where its line is now; gone if it changed meanwhile.
                    let now = doc.text().to_string();
                    m.retain(|e| !(e.anchor == anchor && e.label == "⋯ evaluating"));
                    if let Some(line) = follow(&now, line, &anchor) {
                        m.retain(|e| e.line != line);
                        m.push(EvalMark {
                            line,
                            anchor,
                            label,
                            error,
                            report,
                        });
                    }
                });
                doc.cache_rev().update(|r| *r += 1);
            },
        );
        buf.doc.cache_rev().update(|r| *r += 1);
    }

    /// The buffer changed: results follow their lines, and go with them.
    pub(crate) fn follow_evals(&self, buffer_id: u64) {
        let Some(buf) = self.buffer_by_id(buffer_id) else {
            return;
        };
        if buf.evals.with_untracked(Vec::is_empty) {
            return;
        }
        let text = buf.doc.text().to_string();
        let moved: Vec<EvalMark> = buf.evals.with_untracked(|m| {
            m.iter()
                .filter_map(|e| {
                    let line = follow(&text, e.line, &e.anchor)?;
                    Some(EvalMark { line, ..e.clone() })
                })
                .collect()
        });
        if buf.evals.with_untracked(|m| *m != moved) {
            buf.evals.set(moved);
        }
    }

    /// The result on `line` of the active buffer.
    pub(crate) fn eval_at(&self, line: usize) -> Option<EvalMark> {
        let buf = self.active_buffer()?;
        buf.evals.with_untracked(|m| {
            m.iter()
                .find(|e| e.line == line && !e.report.is_empty())
                .cloned()
        })
    }

    /// The result on `line` in full — value, queries, time — in the Tinker
    /// panel.
    pub(crate) fn show_eval(&self, line: usize) {
        if let Some(e) = self.eval_at(line) {
            self.tinker_output.set(e.report);
            self.tinker_open.set(true);
        }
    }

    pub(crate) fn clear_eval(&self, line: usize) {
        if let Some(buf) = self.active_buffer() {
            buf.evals.update(|m| m.retain(|e| e.line != line));
            buf.doc.cache_rev().update(|r| *r += 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_result_follows_its_line_and_goes_with_it() {
        let text = "a\nb\nOrder::count();\nc\n";
        assert_eq!(follow(text, 2, "Order::count();"), Some(2));
        let moved = "x\ny\na\nb\nOrder::count();\nc\n";
        assert_eq!(follow(moved, 2, "Order::count();"), Some(4));
        let changed = "a\nb\nOrder::count() + 1;\nc\n";
        assert_eq!(follow(changed, 2, "Order::count();"), None);
    }
}
