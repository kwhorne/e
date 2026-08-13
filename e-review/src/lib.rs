//! Session changeset review — the core behind reviewing what an agent changed.
//!
//! After an agent session touches (say) 50 files, you don't want to push and
//! review on GitHub: you want to understand, verify and be able to undo it
//! locally. This crate turns a unified diff (the working tree measured against a
//! session checkpoint) into a **risk-ranked, reviewable changeset**:
//!
//! - [`parse_unified_diff`] parses `git diff` output into [`FileChange`]s+[`Hunk`]s.
//! - [`classify_risk`] ranks a path so migrations/config/auth surface before tests
//!   and lockfiles.
//! - [`Changeset`] tracks review progress (what you've signed off on).
//!
//! Everything here is pure and unit-tested; the git plumbing lives in
//! `e_core::git` and the panel in the app.

pub mod commits;
pub mod flags;
pub mod routes;
pub mod ship;

use std::collections::HashMap;

/// How a file changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
}

/// How much attention a change deserves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Risk {
    /// Low review value (lockfiles, docs, tests).
    Low,
    /// Ordinary source code.
    Medium,
    /// Schema, config, routing, auth, dependencies, CI — review first.
    High,
}

/// One hunk of a file's diff.
#[derive(Debug, Clone, PartialEq)]
pub struct Hunk {
    /// 1-based start line in the old file.
    pub old_start: usize,
    /// 1-based start line in the new file.
    pub new_start: usize,
    /// Raw unified-diff body lines, including the leading `+`/`-`/` `.
    pub lines: Vec<String>,
    pub added: usize,
    pub removed: usize,
}

/// One changed file in the session.
#[derive(Debug, Clone, PartialEq)]
pub struct FileChange {
    /// Path relative to the repository root (the new path for renames).
    pub path: String,
    /// The previous path, for renames.
    pub old_path: Option<String>,
    pub kind: ChangeKind,
    pub hunks: Vec<Hunk>,
    pub added: usize,
    pub removed: usize,
    /// True when git reported a binary diff (no reviewable text).
    pub binary: bool,
    pub risk: Risk,
    /// Why it got that risk, e.g. `"migration"` — shown as a badge.
    pub risk_reason: &'static str,
    /// Whether you've signed this file off.
    pub reviewed: bool,
}

impl FileChange {
    fn new(path: String) -> Self {
        let (risk, risk_reason) = classify_risk(&path);
        FileChange {
            path,
            old_path: None,
            kind: ChangeKind::Modified,
            hunks: Vec::new(),
            added: 0,
            removed: 0,
            binary: false,
            risk,
            risk_reason,
            reviewed: false,
        }
    }

