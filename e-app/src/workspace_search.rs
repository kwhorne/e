//! Workspace-wide search and replace (`⌘⇧F` / Replace All).
//!
//! Search and replace go through the **same walker** and the **same matcher**,
//! so the hit list you see is exactly the set that Replace All rewrites.
//!
//! They used to disagree in two ways that could lose work:
//!
//! - The walker skipped only dot-entries, `target` and `node_modules`, so a
//!   Laravel `vendor/`, `storage/` or `public/build/` was fair game — Replace
//!   All would rewrite Composer dependencies. Ignore rules are now honoured.
//! - Search matched case-insensitively (`to_lowercase().find()`) while replace
//!   matched case-sensitively (`str::contains`), and search reported only the
//!   *first* hit per line while replace rewrote *every* occurrence. So the
//!   count you were shown and the edit you got were different things.

use std::path::{Path, PathBuf};

use ignore::WalkBuilder;
use regex::{Regex, RegexBuilder};

/// Files larger than this are skipped by both search and replace.
const MAX_FILE_BYTES: u64 = 2_000_000;

/// Directories skipped even when a project has no ignore rules at all.
///
/// Everything else is left to `.gitignore`, which is what keeps this honest in
/// both directions: a project's *tracked* `vendor/` (like this repo's vendored
/// Floem) stays searchable, while a Laravel project's *ignored* `vendor/` does
/// not — without either being hardcoded here.
const ALWAYS_SKIP: &[&str] = &[".git", "target", "node_modules"];

/// How a query is interpreted. Shared by search and replace so they cannot
/// drift apart again.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SearchOpts {
    pub case_sensitive: bool,
}

/// One match, located for the picker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub path: PathBuf,
    pub line: u32,
    /// Byte offset of the match *within its line* — the unit
    /// `Editor::offset_of_line_col` expects.
    pub col: u32,
    /// The matching line, trimmed for display.
    pub text: String,
}

/// What a Replace All would do, computed before anything is written.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReplacePlan {
    /// Each file that would change, with how many matches it holds.
    pub files: Vec<(PathBuf, usize)>,
    pub total_matches: usize,
}

impl ReplacePlan {
    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

/// The result of applying a [`ReplacePlan`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReplaceOutcome {
    pub files_changed: usize,
    pub matches_replaced: usize,
    /// Files that matched but could not be written, with the reason.
    pub failures: Vec<(PathBuf, String)>,
}

/// Build the matcher used by *both* search and replace.
///
/// The query is a literal — [`regex::escape`] means a user searching for
/// `$user->name()` gets what they typed, not a regex.
pub fn matcher(query: &str, opts: SearchOpts) -> Option<Regex> {
    if query.is_empty() {
        return None;
    }
    RegexBuilder::new(&regex::escape(query))
        .case_insensitive(!opts.case_sensitive)
        .build()
        .ok()
}

/// Byte ranges of every match in `content`, in ascending order.
pub fn match_ranges(content: &str, re: &Regex) -> Vec<(usize, usize)> {
    re.find_iter(content)
        .map(|m| (m.start(), m.end()))
        .collect()
}

/// Replace `ranges` (ascending, non-overlapping) in `content` with `replacement`.
pub fn splice(content: &str, ranges: &[(usize, usize)], replacement: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut last = 0;
    for &(start, end) in ranges {
        out.push_str(&content[last..start]);
        out.push_str(replacement);
        last = end;
    }
    out.push_str(&content[last..]);
    out
}

/// The newest search asked for (the picker's generation): an older one still
/// walking stops.
pub static LATEST_SEARCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// How long typing must pause before a search runs.
pub const SEARCH_PAUSE: std::time::Duration = std::time::Duration::from_millis(120);

/// The text of files open in the editor, by canonical path: a search reads
/// it instead of the disk, so it sees unsaved edits, and a replace leaves the
/// file to the editor.
pub type OpenTexts = std::collections::HashMap<PathBuf, String>;

/// One text file under the roots.
struct TextFile<'a> {
    path: &'a Path,
    content: &'a str,
    /// What to write it back as (a label from `e_core::buffer::decode`).
    encoding: &'a str,
    /// Open in the editor: `content` is the editor's text.
    open: bool,
}

