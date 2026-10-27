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

/// Local branch names, current branch first.
pub fn branches(root: &Path) -> Vec<String> {
    let out = match Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["branch", "--format=%(refname:short)"])
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// Switch to an existing branch.
pub fn checkout(root: &Path, branch: &str) -> Result<(), String> {
    run_git(root, &["checkout", branch])
}

/// Create and switch to a new branch.
pub fn checkout_new(root: &Path, branch: &str) -> Result<(), String> {
    run_git(root, &["checkout", "-b", branch])
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

/// Suggest a Conventional Commits subject line from the staged (or, if nothing
/// is staged, all) changes. Best-effort heuristic — meant as an editable
/// starting point.
pub fn suggest_commit(root: &Path) -> String {
    let entries = status(root);
    let staged: Vec<&StatusEntry> = entries.iter().filter(|e| e.is_staged()).collect();
    let use_staged = !staged.is_empty();
    let items: Vec<&StatusEntry> = if use_staged {
        staged
    } else {
        entries.iter().collect()
    };
    if items.is_empty() {
        return String::new();
    }

    // (path, effective change code) where '?' counts as added.
    let mut files: Vec<(String, char)> = Vec::new();
    let (mut added, mut modified, mut deleted) = (0u32, 0u32, 0u32);
    for e in &items {
        let raw = if use_staged || (e.index != ' ' && e.index != '?') {
            e.index
        } else {
            e.worktree
        };
        let code = if raw == '?' { 'A' } else { raw };
        match code {
            'A' => added += 1,
            'D' => deleted += 1,
            _ => modified += 1,
        }
        files.push((e.path.clone(), code));
    }

    let ty = commit_type(&files, added);
    let scope = commit_scope(&files);
    let verb = if added > 0 && modified == 0 && deleted == 0 {
        "add"
    } else if deleted > 0 && added == 0 && modified == 0 {
        "remove"
    } else {
        "update"
    };
    let targets = commit_targets(&files);

    let mut subject = ty.to_string();
    if let Some(s) = scope {
        subject.push_str(&format!("({s})"));
    }
    subject.push_str(&format!(": {verb} {targets}"));
    subject
}

fn file_stem(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    let trimmed = name.trim_start_matches('.');
    let stem = trimmed.split('.').next().unwrap_or(trimmed);
    if stem.is_empty() {
        name.trim_start_matches('.').to_string()
    } else {
        stem.to_string()
    }
}

fn commit_type(files: &[(String, char)], added: u32) -> &'static str {
    let all = |pred: &dyn Fn(&str) -> bool| files.iter().all(|(p, _)| pred(&p.to_lowercase()));
    if all(&|p| p.ends_with(".md") || p.starts_with("docs/") || p.contains("/docs/")) {
        return "docs";
    }
    if all(&|p| {
        p.starts_with("tests/") || p.contains("/tests/") || p.contains("test") || p.contains("spec")
    }) {
        return "test";
    }
    if all(&|p| p.ends_with(".css") || p.ends_with(".scss") || p.ends_with(".sass")) {
        return "style";
    }
    if all(&|p| {
        p.ends_with("cargo.toml")
            || p.ends_with("cargo.lock")
            || p.ends_with("package.json")
            || p.ends_with("package-lock.json")
            || p.ends_with(".lock")
            || p.ends_with(".yml")
            || p.ends_with(".yaml")
            || p.ends_with(".toml")
            || p.ends_with("dockerfile")
            || p.ends_with(".gitignore")
            || p.starts_with(".github/")
    }) {
        return "chore";
    }
    if added > 0 {
        "feat"
    } else {
        "fix"
    }
}

fn commit_scope(files: &[(String, char)]) -> Option<String> {
    let first_seg = |p: &str| p.split('/').next().unwrap_or(p).to_string();
    let first = first_seg(&files[0].0);
    if files.iter().all(|(p, _)| first_seg(p) == first)
        && !first.is_empty()
        && !first.contains('.')
        && first != "src"
    {
        // Trim a leading crate prefix for readability (e-app -> app).
        Some(first.strip_prefix("e-").unwrap_or(&first).to_string())
    } else {
        None
    }
}