    /// Re-classify after the path is known to be final (renames rewrite it).
    fn finish(&mut self) {
        let (risk, reason) = classify_risk(&self.path);
        self.risk = risk;
        self.risk_reason = reason;
        self.added = self.hunks.iter().map(|h| h.added).sum();
        self.removed = self.hunks.iter().map(|h| h.removed).sum();
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn has_segment(path: &str, seg: &str) -> bool {
    path.split('/').any(|s| s == seg)
}

/// Rank a path by how much review attention it deserves. Ordered rules, so the
/// first match wins — lockfiles are checked before dependency manifests, and
/// high-risk locations before the generic source/test buckets.
pub fn classify_risk(path: &str) -> (Risk, &'static str) {
    let name = file_name(path);

    // Machine-generated, huge, low signal.
    const LOCKFILES: &[&str] = &[
        "composer.lock",
        "package-lock.json",
        "yarn.lock",
        "pnpm-lock.yaml",
        "Cargo.lock",
        "bun.lockb",
    ];
    if LOCKFILES.contains(&name) {
        return (Risk::Low, "lockfile");
    }

    // High risk: things that change behaviour or deployment outside the code path.
    if path.contains("database/migrations/") || has_segment(path, "migrations") {
        return (Risk::High, "migration");
    }
    if name == ".env" || name.starts_with(".env.") {
        return (Risk::High, "environment");
    }
    if path.starts_with("config/") || has_segment(path, "config") {
        return (Risk::High, "config");
    }
    if path.starts_with("routes/") || has_segment(path, "routes") {
        return (Risk::High, "routes");
    }
    if has_segment(path, "Middleware")
        || has_segment(path, "Policies")
        || has_segment(path, "auth")
        || has_segment(path, "Auth")
        || name.contains("Policy")
        || name.contains("Middleware")
    {
        return (Risk::High, "auth");
    }
    if path.starts_with(".github/workflows/") {
        return (Risk::High, "CI");
    }
    const MANIFESTS: &[&str] = &[
        "composer.json",
        "package.json",
        "Cargo.toml",
        "Dockerfile",
        "docker-compose.yml",
        "docker-compose.yaml",
    ];
    if MANIFESTS.contains(&name) {
        return (Risk::High, "dependencies");
    }

    // Low risk: tests, docs, snapshots.
    if path.starts_with("tests/")
        || path.starts_with("test/")
        || path.starts_with("spec/")
        || has_segment(path, "__snapshots__")
        || name.ends_with("Test.php")
        || name.ends_with("_test.rs")
        || name.ends_with("_test.go")
        || name.contains(".test.")
        || name.contains(".spec.")
    {
        return (Risk::Low, "test");
    }
    if name.ends_with(".md") || name.ends_with(".mdx") || path.starts_with("docs/") {
        return (Risk::Low, "docs");
    }

    (Risk::Medium, "source")
}

fn split_diff_git_paths(rest: &str) -> Option<String> {
    // `a/foo/bar.rs b/foo/bar.rs` — take the `b/` side (the new path).
    // Note: git quotes paths containing spaces (`"a/x y"`); those fall back to None.
    let idx = rest.find(" b/")?;
    Some(rest[idx + 3..].to_string())
}

fn parse_hunk_header(line: &str) -> (usize, usize) {
    let mut it = line.split_whitespace();
    it.next(); // "@@"
    let old = it
        .next()
        .and_then(|t| t.strip_prefix('-'))
        .and_then(|t| t.split(',').next())
        .and_then(|n| n.parse().ok())
        .unwrap_or(1);
    let new = it
        .next()
        .and_then(|t| t.strip_prefix('+'))
        .and_then(|t| t.split(',').next())
        .and_then(|n| n.parse().ok())
        .unwrap_or(1);
    (old, new)
}

/// Parse `git diff` (unified) output into per-file changes.
pub fn parse_unified_diff(text: &str) -> Vec<FileChange> {
    let mut files: Vec<FileChange> = Vec::new();
    let mut cur: Option<FileChange> = None;
    let mut hunk: Option<Hunk> = None;

    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            if let Some(mut f) = cur.take() {
                if let Some(h) = hunk.take() {
                    f.hunks.push(h);
                }
                f.finish();
                files.push(f);
            }
            hunk = None;
            cur = split_diff_git_paths(rest).map(FileChange::new);
            continue;
        }
        let Some(f) = cur.as_mut() else { continue };

        if line.starts_with("@@") {
            if let Some(h) = hunk.take() {
                f.hunks.push(h);
            }
            let (old_start, new_start) = parse_hunk_header(line);
            hunk = Some(Hunk {
                old_start,
                new_start,
                lines: Vec::new(),
                added: 0,
                removed: 0,
            });
            continue;
        }

        if let Some(h) = hunk.as_mut() {
            // Inside a hunk body.
            if line.starts_with('\\') {
                continue; // "\ No newline at end of file"
            }
            match line.as_bytes().first() {
                Some(b'+') => {
                    h.added += 1;
                    h.lines.push(line.to_string());
                }
                Some(b'-') => {
                    h.removed += 1;
                    h.lines.push(line.to_string());
                }
                Some(b' ') | None => h.lines.push(line.to_string()),
                _ => {
                    // Not a diff body line — the hunk ended.
                    if let Some(h) = hunk.take() {
                        f.hunks.push(h);
                    }
                }
            }
            continue;
        }

        // File header lines (before the first hunk).
        if line.starts_with("new file mode") {
            f.kind = ChangeKind::Added;
        } else if line.starts_with("deleted file mode") {
            f.kind = ChangeKind::Deleted;
        } else if let Some(p) = line.strip_prefix("rename from ") {
            f.old_path = Some(p.to_string());
            f.kind = ChangeKind::Renamed;
        } else if let Some(p) = line.strip_prefix("rename to ") {
            f.path = p.to_string();
            f.kind = ChangeKind::Renamed;
        } else if line.starts_with("Binary files") || line.starts_with("GIT binary patch") {
            f.binary = true;
        }
    }

    if let Some(mut f) = cur.take() {
        if let Some(h) = hunk.take() {
            f.hunks.push(h);
        }
        f.finish();
        files.push(f);
    }
    files
}

/// The whole session's changes, ranked for review.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Changeset {
    pub files: Vec<FileChange>,
}

/// Build a review-ordered changeset from a unified diff: highest risk first,
/// then alphabetical (stable and predictable between runs).
pub fn changeset_from_diff(text: &str) -> Changeset {
    let mut files = parse_unified_diff(text);
    files.sort_by(|a, b| b.risk.cmp(&a.risk).then(a.path.cmp(&b.path)));
    Changeset { files }
}

impl Changeset {
    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn total_added(&self) -> usize {
        self.files.iter().map(|f| f.added).sum()
    }

    pub fn total_removed(&self) -> usize {
        self.files.iter().map(|f| f.removed).sum()
    }

