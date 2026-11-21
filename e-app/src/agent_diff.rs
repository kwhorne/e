//! The small diff a tool card shows for a file the agent changed: the lines
//! taken out and put in, with a line of context around each change, and the
//! file's name to click to open it there.

use std::path::Path;

use e_agent::FileDiff;
use floem::views::{dyn_container, label, stack, v_stack_from_iter, Decorators};
use floem::IntoView;

use crate::state::AppState;
use crate::theme;

/// Lines a closed card shows of a file's diff.
const SHOWN: usize = 12;
/// Lines an open card shows: enough for any edit worth reading here.
const MOST: usize = 400;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    Added,
    Removed,
    Same,
    /// Lines left out between two changes.
    Gap,
}

/// The diff of `old` and `new`, line by line, with `context` unchanged lines
/// around each change.
pub fn diff_lines(old: &str, new: &str, context: usize) -> Vec<(Mark, String)> {
    let diff = similar::TextDiff::from_lines(old, new);
    let mut out = Vec::new();
    for (n, group) in diff.grouped_ops(context).iter().enumerate() {
        if n > 0 {
            out.push((Mark::Gap, String::new()));
        }
        for op in group {
            for change in diff.iter_changes(op) {
                let mark = match change.tag() {
                    similar::ChangeTag::Insert => Mark::Added,
                    similar::ChangeTag::Delete => Mark::Removed,
                    similar::ChangeTag::Equal => Mark::Same,
                };
                let text = change.value().trim_end_matches(['\n', '\r']).to_string();
                out.push((mark, text));
            }
        }
    }
    out
}

/// Lines added and removed.
pub fn counts(lines: &[(Mark, String)]) -> (usize, usize) {
    let n = |m: Mark| lines.iter().filter(|(k, _)| *k == m).count();
    (n(Mark::Added), n(Mark::Removed))
}

/// The line (1-based) in `file` where the change is: the first line it
/// added, found in the file as it is now — the diff may be of a part of it.
/// For a change that only took lines out, the line before them.
pub fn line_in(file: &str, lines: &[(Mark, String)]) -> usize {
    let first = lines
        .iter()
        .position(|(m, _)| matches!(m, Mark::Added | Mark::Removed));
    let Some(first) = first else {
        return 1;
    };
    let anchor = lines[first..]
        .iter()
        .take_while(|(m, _)| matches!(m, Mark::Added | Mark::Removed))
        .find(|(m, t)| *m == Mark::Added && !t.trim().is_empty())
        .or_else(|| {
            lines[..first]
                .iter()
                .rev()
                .find(|(m, t)| *m == Mark::Same && !t.trim().is_empty())
        });
    let Some((_, text)) = anchor else {
        return 1;
    };
    file.lines()
        .position(|l| l.trim_end() == text.trim_end())
        .map(|i| i + 1)
        .unwrap_or(1)
}

impl AppState {
    /// Open `path` (as the agent named it) at the change `lines` describe.
    pub fn open_agent_diff(&self, path: &str, lines: &[(Mark, String)]) {
        let p = Path::new(path);
        let abs = if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.agent_root().join(p)
        };
        if !abs.is_file() {
            crate::toast::post(&format!("{path} isn't there any more"));
            return;
        }
        let line = std::fs::read_to_string(&abs)
            .map(|text| line_in(&text, lines))
            .unwrap_or(1);
        self.jump_to(&format!("file://{}", abs.display()), line, 1);
    }
}

/// The diffs of a tool call: for each file its name with the counts, to
/// click, and its changed lines — a few while `open` is off, the rest when on.
pub fn diffs_view(
    state: AppState,
    diffs: impl Fn() -> Vec<FileDiff> + 'static,
    open: impl Fn() -> bool + 'static,
) -> impl IntoView {
    dyn_container(
        move || (diffs(), open()),
        move |(diffs, open)| {
            v_stack_from_iter(diffs.into_iter().map(move |d| file_view(state, d, open)))
                .style(|s| s.flex_col().min_width(0.0).gap(4.0))
        },
    )
    .style(|s| s.min_width(0.0).margin_top(4.0))
}

fn file_view(state: AppState, d: FileDiff, open: bool) -> impl IntoView {
    let lines = diff_lines(&d.old, &d.new, 1);
    let (added, removed) = counts(&lines);
    let title = format!("{}  +{added} −{removed}", d.path);
    let at = lines.clone();
    let path = d.path.clone();
    let head = label(move || title.clone())
        .style(|s| {
            s.font_family(theme::mono_family())
                .font_size(11.0)
                .color(theme::accent())
                .cursor(floem::style::CursorStyle::Pointer)
        })
        .on_click_stop(move |_| state.open_agent_diff(&path, &at));
    let limit = if open { MOST } else { SHOWN };
    let more = lines.len().saturating_sub(limit);
    let rows = lines.into_iter().take(limit).map(|(mark, text)| {
        let shown = match mark {
            Mark::Added => format!("+ {text}"),
            Mark::Removed => format!("- {text}"),
            Mark::Same => format!("  {text}"),
            Mark::Gap => "  ⋯".to_string(),
        };
        label(move || shown.clone())
            .label_style(|s| s.selectable(true))
            .style(move |s| {
                let s = s.font_family(theme::mono_family()).font_size(11.0);
                match mark {
                    Mark::Added => s.color(theme::success()),
                    Mark::Removed => s.color(theme::error()),
                    Mark::Same | Mark::Gap => s.color(theme::fg_dim()),
                }
            })
    });
    let tail = label(move || {
        if more == 0 {
            String::new()
        } else if open {
            format!("  … {more} more lines: open the file")
        } else {
            format!("  … {more} more lines")
        }
    })
    .style(move |s| {
        let s = s
            .font_family(theme::mono_family())
            .font_size(11.0)
            .color(theme::fg_dim());
        if more == 0 {
            s.hide()
        } else {
            s
        }
    });
    let body = v_stack_from_iter(rows).style(|s| s.flex_col().min_width(0.0));
    stack((head, body, tail)).style(|s| s.flex_col().min_width(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_change_with_a_line_of_context() {
        let old = "a\nb\nc\nd\ne\nf\ng\n";
        let new = "a\nb\nC\nd\ne\nf\ng\nh\n";
        let lines = diff_lines(old, new, 1);
        assert_eq!(
            lines,
            vec![
                (Mark::Same, "b".into()),
                (Mark::Removed, "c".into()),
                (Mark::Added, "C".into()),
                (Mark::Same, "d".into()),
                (Mark::Gap, String::new()),
                (Mark::Same, "g".into()),
                (Mark::Added, "h".into()),
            ]
        );
        assert_eq!(counts(&lines), (2, 1));
        // A new file: all of it added.
        assert_eq!(counts(&diff_lines("", "x\ny", 1)), (2, 0));
        assert!(diff_lines("same\n", "same\n", 1).is_empty());
    }

    #[test]
    fn the_change_is_found_where_it_is_now() {
        let file = "<?php\n\nclass A\n{\n    public $x = 2;\n}\n";
        // An edit of a part of the file: found by the line it put in.
        let lines = diff_lines("    public $x = 1;\n", "    public $x = 2;\n", 1);
        assert_eq!(line_in(file, &lines), 5);
        // Lines only taken out: the line before them.
        let lines = diff_lines("{\n    gone();\n}\n", "{\n}\n", 1);
        assert_eq!(line_in(file, &lines), 4);
        // Not there any more: the top.
        let lines = diff_lines("x\n", "y\n", 1);
        assert_eq!(line_in(file, &lines), 1);
    }
}
