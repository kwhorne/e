//! What a rename is about to do, before it does it.
//!
//! Rename used to replace whole-word matches in the active buffer and write them
//! straight in. Two things were wrong with that: it was textual, so it renamed
//! inside strings and comments and missed every other file; and it showed you
//! nothing, so the first you knew of a bad rename was the diff afterwards.
//!
//! The language server answers `textDocument/rename` with edits across the whole
//! workspace. This turns those into something a reader can check — one line per
//! site, with the text as it will read — and that list is what the dialog shows.

use std::collections::BTreeMap;
use std::path::PathBuf;

use lsp_types::TextEdit;

/// One site a rename would change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Site {
    /// 0-based line in the file.
    pub line: u32,
    /// The line as it reads now, trimmed.
    pub before: String,
    /// The same line with the rename applied, trimmed.
    pub after: String,
}

/// The sites in one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilePlan {
    pub path: PathBuf,
    pub sites: Vec<Site>,
    /// The edits themselves, kept so confirming applies exactly what was
    /// previewed rather than re-deriving it from the display rows.
    pub edits: Vec<TextEdit>,
}

/// Everything a rename would do.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RenamePlan {
    pub old_name: String,
    pub new_name: String,
    /// Files in path order, so the list doesn't reshuffle between runs.
    pub files: Vec<FilePlan>,
    /// Files the server named but that couldn't be read. Reported rather than
    /// dropped: a rename that silently skips a file is worse than one that says
    /// it couldn't.
    pub unreadable: Vec<PathBuf>,
}

