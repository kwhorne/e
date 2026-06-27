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

/// One entry from `git status --porcelain`.
#[derive(Debug, Clone)]
pub struct StatusEntry {
    /// Path relative to the repository root.
    pub path: String,
    /// Index (staged) status code, e.g. `M`, `A`, `D`, `R`, or ` `.
    pub index: char,
    /// Work-tree (unstaged) status code, e.g. `M`, `D`, `?`, or ` `.
    pub worktree: char,
}

impl StatusEntry {
    pub fn is_staged(&self) -> bool {
        self.index != ' ' && self.index != '?'
    }
    pub fn is_unstaged(&self) -> bool {
        self.worktree != ' '
    }
    pub fn is_untracked(&self) -> bool {
        self.index == '?' && self.worktree == '?'
    }
    /// A single-letter badge for the UI.
    pub fn badge(&self) -> char {
        if self.is_untracked() {
            'U'
        } else if self.is_staged() {
            self.index
        } else {
            self.worktree
        }
    }
}

fn git_root(path: &Path) -> Option<std::path::PathBuf> {
    let out = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?;
    Some(std::path::PathBuf::from(s.trim()))
}

/// The current branch name (or a short commit hash when detached).
pub fn current_branch(root: &Path) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let name = String::from_utf8(out.stdout).ok()?.trim().to_string();
    if name == "HEAD" {
        // Detached: show the short hash instead.
        let h = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["rev-parse", "--short", "HEAD"])
            .output()
            .ok()?;
        Some(format!("({})", String::from_utf8(h.stdout).ok()?.trim()))
    } else if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

/// Parse `git status --porcelain` into per-file entries.
pub fn status(root: &Path) -> Vec<StatusEntry> {
    let out = match Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["status", "--porcelain"])
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut entries = Vec::new();
    for line in text.lines() {
        if line.len() < 3 {
            continue;
        }
        let bytes: Vec<char> = line.chars().collect();
        let index = bytes[0];
        let worktree = bytes[1];
        let mut path = line[3..].to_string();
        // Renames are shown as "old -> new"; keep the new path.
        if let Some(pos) = path.find(" -> ") {
            path = path[pos + 4..].to_string();
        }
        let path = path.trim_matches('"').to_string();
        entries.push(StatusEntry {
            index,
            worktree,
            path,
        });
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    entries
}

fn run_git(root: &Path, args: &[&str]) -> Result<(), String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

pub fn stage(root: &Path, path: &str) -> Result<(), String> {
    run_git(root, &["add", "--", path])
}

pub fn unstage(root: &Path, path: &str) -> Result<(), String> {
    run_git(root, &["reset", "-q", "HEAD", "--", path])
}

pub fn stage_all(root: &Path) -> Result<(), String> {
    run_git(root, &["add", "-A"])
}

pub fn unstage_all(root: &Path) -> Result<(), String> {
    run_git(root, &["reset", "-q", "HEAD"])
}

pub fn commit(root: &Path, message: &str) -> Result<(), String> {
    run_git(root, &["commit", "-m", message])
}

pub fn push(root: &Path) -> Result<(), String> {
    run_git(root, &["push"])
}

pub fn pull(root: &Path) -> Result<(), String> {
    run_git(root, &["pull", "--ff-only"])
}

/// Discard work-tree changes for a single file (`git checkout -- <path>`).
pub fn discard(root: &Path, path: &str) -> Result<(), String> {
    run_git(root, &["checkout", "--", path])
}

/// The repository root for a workspace path, if any.
pub fn repo_root(path: &Path) -> Option<std::path::PathBuf> {
    git_root(path)
}

/// Per-line blame: `(author, unix_time, summary)` for each line of `path`.
/// Uncommitted lines yield `("You", 0, "Uncommitted changes")`.
pub fn blame(path: &Path) -> Vec<(String, i64, String)> {
    let Some(dir) = path.parent() else {
        return Vec::new();
    };
    let out = match Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["blame", "--line-porcelain", "--"])
        .arg(path)
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut result = Vec::new();
    let (mut author, mut time, mut summary) = (String::new(), 0i64, String::new());
    for line in text.lines() {
        if let Some(a) = line.strip_prefix("author ") {
            author = a.to_string();
        } else if let Some(t) = line.strip_prefix("author-time ") {
            time = t.trim().parse().unwrap_or(0);
        } else if let Some(s) = line.strip_prefix("summary ") {
            summary = s.to_string();
        } else if line.starts_with('\t') {
            // The actual source line terminates a blame group.
            let (a, s) = if author == "Not Committed Yet" {
                ("You".to_string(), "Uncommitted changes".to_string())
            } else {
                (author.clone(), summary.clone())
            };
            result.push((a, time, s));
        }
    }
    result
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