fn commit_targets(files: &[(String, char)]) -> String {
    let mut stems: Vec<String> = Vec::new();
    for (p, _) in files {
        let s = file_stem(p);
        if !s.is_empty() && !stems.contains(&s) {
            stems.push(s);
        }
    }
    match stems.len() {
        0 => "changes".to_string(),
        1..=3 => stems.join(", "),
        n => format!("{} and {} more", stems[..2].join(", "), n - 2),
    }
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

/// Recent commits: `(short hash, author, relative time, summary)`.
pub fn log(root: &Path, max: usize) -> Vec<(String, String, String, String)> {
    let out = match Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "log",
            &format!("-n{max}"),
            "--pretty=format:%h\x1f%an\x1f%ar\x1f%s",
        ])
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('\x1f');
            Some((
                parts.next()?.to_string(),
                parts.next()?.to_string(),
                parts.next()?.to_string(),
                parts.next().unwrap_or("").to_string(),
            ))
        })
        .collect()
}

pub fn stash_push(root: &Path) -> Result<(), String> {
    run_git(root, &["stash", "push", "-u"])
}

pub fn stash_pop(root: &Path) -> Result<(), String> {
    run_git(root, &["stash", "pop"])
}

/// Number of stash entries.
pub fn stash_count(root: &Path) -> usize {
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["stash", "list"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).lines().count())
        .unwrap_or(0)
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

// ---- Reversible working-tree checkpoints (verify-fix loop) ------------------

/// A capture of the working tree at a point in time, used to run a reversible
/// experiment: apply changes, then restore exactly what was here before.
#[derive(Clone, Debug, PartialEq)]
pub struct Checkpoint {
    /// HEAD commit at capture time.
    pub head: String,
    /// A `git stash create` object of any tracked work-in-progress (empty if the
    /// tree was clean), re-applied on restore so the user's own WIP survives.
    pub stash: Option<String>,
    /// Untracked files present at capture time (relative paths). On restore,
    /// untracked files *not* in this set are removed as experiment artifacts.
    pub untracked: Vec<String>,
}

