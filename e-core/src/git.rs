//! Minimal git integration: compare a buffer against its `HEAD` version to
//! produce per-line change markers (for the editor gutter).

use std::path::Path;
use std::process::Command;

use similar::{ChangeTag, DiffOp, TextDiff};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineMark {
    Added,
    Modified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffKind {
    Context,
    Added,
    Removed,
}

#[derive(Debug, Clone)]
pub struct DiffLine {
    pub kind: DiffKind,
    pub text: String,
}

/// A unified line-diff between `head` and `current`.
pub fn diff(head: &str, current: &str) -> Vec<DiffLine> {
    TextDiff::from_lines(head, current)
        .iter_all_changes()
        .map(|c| {
            let kind = match c.tag() {
                ChangeTag::Insert => DiffKind::Added,
                ChangeTag::Delete => DiffKind::Removed,
                ChangeTag::Equal => DiffKind::Context,
            };
            DiffLine {
                kind,
                text: c.value().trim_end_matches('\n').to_string(),
            }
        })
        .collect()
}

/// Fetch the `HEAD` version of `path`, or `None` if it's untracked / not a repo.
pub fn head_text(path: &Path) -> Option<String> {
    let dir = path.parent()?;
    let name = path.file_name()?.to_str()?;
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .arg("show")
        .arg(format!("HEAD:./{name}"))
        .output()
        .ok()?;
    if output.status.success() {
        String::from_utf8(output.stdout).ok()
    } else {
        None
    }
}

/// Compute per-line markers comparing `head` to `current`.
/// The returned vector has one slot per line of `current`.
pub fn marks(head: &str, current: &str, line_count: usize) -> Vec<Option<LineMark>> {
    let mut marks = vec![None; line_count.max(1)];
    let diff = TextDiff::from_lines(head, current);
    for op in diff.ops() {
        match *op {
            DiffOp::Insert {
                new_index, new_len, ..
            } => {
                for l in new_index..new_index + new_len {
                    if let Some(slot) = marks.get_mut(l) {
                        *slot = Some(LineMark::Added);
                    }
                }
            }
            DiffOp::Replace {
                new_index, new_len, ..
            } => {
                for l in new_index..new_index + new_len {
                    if let Some(slot) = marks.get_mut(l) {
                        *slot = Some(LineMark::Modified);
                    }
                }
            }
            DiffOp::Delete { new_index, .. } => {
                // Show deletions as a modification marker on the following line.
                if let Some(slot) = marks.get_mut(new_index) {
                    if slot.is_none() {
                        *slot = Some(LineMark::Modified);
                    }
                }
            }
            DiffOp::Equal { .. } => {}
        }
    }
    marks
}

#[cfg(test)]
mod tests {
    use super::{diff, marks, DiffKind, LineMark};

    #[test]
    fn diff_marks_changes() {
        let head = "a\nb\nc\n";
        let cur = "a\nB\nc\nd\n";
        let m = marks(head, cur, 4);
        assert_eq!(m[0], None);
        assert_eq!(m[1], Some(LineMark::Modified));
        assert_eq!(m[3], Some(LineMark::Added));
    }

    #[test]
    fn diff_lines_have_signs() {
        let d = diff("x\ny\n", "x\nY\n");
        assert!(d
            .iter()
            .any(|l| l.kind == DiffKind::Removed && l.text == "y"));
        assert!(d.iter().any(|l| l.kind == DiffKind::Added && l.text == "Y"));
    }
}