    pub fn high_risk_count(&self) -> usize {
        self.files.iter().filter(|f| f.risk == Risk::High).count()
    }

    /// `(reviewed, total)` for the progress indicator.
    pub fn progress(&self) -> (usize, usize) {
        (
            self.files.iter().filter(|f| f.reviewed).count(),
            self.files.len(),
        )
    }

    pub fn all_reviewed(&self) -> bool {
        !self.files.is_empty() && self.files.iter().all(|f| f.reviewed)
    }

    pub fn get(&self, path: &str) -> Option<&FileChange> {
        self.files.iter().find(|f| f.path == path)
    }

    pub fn mark_reviewed(&mut self, path: &str, reviewed: bool) {
        if let Some(f) = self.files.iter_mut().find(|f| f.path == path) {
            f.reviewed = reviewed;
        }
    }

    /// Drop a file from the changeset (e.g. after reverting it).
    pub fn remove(&mut self, path: &str) {
        self.files.retain(|f| f.path != path);
    }

    /// The next file needing review after `after` (wrapping), for keyboard nav.
    pub fn next_unreviewed(&self, after: Option<&str>) -> Option<&FileChange> {
        let start = match after {
            Some(p) => self
                .files
                .iter()
                .position(|f| f.path == p)
                .map_or(0, |i| i + 1),
            None => 0,
        };
        self.files[start..]
            .iter()
            .chain(self.files[..start].iter())
            .find(|f| !f.reviewed)
    }

    /// One-line headline for the panel.
    pub fn summary(&self) -> String {
        let high = self.high_risk_count();
        let mut s = format!(
            "{} file{} · +{} −{}",
            self.len(),
            if self.len() == 1 { "" } else { "s" },
            self.total_added(),
            self.total_removed()
        );
        if high > 0 {
            s.push_str(&format!(" · {high} high-risk"));
        }
        s
    }

