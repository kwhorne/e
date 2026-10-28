//! Line-based diffing into ordered segments (equal context vs. changes),
//! used to present agent-proposed edits for hunk-by-hunk review.

use similar::{ChangeTag, TextDiff};

/// One ordered segment of a diff.
pub struct DiffSeg {
    /// True for unchanged context; false for a change (old → new).
    pub equal: bool,
    pub old: String,
    pub new: String,
}

/// Diff `old` vs `new` into a sequence of equal/change segments (line-based).
pub fn edit_segments(old: &str, new: &str) -> Vec<DiffSeg> {
    let diff = TextDiff::from_lines(old, new);
    let mut segs: Vec<DiffSeg> = Vec::new();
    let mut eq = String::new();
    let mut o = String::new();
    let mut n = String::new();
    // 0 = start, 1 = accumulating equal, 2 = accumulating change.
    let mut mode = 0u8;

    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Equal => {
                if mode == 2 {
                    segs.push(DiffSeg {
                        equal: false,
                        old: std::mem::take(&mut o),
                        new: std::mem::take(&mut n),
                    });
                }
                eq.push_str(change.value());
                mode = 1;
            }
            ChangeTag::Delete => {
                if mode == 1 {
                    segs.push(DiffSeg {
                        equal: true,
                        old: std::mem::take(&mut eq),
                        new: String::new(),
                    });
                }
                o.push_str(change.value());
                mode = 2;
            }
            ChangeTag::Insert => {
                if mode == 1 {
                    segs.push(DiffSeg {
                        equal: true,
                        old: std::mem::take(&mut eq),
                        new: String::new(),
                    });
                }
                n.push_str(change.value());
                mode = 2;
            }
        }
    }
    if mode == 1 && !eq.is_empty() {
        segs.push(DiffSeg {
            equal: true,
            old: eq,
            new: String::new(),
        });
    } else if mode == 2 {
        segs.push(DiffSeg {
            equal: false,
            old: o,
            new: n,
        });
    }
    segs
}

/// Where `lines` (0-based lines of `old`) are in `new`. A line keeps its place
/// among the unchanged lines around it, so text added above pushes it down; an
/// edited line stays put, and lines joined into one all land on it. A line that
/// was removed is dropped. The result is sorted, without duplicates.
pub fn follow_lines(old: &str, new: &str, lines: &[usize]) -> Vec<usize> {
    if lines.is_empty() || old == new {
        return lines.to_vec();
    }
    let diff = TextDiff::from_lines(old, new);
    let ops = diff.ops();
    let end = |op: Option<&similar::DiffOp>| {
        op.map_or((0, 0), |o| (o.old_range().end, o.new_range().end))
    };
    let (old_end, new_end) = end(ops.last());
    let mut out: Vec<usize> = lines
        .iter()
        .filter_map(|&line| {
            let Some(op) = ops.iter().find(|op| op.old_range().contains(&line)) else {
                // Past the last line the diff saw, such as the empty line after
                // a final newline: it moves with the end of the text.
                return (line >= old_end).then(|| line - old_end + new_end);
            };
            let (from, to) = (op.old_range(), op.new_range());
            match op.tag() {
                similar::DiffTag::Equal => Some(to.start + line - from.start),
                similar::DiffTag::Replace => Some(to.start + (line - from.start).min(to.len() - 1)),
                similar::DiffTag::Delete | similar::DiffTag::Insert => None,
            }
        })
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_roundtrip() {
        let old = "a\nb\nc\n";
        let new = "a\nB\nc\nd\n";
        let segs = edit_segments(old, new);
        // Rebuild "accept all" == new.
        let rebuilt: String = segs
            .iter()
            .map(|s| {
                if s.equal {
                    s.old.clone()
                } else {
                    s.new.clone()
                }
            })
            .collect();
        assert_eq!(rebuilt, new);
        // Rebuild "reject all" == old.
        let orig: String = segs.iter().map(|s| s.old.clone()).collect();
        assert_eq!(orig, old);
        // There is at least one change segment.
        assert!(segs.iter().any(|s| !s.equal));
    }

    #[test]
    fn lines_added_above_push_a_line_down() {
        let old = "a\nb\nc\n";
        assert_eq!(follow_lines(old, "x\ny\na\nb\nc\n", &[1, 2]), vec![3, 4]);
        // Below it, nothing moves.
        assert_eq!(follow_lines(old, "a\nb\nc\nd\n", &[1]), vec![1]);
    }

    #[test]
    fn an_edited_line_stays_and_a_removed_one_goes() {
        let old = "a\nb\nc\nd\n";
        assert_eq!(follow_lines(old, "a\nbee\nc\nd\n", &[1, 3]), vec![1, 3]);
        assert_eq!(follow_lines(old, "a\nc\nd\n", &[1, 3]), vec![2]);
    }

    #[test]
    fn joined_lines_land_on_the_one_line_left() {
        let old = "a\nb\nc\nd\n";
        assert_eq!(follow_lines(old, "a\nbc\nd\n", &[1, 2, 3]), vec![1, 2]);
    }

    #[test]
    fn a_whole_text_swap_keeps_the_lines_that_survive() {
        // Undo and formatting replace the whole text in one edit.
        let old = "<?php\n\nfoo();\nbar();\n";
        let new = "<?php\n\n// note\nfoo();\nbar();\n";
        assert_eq!(follow_lines(old, new, &[2, 3]), vec![3, 4]);
        // The empty line after the final newline moves with the end.
        assert_eq!(follow_lines(old, new, &[4]), vec![5]);
    }
}
