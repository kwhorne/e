//! `Buffer::edit` must not be able to abort the process.
//!
//! It used to: a selection carrying offsets from an older revision, or two
//! selections overlapping each other, produced a delta the CRDT engine rejected
//! with an assertion. Those assertions fire inside a callback that cannot
//! unwind, so the process aborts and unsaved work is lost.

use floem_editor_core::buffer::rope_text::RopeText;
use floem_editor_core::buffer::Buffer;
use floem_editor_core::editor::EditType;
use floem_editor_core::selection::Selection;

/// Deterministic pseudo-randomness — no dev-dependency, same cases every run.
fn lcg(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *state >> 33
}

#[test]
fn offsets_past_the_end_are_clamped_instead_of_aborting() {
    // The exact shape from the crash log: multiset.rs "self must cover all
    // 0-regions of other", reached from Buffer::edit.
    let mut b = Buffer::new("hello");
    let stale = Selection::region(20, 30);
    b.edit([(&stale, "X")], EditType::InsertChars);
    assert_eq!(b.text().to_string(), "helloX");
}

#[test]
fn a_half_stale_region_keeps_the_part_that_is_still_valid() {
    let mut b = Buffer::new("hello");
    let half = Selection::region(2, 99);
    b.edit([(&half, "Y")], EditType::InsertChars);
    assert_eq!(b.text().to_string(), "heY");
}

#[test]
fn overlapping_selections_across_edits_do_not_abort() {
    let mut b = Buffer::new("aaaa bbbb cccc");
    let first = Selection::region(0, 8);
    let second = Selection::region(4, 12);
    b.edit([(&first, "X"), (&second, "Y")], EditType::InsertChars);
    // The conflicting second region is dropped rather than corrupting the delta.
    assert_eq!(b.text().to_string(), "Xb cccc");
}

#[test]
fn identical_selections_across_edits_do_not_abort() {
    let mut b = Buffer::new("hello world");
    let a = Selection::region(0, 5);
    let c = Selection::region(0, 5);
    b.edit([(&a, "X"), (&c, "Y")], EditType::InsertChars);
    assert_eq!(b.text().to_string(), "X world");
}

#[test]
fn empty_buffer_with_any_selection_does_not_abort() {
    for (start, end) in [(0, 0), (0, 5), (3, 9), (7, 7)] {
        let mut b = Buffer::new("");
        let sel = Selection::region(start, end);
        b.edit([(&sel, "Z")], EditType::InsertChars);
        assert_eq!(b.text().to_string(), "Z", "region {start}..{end}");
    }
}

/// The real guard: throw many degenerate interval sets at `edit` and require
/// that none of them abort. Exhaustive reasoning about `DeltaBuilder`'s contract
/// is harder than covering the space.
#[test]
fn no_combination_of_selections_can_abort_the_editor() {
    let mut seed = 0xC0FFEEu64;
    for case in 0..2000 {
        let base_len = (lcg(&mut seed) % 30) as usize;
        let base: String = (0..base_len)
            .map(|i| (b'a' + (i % 26) as u8) as char)
            .collect();
        let mut b = Buffer::new(&base);

        // Up to four selections, deliberately allowed to run past the end,
        // to be empty, to repeat and to overlap each other.
        let n = 1 + (lcg(&mut seed) % 4) as usize;
        let sels: Vec<Selection> = (0..n)
            .map(|_| {
                let a = (lcg(&mut seed) % 40) as usize;
                let b = (lcg(&mut seed) % 40) as usize;
                Selection::region(a.min(b), a.max(b))
            })
            .collect();
        let edits: Vec<(&Selection, &str)> = sels.iter().map(|s| (s, "*")).collect();

        b.edit(edits, EditType::InsertChars);
        // Whatever it produced, the buffer must remain coherent.
        let text = b.text().to_string();
        assert_eq!(
            text.len(),
            b.len(),
            "case {case}: length disagrees with rope"
        );
    }
}

#[test]
fn a_selection_left_over_from_longer_text_does_not_abort() {
    // The scenario from the field: the buffer is replaced by something shorter
    // -- a disk reload, an agent edit, an undo-tree jump -- while a selection
    // measured against the old text is still live, and the next keystroke uses
    // it.
    let mut b = Buffer::new("a much longer piece of text than what follows");
    let stale = Selection::region(30, 40);

    let whole = Selection::region(0, b.len());
    b.edit([(&whole, "short")], EditType::Delete);
    assert_eq!(b.text().to_string(), "short");

    b.edit([(&stale, "!")], EditType::InsertChars);
    assert_eq!(b.text().to_string(), "short!");
}