    /// Carry review ticks over to a freshly-parsed changeset (the diff is
    /// re-read after a revert, and we don't want to lose sign-offs).
    pub fn carry_reviewed_from(&mut self, old: &Changeset) {
        let marks: HashMap<&str, bool> = old
            .files
            .iter()
            .map(|f| (f.path.as_str(), f.reviewed))
            .collect();
        for f in &mut self.files {
            if let Some(&r) = marks.get(f.path.as_str()) {
                f.reviewed = r;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIFF: &str = "\
diff --git a/app/Models/User.php b/app/Models/User.php
index 1111111..2222222 100644
--- a/app/Models/User.php
+++ b/app/Models/User.php
@@ -10,7 +10,8 @@ class User extends Model
     protected $fillable = [
         'name',
-        'email',
+        'email_address',
+        'locale',
     ];
diff --git a/database/migrations/2026_01_01_add_locale.php b/database/migrations/2026_01_01_add_locale.php
new file mode 100644
index 0000000..3333333
--- /dev/null
+++ b/database/migrations/2026_01_01_add_locale.php
@@ -0,0 +1,3 @@
+<?php
+// migration
+return new class extends Migration {};
diff --git a/tests/Feature/UserTest.php b/tests/Feature/UserTest.php
index 4444444..5555555 100644
--- a/tests/Feature/UserTest.php
+++ b/tests/Feature/UserTest.php
@@ -5,3 +5,3 @@
-    it('has email', fn () => expect(1)->toBe(1));
+    it('has email_address', fn () => expect(1)->toBe(1));
";

    #[test]
    fn parses_files_hunks_and_counts() {
        let files = parse_unified_diff(DIFF);
        assert_eq!(files.len(), 3);

        let user = files.iter().find(|f| f.path.ends_with("User.php")).unwrap();
        assert_eq!(user.kind, ChangeKind::Modified);
        assert_eq!(user.hunks.len(), 1);
        assert_eq!(user.added, 2);
        assert_eq!(user.removed, 1);
        assert_eq!(user.hunks[0].old_start, 10);
        assert_eq!(user.hunks[0].new_start, 10);

        let mig = files
            .iter()
            .find(|f| f.path.contains("migrations"))
            .unwrap();
        assert_eq!(mig.kind, ChangeKind::Added);
        assert_eq!(mig.added, 3);
        assert_eq!(mig.removed, 0);
    }

    #[test]
    fn ranks_migration_first_and_test_last() {
        let cs = changeset_from_diff(DIFF);
        assert_eq!(cs.files[0].risk, Risk::High);
        assert!(cs.files[0].path.contains("migrations"));
        assert_eq!(cs.files[0].risk_reason, "migration");
        assert_eq!(cs.files.last().unwrap().risk, Risk::Low);
        assert_eq!(cs.files.last().unwrap().risk_reason, "test");
    }

    #[test]
    fn risk_rules() {
        assert_eq!(classify_risk("composer.lock").0, Risk::Low);
        assert_eq!(classify_risk("composer.lock").1, "lockfile");
        assert_eq!(classify_risk("composer.json").1, "dependencies");
        assert_eq!(classify_risk(".env.production").1, "environment");
        assert_eq!(classify_risk("config/app.php").1, "config");
        assert_eq!(classify_risk("routes/web.php").1, "routes");
        assert_eq!(classify_risk("app/Http/Middleware/Guard.php").1, "auth");
        assert_eq!(classify_risk("app/Policies/PostPolicy.php").1, "auth");
        assert_eq!(classify_risk(".github/workflows/ci.yml").1, "CI");
        assert_eq!(classify_risk("docs/readme.md").1, "docs");
        assert_eq!(classify_risk("README.md").1, "docs");
        assert_eq!(classify_risk("src/main.rs").1, "source");
        assert_eq!(classify_risk("resources/js/app.test.ts").1, "test");
    }

    #[test]
    fn handles_deleted_renamed_and_binary() {
        let d = "\
diff --git a/old/Name.php b/new/Name.php
similarity index 90%
rename from old/Name.php
rename to new/Name.php
diff --git a/gone.php b/gone.php
deleted file mode 100644
--- a/gone.php
+++ /dev/null
@@ -1,2 +0,0 @@
-<?php
-echo 1;
diff --git a/public/logo.png b/public/logo.png
index aaa..bbb 100644
Binary files a/public/logo.png and b/public/logo.png differ
";
        let files = parse_unified_diff(d);
        assert_eq!(files.len(), 3);
        let ren = files.iter().find(|f| f.path == "new/Name.php").unwrap();
        assert_eq!(ren.kind, ChangeKind::Renamed);
        assert_eq!(ren.old_path.as_deref(), Some("old/Name.php"));
        let del = files.iter().find(|f| f.path == "gone.php").unwrap();
        assert_eq!(del.kind, ChangeKind::Deleted);
        assert_eq!(del.removed, 2);
        let bin = files.iter().find(|f| f.path.ends_with(".png")).unwrap();
        assert!(bin.binary);
        assert!(bin.hunks.is_empty());
    }

    #[test]
    fn progress_and_navigation() {
        let mut cs = changeset_from_diff(DIFF);
        assert_eq!(cs.progress(), (0, 3));
        assert!(!cs.all_reviewed());

        let first = cs.next_unreviewed(None).unwrap().path.clone();
        cs.mark_reviewed(&first, true);
        assert_eq!(cs.progress(), (1, 3));

        // Navigation continues after the marked file.
        let second = cs.next_unreviewed(Some(&first)).unwrap().path.clone();
        assert_ne!(second, first);
        cs.mark_reviewed(&second, true);
        let third = cs.next_unreviewed(Some(&second)).unwrap().path.clone();
        cs.mark_reviewed(&third, true);
        assert!(cs.all_reviewed());
        assert!(cs.next_unreviewed(None).is_none());
    }

    #[test]
    fn navigation_wraps_around() {
        let mut cs = changeset_from_diff(DIFF);
        let last = cs.files.last().unwrap().path.clone();
        // Mark everything except the first file.
        for p in cs
            .files
            .iter()
            .map(|f| f.path.clone())
            .skip(1)
            .collect::<Vec<_>>()
        {
            cs.mark_reviewed(&p, true);
        }
        let next = cs.next_unreviewed(Some(&last)).unwrap();
        assert_eq!(next.path, cs.files[0].path);
    }

    #[test]
    fn summary_counts_high_risk() {
        let cs = changeset_from_diff(DIFF);
        let s = cs.summary();
        assert!(s.starts_with("3 files · +"), "{s}");
        assert!(s.contains("1 high-risk"), "{s}");
    }

    #[test]
    fn carries_review_marks_across_reparse() {
        let mut cs = changeset_from_diff(DIFF);
        let p = cs.files[0].path.clone();
        cs.mark_reviewed(&p, true);

        let mut fresh = changeset_from_diff(DIFF);
        fresh.carry_reviewed_from(&cs);
        assert!(fresh.get(&p).unwrap().reviewed);
        assert_eq!(fresh.progress(), (1, 3));
    }

    #[test]
    fn remove_drops_file() {
        let mut cs = changeset_from_diff(DIFF);
        let p = cs.files[0].path.clone();
        cs.remove(&p);
        assert_eq!(cs.len(), 2);
        assert!(cs.get(&p).is_none());
    }

    #[test]
    fn empty_diff_is_empty_changeset() {
        let cs = changeset_from_diff("");
        assert!(cs.is_empty());
        assert_eq!(cs.progress(), (0, 0));
        assert!(!cs.all_reviewed());
    }
}