fn git_out(root: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn untracked_files(root: &Path) -> Vec<String> {
    git_out(root, &["ls-files", "--others", "--exclude-standard"])
        .map(|s| s.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

/// Stage exactly `paths` and commit them — one logical commit out of a larger
/// changeset. Anything already staged from other paths is left alone, so call
/// this with a clean index (see [`unstage_all`]).
pub fn commit_paths(root: &Path, paths: &[String], message: &str) -> Result<(), String> {
    if paths.is_empty() {
        return Err("nothing to commit".into());
    }
    for p in paths {
        // `--` guards paths that look like flags; `-A` also stages deletions.
        run_git(root, &["add", "-A", "--", p])?;
    }
    run_git(root, &["commit", "-m", message])
}

/// Push `branch` and set its upstream (for a branch that doesn't exist remotely).
pub fn push_new_branch(root: &Path, branch: &str) -> Result<(), String> {
    run_git(root, &["push", "-u", "origin", branch])
}

/// Open a pull request with the GitHub CLI. Returns the PR URL.
pub fn create_pr(root: &Path, title: &str, body: &str) -> Result<String, String> {
    let out = Command::new("gh")
        .current_dir(root)
        .args(["pr", "create", "--title", title, "--body", body])
        .output()
        .map_err(|e| format!("gh not available: {e}"))?;
    if out.status.success() {
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        // gh prints the PR URL on the last line.
        Ok(s.lines().last().unwrap_or(&s).to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Largest untracked file inlined into a session diff before we treat it as
/// binary (keeps a review of generated assets from exploding).
const MAX_INLINE_UNTRACKED: usize = 256 * 1024;

/// Synthesize a “new file” unified diff for an untracked path, so it shows up in
/// a session diff without an index-mutating `git add -N`.
fn synth_new_file_diff(root: &Path, rel: &str) -> String {
    let Ok(bytes) = std::fs::read(root.join(rel)) else {
        return String::new();
    };
    let mut s =
        format!("diff --git a/{rel} b/{rel}\nnew file mode 100644\n--- /dev/null\n+++ b/{rel}\n");
    if bytes.contains(&0) || bytes.len() > MAX_INLINE_UNTRACKED {
        s.push_str(&format!("Binary files /dev/null and b/{rel} differ\n"));
        return s;
    }
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().collect();
    s.push_str(&format!("@@ -0,0 +1,{} @@\n", lines.len()));
    for l in lines {
        s.push('+');
        s.push_str(l);
        s.push('\n');
    }
    s
}

/// The unified diff of the working tree against `head` — i.e. everything an agent
/// session changed since its [`checkpoint`]. Tracked changes come from
/// `git diff <head>`; untracked files are synthesized as new-file diffs.
pub fn diff_since(root: &Path, head: &str) -> Result<String, String> {
    let mut out = git_out(root, &["diff", head])?;
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    for rel in untracked_files(root) {
        out.push_str(&synth_new_file_diff(root, &rel));
    }
    Ok(out)
}

/// Capture the current working tree so it can be restored later. Non-destructive
/// (uses `git stash create`, which does not touch the working tree).
pub fn checkpoint(root: &Path) -> Result<Checkpoint, String> {
    let head = git_out(root, &["rev-parse", "HEAD"])?;
    let stash = git_out(root, &["stash", "create"])
        .ok()
        .filter(|s| !s.is_empty());
    Ok(Checkpoint {
        head,
        stash,
        untracked: untracked_files(root),
    })
}

/// Restore the working tree to a [`checkpoint`]: reset tracked files to the
/// captured HEAD, remove untracked files created since (experiment artifacts),
/// then re-apply the user's original WIP stash. Destructive by design.
pub fn restore_checkpoint(root: &Path, cp: &Checkpoint) -> Result<(), String> {
    run_git(root, &["reset", "--hard", &cp.head])?;
    // Remove untracked files that appeared after the checkpoint (e.g. an agent's
    // new migration), but keep the user's pre-existing untracked files.
    let before: std::collections::HashSet<String> = cp.untracked.iter().cloned().collect();
    for path in untracked_files(root) {
        if !before.contains(&path) {
            let _ = std::fs::remove_file(root.join(&path));
        }
    }
    if let Some(stash) = &cp.stash {
        // Re-apply the captured WIP. `stash create` made a commit object; apply
        // it without touching the stash list.
        run_git(root, &["stash", "apply", "--index", stash])
            .or_else(|_| run_git(root, &["stash", "apply", stash]))?;
    }
    Ok(())
}

// ---- Turn snapshots ----------------------------------------------------------
//
// What one agent turn changed, and undoing just that: the working tree is
// captured as a tree object before the turn and again after it. Capturing
// writes blobs and a tree to the object store and nothing else — not the
// index, not HEAD, not the stash — so it is safe while the user works.

/// `git` with a different index file.
fn git_with_index(root: &Path, index: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_INDEX_FILE", index)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// `git` whose output is bytes, not text (a file's contents).
fn git_bytes(root: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// The working tree as it is now, as a tree object: tracked files with their
/// current contents and untracked ones that aren't ignored. Built in a copy
/// of the index, so the real one is never touched.
pub fn snapshot_tree(root: &Path) -> Result<String, String> {
    let index = git_out(root, &["rev-parse", "--git-path", "index"])?;
    let index = root.join(index);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!("e-turn-index-{}-{nanos}", std::process::id()));
    // A copy of the real index carries its stat cache: only files that changed
    // are hashed again.
    if index.is_file() {
        std::fs::copy(&index, &tmp).map_err(|e| e.to_string())?;
    }
    let tree = git_with_index(root, &tmp, &["add", "-A", "--", "."])
        .and_then(|_| git_with_index(root, &tmp, &["write-tree"]));
    let _ = std::fs::remove_file(&tmp);
    tree
}

/// One file a turn changed, with its line counts (both 0 for a binary file).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeChange {
    pub path: String,
    pub added: u32,
    pub removed: u32,
}

/// The files that differ between two snapshots.
pub fn tree_changes(root: &Path, before: &str, after: &str) -> Result<Vec<TreeChange>, String> {
    let out = git_bytes(
        root,
        &["diff", "--numstat", "-z", "--no-renames", before, after],
    )?;
    let text = String::from_utf8_lossy(&out);
    Ok(text
        .split('\0')
        .filter(|e| !e.is_empty())
        .filter_map(|e| {
            let mut parts = e.splitn(3, '\t');
            let added = parts.next()?.parse().unwrap_or(0);
            let removed = parts.next()?.parse().unwrap_or(0);
            let path = parts.next()?.to_string();
            Some(TreeChange {
                path,
                added,
                removed,
            })
        })
        .collect())
}

/// The unified diff between two snapshots.
pub fn tree_diff(root: &Path, before: &str, after: &str) -> Result<String, String> {
    let out = git_bytes(root, &["diff", "--no-renames", before, after])?;
    Ok(String::from_utf8_lossy(&out).into_owned())
}

/// What undoing a turn did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UndoReport {
    /// Put back as they were before the turn (or removed, if it created them).
    pub restored: Vec<String>,
    /// Left alone: changed again since the turn, so undoing would lose that.
    pub skipped: Vec<String>,
}

/// `(mode, blob)` of `path` in `tree`, or `None` when it isn't there.
fn entry_in(root: &Path, tree: &str, path: &str) -> Option<(String, String)> {
    let out = git_out(root, &["ls-tree", tree, "--", path]).ok()?;
    let (meta, _) = out.split_once('\t')?;
    let mut it = meta.split_whitespace();
    let mode = it.next()?.to_string();
    let _kind = it.next()?;
    let sha = it.next()?.to_string();
    Some((mode, sha))
}

/// Put `paths` back as they were in `before` — but only those still exactly
/// as `after` left them. A file changed again since (by the user, or a later
/// turn) is skipped, never overwritten.
pub fn undo_tree_change(
    root: &Path,
    before: &str,
    after: &str,
    paths: &[String],
) -> Result<UndoReport, String> {
    let mut report = UndoReport::default();
    for path in paths {
        let file = root.join(path);
        let now = if file.is_file() {
            Some(git_out(root, &["hash-object", "--", path])?)
        } else {
            None
        };
        let then = entry_in(root, after, path).map(|(_, sha)| sha);
        if now != then {
            report.skipped.push(path.clone());
            continue;
        }
        match entry_in(root, before, path) {
            Some((mode, sha)) => {
                let bytes = git_bytes(root, &["cat-file", "blob", &sha])?;
                if let Some(dir) = file.parent() {
                    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
                }
                std::fs::write(&file, bytes).map_err(|e| e.to_string())?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let bits = if mode == "100755" { 0o755 } else { 0o644 };
                    let _ = std::fs::set_permissions(&file, std::fs::Permissions::from_mode(bits));
                }
            }
            // The turn created it: undoing removes it.
            None => {
                let _ = std::fs::remove_file(&file);
            }
        }
        report.restored.push(path.clone());
    }
    Ok(report)
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

    use super::{
        checkpoint, restore_checkpoint, snapshot_tree, tree_changes, tree_diff, undo_tree_change,
    };
    use std::path::{Path, PathBuf};
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        assert!(ok, "git {args:?} failed");
    }

    fn init_repo(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("e-ckpt-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q"]);
        git(&dir, &["config", "user.email", "t@example.com"]);
        git(&dir, &["config", "user.name", "Test"]);
        git(&dir, &["config", "commit.gpgsign", "false"]);
        dir
    }

    #[test]
    fn checkpoint_reverts_agent_changes() {
        let dir = init_repo("revert");
        std::fs::write(dir.join("a.txt"), "one").unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "init"]);

        let cp = checkpoint(&dir).unwrap();

        // Simulate an experiment: modify a tracked file + add a new one.
        std::fs::write(dir.join("a.txt"), "changed-by-agent").unwrap();
        std::fs::write(dir.join("migration.txt"), "CREATE INDEX").unwrap();

        restore_checkpoint(&dir, &cp).unwrap();

        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "one");
        assert!(
            !dir.join("migration.txt").exists(),
            "agent-created untracked file should be removed"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn diff_since_covers_tracked_and_untracked() {
        use super::diff_since;
        let dir = init_repo("diffsince");
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "init"]);

        let cp = checkpoint(&dir).unwrap();

        // A tracked edit and a brand-new (untracked) file, like an agent session.
        std::fs::write(dir.join("a.txt"), "two\n").unwrap();
        std::fs::create_dir_all(dir.join("database/migrations")).unwrap();
        std::fs::write(
            dir.join("database/migrations/2026_add_x.php"),
            "<?php\n// new\n",
        )
        .unwrap();

        let d = diff_since(&dir, &cp.head).unwrap();

        // Tracked modification.
        assert!(d.contains("diff --git a/a.txt b/a.txt"), "{d}");
        assert!(d.contains("-one") && d.contains("+two"), "{d}");
        // Untracked file synthesized as a new-file diff.
        assert!(
            d.contains("diff --git a/database/migrations/2026_add_x.php"),
            "{d}"
        );
        assert!(d.contains("new file mode"), "{d}");
        assert!(d.contains("+// new"), "{d}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn checkpoint_preserves_preexisting_wip() {
        let dir = init_repo("wip");
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "init"]);

        // The user already has uncommitted work when the experiment starts.
        std::fs::write(dir.join("a.txt"), "user-wip\n").unwrap();
        let cp = checkpoint(&dir).unwrap();
        assert!(
            cp.stash.is_some(),
            "WIP should be captured as a stash object"
        );

        // Agent changes on top, plus a new file.
        std::fs::write(dir.join("a.txt"), "agent\n").unwrap();
        std::fs::write(dir.join("new.txt"), "x").unwrap();

        restore_checkpoint(&dir, &cp).unwrap();

        assert_eq!(
            std::fs::read_to_string(dir.join("a.txt")).unwrap(),
            "user-wip\n",
            "the user's original WIP should be restored"
        );
        assert!(!dir.join("new.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_turn_is_captured_without_touching_the_index_and_undone_file_by_file() {
        let dir = init_repo("turn");
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        std::fs::write(dir.join(".gitignore"), "ignored.log\n").unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-qm", "init"]);
        // Work in progress the user has, staged and not.
        std::fs::write(dir.join("a.txt"), "one\nuser wip\n").unwrap();
        std::fs::write(dir.join("notes.txt"), "mine\n").unwrap();
        git(&dir, &["add", "notes.txt"]);
        let index_before = std::fs::read(dir.join(".git/index")).unwrap();

        let before = snapshot_tree(&dir).unwrap();
        // The turn: edits a.txt, creates b.txt, deletes notes.txt, writes an
        // ignored log.
        std::fs::write(dir.join("a.txt"), "one\nuser wip\nagent\n").unwrap();
        std::fs::write(dir.join("b.txt"), "new\n").unwrap();
        std::fs::remove_file(dir.join("notes.txt")).unwrap();
        std::fs::write(dir.join("ignored.log"), "noise").unwrap();
        let after = snapshot_tree(&dir).unwrap();

        assert_eq!(std::fs::read(dir.join(".git/index")).unwrap(), index_before);
        let changes = tree_changes(&dir, &before, &after).unwrap();
        let paths: Vec<&str> = changes.iter().map(|c| c.path.as_str()).collect();
        assert_eq!(paths, ["a.txt", "b.txt", "notes.txt"]);
        assert_eq!((changes[0].added, changes[0].removed), (1, 0));
        assert!(tree_diff(&dir, &before, &after).unwrap().contains("+agent"));

        // The user changes b.txt again after the turn: undo leaves it alone.
        std::fs::write(dir.join("b.txt"), "new\nedited by me\n").unwrap();
        let all: Vec<String> = changes.iter().map(|c| c.path.clone()).collect();
        let report = undo_tree_change(&dir, &before, &after, &all).unwrap();
        assert_eq!(report.restored, ["a.txt", "notes.txt"]);
        assert_eq!(report.skipped, ["b.txt"]);
        assert_eq!(
            std::fs::read_to_string(dir.join("a.txt")).unwrap(),
            "one\nuser wip\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("notes.txt")).unwrap(),
            "mine\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("b.txt")).unwrap(),
            "new\nedited by me\n"
        );
        // Undo of a created file that is still as the turn left it removes it.
        std::fs::write(dir.join("b.txt"), "new\n").unwrap();
        let report = undo_tree_change(&dir, &before, &after, &["b.txt".to_string()]).unwrap();
        assert_eq!(report.restored, ["b.txt"]);
        assert!(!dir.join("b.txt").exists());
        assert_eq!(std::fs::read(dir.join(".git/index")).unwrap(), index_before);
        std::fs::remove_dir_all(&dir).ok();
    }
}
