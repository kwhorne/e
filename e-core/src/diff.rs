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
}