impl RenamePlan {
    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    pub fn site_count(&self) -> usize {
        self.files.iter().map(|f| f.sites.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.site_count() == 0
    }

    /// `12 sites in 4 files`, for the dialog.
    pub fn summary(&self) -> String {
        let (s, f) = (self.site_count(), self.file_count());
        format!(
            "{s} {} in {f} {}",
            if s == 1 { "site" } else { "sites" },
            if f == 1 { "file" } else { "files" },
        )
    }
}

/// Byte offset of the start of each line.
fn line_starts(text: &str) -> Vec<usize> {
    let mut v = vec![0];
    v.extend(
        text.match_indices('\n')
            .map(|(i, _)| i + 1)
            .filter(|&i| i < text.len()),
    );
    v
}

/// Byte offset of an LSP position. Characters are counted as UTF-8 bytes, which
/// is what the rest of the editor uses.
fn offset_of(text: &str, starts: &[usize], line: u32, character: u32) -> Option<usize> {
    let start = *starts.get(line as usize)?;
    let rest = &text[start..];
    let line_text = rest.split('\n').next().unwrap_or("");
    Some(start + (character as usize).min(line_text.len()))
}

/// Apply a file's edits to its text.
///
/// Applied last-first so earlier offsets stay valid — the same reason the rest
/// of the editor does it that way.
pub fn apply_edits(text: &str, edits: &[TextEdit]) -> String {
    let starts = line_starts(text);
    let mut spans: Vec<(usize, usize, &str)> = edits
        .iter()
        .filter_map(|e| {
            let s = offset_of(text, &starts, e.range.start.line, e.range.start.character)?;
            let t = offset_of(text, &starts, e.range.end.line, e.range.end.character)?;
            (t >= s).then_some((s, t, e.new_text.as_str()))
        })
        .collect();
    spans.sort_by_key(|&(s, _, _)| s);

    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (s, t, new) in spans {
        if s < last {
            // Overlapping edits would corrupt the result; a server shouldn't
            // send them, and applying them anyway is worse than skipping one.
            continue;
        }
        out.push_str(&text[last..s]);
        out.push_str(new);
        last = t;
    }
    out.push_str(&text[last..]);
    out
}

/// Build the plan from a server's workspace edit.
///
/// `read` resolves a path to its current text, so the caller can serve open
/// buffers from memory rather than from disk — renaming against a stale copy of
/// a file the user has edited would produce a preview that doesn't match what
/// gets written.
pub fn plan<F>(
    old_name: &str,
    new_name: &str,
    edits: &[(String, Vec<TextEdit>)],
    mut read: F,
) -> RenamePlan
where
    F: FnMut(&std::path::Path) -> Option<String>,
{
    let mut plan = RenamePlan {
        old_name: old_name.to_string(),
        new_name: new_name.to_string(),
        ..Default::default()
    };

    // Group by path first: a server may send several entries for one file.
    let mut by_path: BTreeMap<PathBuf, Vec<TextEdit>> = BTreeMap::new();
    for (uri, es) in edits {
        by_path
            .entry(e_lsp::uri_to_path(uri))
            .or_default()
            .extend(es.iter().cloned());
    }

    for (path, es) in by_path {
        let Some(text) = read(&path) else {
            plan.unreadable.push(path);
            continue;
        };
        let starts = line_starts(&text);
        let after_text = apply_edits(&text, &es);
        let after_starts = line_starts(&after_text);

        // One row per changed line, in file order.
        let mut lines: Vec<u32> = es.iter().map(|e| e.range.start.line).collect();
        lines.sort_unstable();
        lines.dedup();

        let mut sites = Vec::new();
        for line in &lines {
            let before = nth_line(&text, &starts, *line as usize);
            // Earlier edits can add *or remove* lines, so the same content sits
            // at a different index in the new text. The shift is signed: a
            // collapsed block moves later lines up, and clamping that to zero
            // pointed the preview at a line that no longer exists.
            let moved = (*line as i64 + shift(&es, *line)).max(0) as usize;
            let after = nth_line(&after_text, &after_starts, moved);
            sites.push(Site {
                line: *line,
                before: before.trim().to_string(),
                after: after.trim().to_string(),
            });
        }
        if !sites.is_empty() {
            plan.files.push(FilePlan {
                path,
                sites,
                edits: es,
            });
        }
    }
    plan
}

/// How many lines the edits *before* `line` add or remove, signed.
///
/// Renames are almost always single-line, but a server is allowed to rewrite a
/// block, and then every later line moves. Negative is the interesting case: it
/// means lines were collapsed.
fn shift(edits: &[TextEdit], line: u32) -> i64 {
    let mut delta: i64 = 0;
    for e in edits {
        if e.range.end.line >= line {
            continue;
        }
        let removed = (e.range.end.line - e.range.start.line) as i64;
        let added = e.new_text.matches('\n').count() as i64;
        delta += added - removed;
    }
    delta
}

fn nth_line(text: &str, starts: &[usize], line: usize) -> String {
    starts
        .get(line)
        .map(|&s| text[s..].split('\n').next().unwrap_or("").to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{Position, Range};

    fn edit(line: u32, start: u32, end: u32, new: &str) -> TextEdit {
        TextEdit {
            range: Range {
                start: Position {
                    line,
                    character: start,
                },
                end: Position {
                    line,
                    character: end,
                },
            },
            new_text: new.to_string(),
        }
    }

    fn reader(files: Vec<(&str, &str)>) -> impl FnMut(&std::path::Path) -> Option<String> {
        let map: std::collections::HashMap<PathBuf, String> = files
            .into_iter()
            .map(|(p, t)| (PathBuf::from(p), t.to_string()))
            .collect();
        move |p: &std::path::Path| map.get(p).cloned()
    }

    #[test]
    fn a_plan_shows_each_line_before_and_after() {
        let text = "class Order {\n    public $total;\n    public function total() {}\n}\n";
        let edits = vec![(
            "file:///app/Order.php".to_string(),
            vec![edit(1, 11, 17, "$amount")],
        )];
        let p = plan(
            "total",
            "amount",
            &edits,
            reader(vec![("/app/Order.php", text)]),
        );
        assert_eq!(p.site_count(), 1);
        assert_eq!(p.file_count(), 1);
        assert_eq!(p.summary(), "1 site in 1 file");
        assert_eq!(p.files[0].sites[0].before, "public $total;");
        assert_eq!(p.files[0].sites[0].after, "public $amount;");
    }

    #[test]
    fn several_files_are_listed_in_path_order() {
        // Stable order matters: the same rename must not reshuffle its own
        // preview between runs.
        let edits = vec![
            ("file:///app/Z.php".to_string(), vec![edit(0, 0, 3, "new")]),
            ("file:///app/A.php".to_string(), vec![edit(0, 0, 3, "new")]),
        ];
        let p = plan(
            "old",
            "new",
            &edits,
            reader(vec![("/app/Z.php", "old\n"), ("/app/A.php", "old\n")]),
        );
        let paths: Vec<_> = p.files.iter().map(|f| f.path.to_str().unwrap()).collect();
        assert_eq!(paths, ["/app/A.php", "/app/Z.php"]);
        assert_eq!(p.summary(), "2 sites in 2 files");
    }

    #[test]
    fn several_edits_on_one_line_produce_one_row() {
        // `$total + $total` is one line a reader checks once, not two.
        let text = "$sum = $total + $total;\n";
        let edits = vec![(
            "file:///a.php".to_string(),
            vec![edit(0, 7, 13, "$amount"), edit(0, 16, 22, "$amount")],
        )];
        let p = plan("$total", "$amount", &edits, reader(vec![("/a.php", text)]));
        assert_eq!(p.files[0].sites.len(), 1);
        assert_eq!(p.files[0].sites[0].after, "$sum = $amount + $amount;");
    }

    #[test]
    fn a_file_that_cannot_be_read_is_reported_not_skipped() {
        let edits = vec![
            ("file:///a.php".to_string(), vec![edit(0, 0, 3, "new")]),
            ("file:///gone.php".to_string(), vec![edit(0, 0, 3, "new")]),
        ];
        let p = plan("old", "new", &edits, reader(vec![("/a.php", "old\n")]));
        assert_eq!(p.file_count(), 1);
        assert_eq!(p.unreadable, [PathBuf::from("/gone.php")]);
    }

    #[test]
    fn applying_edits_is_right_to_left() {
        // Left-to-right application would invalidate every offset after the
        // first edit that changes length.
        let text = "aa bb cc\n";
        let edits = vec![
            edit(0, 0, 2, "xxxx"),
            edit(0, 3, 5, "y"),
            edit(0, 6, 8, "zzz"),
        ];
        assert_eq!(apply_edits(text, &edits), "xxxx y zzz\n");
    }

    #[test]
    fn overlapping_edits_are_skipped_rather_than_corrupting_the_text() {
        let text = "abcdef\n";
        let edits = vec![edit(0, 0, 4, "X"), edit(0, 2, 6, "Y")];
        let out = apply_edits(text, &edits);
        assert!(out.starts_with('X'), "{out}");
        assert!(!out.contains("XY"), "the second edit overlapped: {out}");
    }

    #[test]
    fn a_multi_line_edit_keeps_the_preview_aligned() {
        let text = "one\ntwo\nthree\nfour\n";
        // Replace lines 0-1 with a single line, then rename on line 3.
        let edits = vec![(
            "file:///a.txt".to_string(),
            vec![edit(0, 0, 3, "1"), edit(3, 0, 4, "IV")],
        )];
        let mut e2 = edits.clone();
        e2[0].1[0].range.end = Position {
            line: 1,
            character: 3,
        };
        let p = plan("x", "y", &e2, reader(vec![("/a.txt", text)]));
        let last = p.files[0].sites.last().unwrap();
        assert_eq!(last.line, 3);
        assert_eq!(
            last.after, "IV",
            "the row must show the renamed line: {last:?}"
        );
    }

    #[test]
    fn an_empty_result_is_an_empty_plan() {
        let p = plan("old", "new", &[], reader(vec![]));
        assert!(p.is_empty());
        assert_eq!(p.site_count(), 0);
    }

    #[test]
    fn a_position_past_the_end_of_a_line_is_clamped() {
        // Servers occasionally report a character past the line end; clamping
        // beats panicking on a slice boundary.
        let text = "ab\n";
        let edits = vec![edit(0, 0, 99, "z")];
        assert_eq!(apply_edits(text, &edits), "z\n");
    }
}