/// Walk the text files under `roots`, honouring `.gitignore` (and the global
/// and `.git/info/exclude` rules), calling `visit` with each file's text:
/// decoded from what it is on disk (UTF-8, with or without a BOM, UTF-16,
/// or a legacy single-byte encoding), or the editor's when it is open.
///
/// `visit` returns `false` to stop the walk early.
fn walk_text_files(roots: &[PathBuf], open: &OpenTexts, mut visit: impl FnMut(TextFile) -> bool) {
    let Some((first, rest)) = roots.split_first() else {
        return;
    };
    let mut builder = WalkBuilder::new(first);
    for root in rest {
        builder.add(root);
    }
    builder
        .hidden(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .ignore(true)
        // Honour `.gitignore` even in a directory that isn't a git repo.
        .require_git(false)
        .parents(true)
        .max_filesize(Some(MAX_FILE_BYTES))
        .filter_entry(|entry| {
            entry
                .file_name()
                .to_str()
                .map(|name| !ALWAYS_SKIP.contains(&name))
                .unwrap_or(true)
        });

    for entry in builder.build() {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let path = entry.path();
        if !open.is_empty() {
            let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
            if let Some(text) = open.get(&key) {
                let file = TextFile {
                    path,
                    content: text,
                    encoding: "",
                    open: true,
                };
                if !visit(file) {
                    return;
                }
                continue;
            }
        }
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        if e_core::buffer::looks_binary(&bytes[..bytes.len().min(8192)]) {
            continue;
        }
        let (content, encoding) = e_core::buffer::decode(&bytes);
        if content.contains('\0') {
            continue;
        }
        let file = TextFile {
            path,
            content: &content,
            encoding: &encoding,
            open: false,
        };
        if !visit(file) {
            return;
        }
    }
}

/// Byte offset at which each line of `content` starts.
fn line_starts(content: &str) -> Vec<usize> {
    let mut starts = vec![0];
    starts.extend(
        content
            .match_indices('\n')
            .map(|(i, _)| i + 1)
            .filter(|&i| i < content.len()),
    );
    starts
}

/// Collect every match under `roots`, up to `max` hits, reading open files'
/// text from `open`. Stops early, with what it has, once `cancelled` says so.
pub fn search(
    roots: &[PathBuf],
    query: &str,
    opts: SearchOpts,
    max: usize,
    open: &OpenTexts,
    cancelled: &dyn Fn() -> bool,
) -> Vec<Hit> {
    let Some(re) = matcher(query, opts) else {
        return Vec::new();
    };
    let mut hits = Vec::new();
    walk_text_files(roots, open, |TextFile { path, content, .. }| {
        if cancelled() {
            return false;
        }
        let ranges = match_ranges(content, &re);
        if ranges.is_empty() {
            return true;
        }
        let starts = line_starts(content);
        for (start, _) in ranges {
            let line = starts.partition_point(|&s| s <= start).saturating_sub(1);
            let line_start = starts[line];
            let line_text = content[line_start..]
                .split('\n')
                .next()
                .unwrap_or("")
                .trim_end_matches('\r');
            hits.push(Hit {
                path: path.to_path_buf(),
                line: line as u32,
                col: (start - line_start) as u32,
                text: line_text.trim_start().chars().take(120).collect(),
            });
            if hits.len() >= max {
                return false;
            }
        }
        true
    });
    hits
}

/// Work out what a Replace All would rewrite — **without writing anything**.
/// Open files are counted in the editor's text.
pub fn plan_replace(
    roots: &[PathBuf],
    query: &str,
    opts: SearchOpts,
    open: &OpenTexts,
) -> ReplacePlan {
    let Some(re) = matcher(query, opts) else {
        return ReplacePlan::default();
    };
    let mut plan = ReplacePlan::default();
    walk_text_files(roots, open, |TextFile { path, content, .. }| {
        let count = re.find_iter(content).count();
        if count > 0 {
            plan.files.push((path.to_path_buf(), count));
            plan.total_matches += count;
        }
        true
    });
    plan
}

/// Apply a Replace All. Files are re-matched as they are read, so a file that
/// changed since the plan was built is handled as it is now, not as it was.
///
/// Files open in the editor (`open`) are left alone: the editor replaces in
/// its own text, unsaved edits and all. The rest are written back in the
/// encoding they were read in, atomically; one whose encoding can't hold the
/// replacement is left as it was and reported.
pub fn apply_replace(
    roots: &[PathBuf],
    query: &str,
    opts: SearchOpts,
    replacement: &str,
    open: &OpenTexts,
) -> ReplaceOutcome {
    let mut outcome = ReplaceOutcome::default();
    let Some(re) = matcher(query, opts) else {
        return outcome;
    };
    // Written as the walk goes rather than buffered to the end: this only ever
    // rewrites the *contents* of files the walker has already handed us, so it
    // cannot make the walker visit its own output — and buffering every new
    // file body would put the whole changeset in memory at once.
    walk_text_files(roots, open, |file| {
        if file.open {
            return true;
        }
        let ranges = match_ranges(file.content, &re);
        if ranges.is_empty() {
            return true;
        }
        let updated = splice(file.content, &ranges, replacement);
        if updated == file.content {
            return true;
        }
        match e_core::buffer::write_with_encoding(file.path, &updated, file.encoding) {
            Ok(()) => {
                outcome.files_changed += 1;
                outcome.matches_replaced += ranges.len();
            }
            Err(e) => outcome
                .failures
                .push((file.path.to_path_buf(), format!("{e:#}"))),
        }
        true
    });
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn none() -> OpenTexts {
        OpenTexts::new()
    }

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// A throwaway directory holding `files` (path → contents).
    fn scratch(files: &[(&str, &str)]) -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("e-ws-search-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (rel, body) in files {
            let path = dir.join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&path, body).unwrap();
        }
        dir
    }

    fn read(dir: &Path, rel: &str) -> String {
        std::fs::read_to_string(dir.join(rel)).unwrap()
    }

    fn insensitive() -> SearchOpts {
        SearchOpts {
            case_sensitive: false,
        }
    }

    fn sensitive() -> SearchOpts {
        SearchOpts {
            case_sensitive: true,
        }
    }

    // ── the two halves agree ────────────────────────────────────────────

    #[test]
    fn search_and_replace_agree_on_case() {
        let dir = scratch(&[("a.php", "user User USER\n")]);
        let roots = vec![dir.clone()];

        // Case-insensitive: search finds three, replace rewrites three.
        let hits = search(&roots, "user", insensitive(), 100, &none(), &|| false);
        let plan = plan_replace(&roots, "user", insensitive(), &none());
        assert_eq!(hits.len(), 3);
        assert_eq!(plan.total_matches, hits.len());

        // Case-sensitive: both narrow to one.
        let hits = search(&roots, "user", sensitive(), 100, &none(), &|| false);
        let plan = plan_replace(&roots, "user", sensitive(), &none());
        assert_eq!(hits.len(), 1);
        assert_eq!(plan.total_matches, hits.len());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn search_reports_every_occurrence_on_a_line() {
        // The old search stopped at the first hit per line, so the count
        // undersold what Replace All would do.
        let dir = scratch(&[("a.txt", "foo foo foo\n")]);
        let roots = vec![dir.clone()];
        let hits = search(&roots, "foo", insensitive(), 100, &none(), &|| false);
        assert_eq!(hits.len(), 3);
        assert_eq!(hits.iter().map(|h| h.col).collect::<Vec<_>>(), [0, 4, 8]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn plan_matches_what_apply_does() {
        let dir = scratch(&[("a.txt", "x x\n"), ("b.txt", "x\n"), ("c.txt", "none\n")]);
        let roots = vec![dir.clone()];
        let plan = plan_replace(&roots, "x", insensitive(), &none());
        let out = apply_replace(&roots, "x", insensitive(), "y", &none());
        assert_eq!(plan.file_count(), out.files_changed);
        assert_eq!(plan.total_matches, out.matches_replaced);
        assert_eq!(read(&dir, "c.txt"), "none\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── ignore rules ────────────────────────────────────────────────────

    #[test]
    fn gitignored_paths_are_never_searched_or_rewritten() {
        let dir = scratch(&[
            (".gitignore", "vendor/\nstorage/\n"),
            ("app/User.php", "secret\n"),
            ("vendor/laravel/framework/src/App.php", "secret\n"),
            ("storage/logs/laravel.log", "secret\n"),
        ]);
        let roots = vec![dir.clone()];

        let hits = search(&roots, "secret", insensitive(), 100, &none(), &|| false);
        assert_eq!(hits.len(), 1, "only the tracked file should match");
        assert!(hits[0].path.ends_with("app/User.php"));

        apply_replace(&roots, "secret", insensitive(), "redacted", &none());
        assert_eq!(read(&dir, "app/User.php"), "redacted\n");
        assert_eq!(
            read(&dir, "vendor/laravel/framework/src/App.php"),
            "secret\n",
            "a Composer dependency must not be rewritten"
        );
        assert_eq!(read(&dir, "storage/logs/laravel.log"), "secret\n");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn gitignore_is_honoured_inside_a_real_repo() {
        // The tests above run in a plain directory, which exercises the
        // `require_git(false)` path. Inside an actual repo the ignore rules go
        // through git's own machinery, so cover that too — it's the case every
        // real workspace hits.
        let dir = scratch(&[
            (".gitignore", "vendor/\n"),
            ("app/User.php", "secret\n"),
            ("vendor/pkg/File.php", "secret\n"),
        ]);
        let ran = std::process::Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(&dir)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ran {
            let _ = std::fs::remove_dir_all(&dir);
            return; // no git on this machine — the other tests still cover the rules
        }

        let roots = vec![dir.clone()];
        let hits = search(&roots, "secret", insensitive(), 100, &none(), &|| false);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].path.ends_with("app/User.php"));

        apply_replace(&roots, "secret", insensitive(), "redacted", &none());
        assert_eq!(read(&dir, "app/User.php"), "redacted\n");
        assert_eq!(read(&dir, "vendor/pkg/File.php"), "secret\n");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_tracked_vendor_dir_stays_searchable() {
        // This repo vendors Floem under `vendor/` and tracks it; blanket-
        // skipping the name would make our own source unsearchable.
        let dir = scratch(&[
            (".gitignore", "/target\n"),
            ("vendor/floem/src/lib.rs", "fn e() {}\n"),
        ]);
        let roots = vec![dir.clone()];
        let hits = search(&roots, "fn e", insensitive(), 100, &none(), &|| false);
        assert_eq!(hits.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_dirs_are_skipped_without_any_gitignore() {
        let dir = scratch(&[
            ("src/main.rs", "needle\n"),
            ("target/debug/build.rs", "needle\n"),
            ("node_modules/pkg/index.js", "needle\n"),
        ]);
        let roots = vec![dir.clone()];
        let hits = search(&roots, "needle", insensitive(), 100, &none(), &|| false);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].path.ends_with("src/main.rs"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── locating and rewriting ──────────────────────────────────────────

    #[test]
    fn hit_line_and_col_are_byte_offsets_into_the_original_text() {
        // `col` feeds `offset_of_line_col`, which counts UTF-8 bytes. Deriving
        // it from a lowercased copy (as the old code did) drifts on text whose
        // length changes under case folding.
        let dir = scratch(&[("a.txt", "first\nblåbær MATCH here\n")]);
        let roots = vec![dir.clone()];
        let hits = search(&roots, "match", insensitive(), 10, &none(), &|| false);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].line, 1);
        assert_eq!(hits[0].col, "blåbær ".len() as u32);
        assert_eq!(hits[0].text, "blåbær MATCH here");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_query_is_a_literal_not_a_regex() {
        let dir = scratch(&[("a.php", "$user->name() . '.*'\n")]);
        let roots = vec![dir.clone()];
        assert_eq!(
            search(&roots, "$user->name()", insensitive(), 10, &none(), &|| {
                false
            })
            .len(),
            1
        );
        // `.*` must match the two literal characters, not every character.
        let hits = search(&roots, ".*", insensitive(), 10, &none(), &|| false);
        assert_eq!(hits.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn case_insensitive_replace_writes_the_replacement_verbatim() {
        let dir = scratch(&[("a.txt", "User user USER\n")]);
        let roots = vec![dir.clone()];
        apply_replace(&roots, "user", insensitive(), "member", &none());
        assert_eq!(read(&dir, "a.txt"), "member member member\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A throwaway directory holding `files` (path → raw bytes).
    fn scratch_bytes(files: &[(&str, &[u8])]) -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("e-ws-bytes-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (rel, body) in files {
            std::fs::write(dir.join(rel), body).unwrap();
        }
        dir
    }

    fn utf16le(text: &str) -> Vec<u8> {
        let mut out = vec![0xFF, 0xFE];
        for u in text.encode_utf16() {
            out.extend_from_slice(&u.to_le_bytes());
        }
        out
    }

    #[test]
    fn files_are_read_and_written_back_in_their_own_encoding() {
        let latin1: &[u8] = b"Bl\xe5b\xe6r og \xf8l\n"; // "Blåbær og øl", windows-1252
        let dir = scratch_bytes(&[
            ("utf16.txt", &utf16le("hello world\n")),
            ("latin1.txt", latin1),
            ("bom.txt", b"\xEF\xBB\xBFhello bom\n"),
        ]);
        let roots = vec![dir.clone()];
        let hits = search(&roots, "hello", insensitive(), 10, &none(), &|| false);
        assert_eq!(hits.len(), 2, "UTF-16 and BOM files are searched: {hits:?}");
        assert_eq!(
            search(&roots, "øl", insensitive(), 10, &none(), &|| false).len(),
            1
        );

        let out = apply_replace(&roots, "hello", insensitive(), "hei", &none());
        assert_eq!(out.files_changed, 2, "{out:?}");
        assert_eq!(
            std::fs::read(dir.join("utf16.txt")).unwrap(),
            utf16le("hei world\n")
        );
        assert_eq!(
            std::fs::read(dir.join("bom.txt")).unwrap(),
            b"\xEF\xBB\xBFhei bom\n"
        );

        let out = apply_replace(&roots, "øl", insensitive(), "øl!", &none());
        assert_eq!(out.files_changed, 1);
        assert_eq!(
            std::fs::read(dir.join("latin1.txt")).unwrap(),
            b"Bl\xe5b\xe6r og \xf8l!\n"
        );
        // A replacement the file's encoding can't hold leaves it as it was.
        let out = apply_replace(&roots, "øl", insensitive(), "→", &none());
        assert_eq!(out.files_changed, 0);
        assert_eq!(out.failures.len(), 1, "{out:?}");
        assert_eq!(
            std::fs::read(dir.join("latin1.txt")).unwrap(),
            b"Bl\xe5b\xe6r og \xf8l!\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_files_are_read_from_the_editor_and_left_to_it() {
        let dir = scratch(&[("open.php", "saved\n"), ("closed.php", "needle\n")]);
        let roots = vec![dir.clone()];
        let mut open = OpenTexts::new();
        open.insert(
            dir.join("open.php").canonicalize().unwrap(),
            "needle not saved\n".into(),
        );

        let hits = search(&roots, "needle", insensitive(), 10, &open, &|| false);
        let mut files: Vec<_> = hits
            .iter()
            .map(|h| h.path.file_name().unwrap().to_owned())
            .collect();
        files.sort();
        assert_eq!(files, ["closed.php", "open.php"]);
        assert_eq!(
            plan_replace(&roots, "needle", insensitive(), &open).total_matches,
            2
        );

        let out = apply_replace(&roots, "needle", insensitive(), "pin", &open);
        assert_eq!(out.files_changed, 1, "only the closed file is written");
        assert_eq!(
            std::fs::read_to_string(dir.join("open.php")).unwrap(),
            "saved\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("closed.php")).unwrap(),
            "pin\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cancelled_search_stops() {
        let dir = scratch(&[("a.txt", "needle\n"), ("b.txt", "needle\n")]);
        let roots = vec![dir.clone()];
        assert!(search(&roots, "needle", insensitive(), 10, &none(), &|| true).is_empty());
        assert_eq!(
            search(&roots, "needle", insensitive(), 10, &none(), &|| false).len(),
            2
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn splice_rewrites_every_range() {
        assert_eq!(splice("a b a", &[(0, 1), (4, 5)], "z"), "z b z");
        assert_eq!(splice("abc", &[], "z"), "abc");
    }

    #[test]
    fn an_empty_query_matches_nothing() {
        let dir = scratch(&[("a.txt", "anything\n")]);
        let roots = vec![dir.clone()];
        assert!(search(&roots, "", insensitive(), 10, &none(), &|| false).is_empty());
        assert!(plan_replace(&roots, "", insensitive(), &none()).is_empty());
        assert_eq!(
            apply_replace(&roots, "", insensitive(), "x", &none()),
            ReplaceOutcome::default()
        );
        assert_eq!(read(&dir, "a.txt"), "anything\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn search_stops_at_the_hit_cap() {
        let body = "hit\n".repeat(50);
        let dir = scratch(&[("a.txt", body.as_str())]);
        let roots = vec![dir.clone()];
        assert_eq!(
            search(&roots, "hit", insensitive(), 10, &none(), &|| false).len(),
            10
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn binaries_are_left_alone() {
        let dir = scratch(&[("a.bin", "needle\0needle\n"), ("b.txt", "needle\n")]);
        let roots = vec![dir.clone()];
        let hits = search(&roots, "needle", insensitive(), 10, &none(), &|| false);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].path.ends_with("b.txt"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
