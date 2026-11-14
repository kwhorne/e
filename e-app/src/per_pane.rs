//! Something a buffer has once per editor pane: its `Editor` (caret,
//! selection, scroll) and where that editor sits in the window.
//!
//! With the editor split, each pane builds its own `Editor` over the shared
//! document. Kept in one slot, the second pane to build overwrote the first,
//! so a key, a completion, a goto or a hover meant for the left pane moved
//! the caret in the right one. Here each pane has its slot, and a read picks
//! the pane the buffer is being worked in: the focused pane if it shows the
//! buffer, else whichever pane does.

use floem::reactive::{RwSignal, SignalGet, SignalUpdate};

/// The panes and which buffer each shows, to decide which slot a read means.
#[derive(Clone, Copy)]
pub struct Panes {
    pub split: RwSignal<bool>,
    pub focused: RwSignal<u8>,
    pub active: [RwSignal<Option<u64>>; 2],
}

/// Which pane buffer `id` is being worked in: the focused pane if it shows
/// it, else the other pane that does, else the first.
pub fn pane_of(id: u64, split: bool, focused: u8, active: [Option<u64>; 2]) -> usize {
    if !split {
        return 0;
    }
    let focused = usize::from(focused.min(1));
    if active[focused] == Some(id) {
        focused
    } else if active[1 - focused] == Some(id) {
        1 - focused
    } else {
        0
    }
}

pub struct PerPane<T: 'static> {
    id: u64,
    slots: [RwSignal<T>; 2],
    panes: Panes,
}

// Signals are handles: a `PerPane` copies whatever `T` is (a derive would
// ask `T: Copy`, which an `Option<Editor>` isn't).
impl<T: 'static> Clone for PerPane<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: 'static> Copy for PerPane<T> {}

impl<T: Clone + 'static> PerPane<T> {
    pub fn new(id: u64, panes: Panes, init: T) -> Self {
        Self {
            id,
            slots: [RwSignal::new(init.clone()), RwSignal::new(init)],
            panes,
        }
    }

    fn pane(&self, tracked: bool) -> usize {
        let p = self.panes;
        if tracked {
            pane_of(
                self.id,
                p.split.get(),
                p.focused.get(),
                [p.active[0].get(), p.active[1].get()],
            )
        } else {
            pane_of(
                self.id,
                p.split.get_untracked(),
                p.focused.get_untracked(),
                [p.active[0].get_untracked(), p.active[1].get_untracked()],
            )
        }
    }

    /// The value for the pane the buffer is being worked in (tracked).
    pub fn get(&self) -> T {
        self.slots[self.pane(true)].get()
    }

    /// The value for the pane the buffer is being worked in.
    pub fn get_untracked(&self) -> T {
        self.slots[self.pane(false)].get_untracked()
    }

    /// Pane `pane`'s value, whichever pane the buffer is worked in.
    pub fn get_in(&self, pane: u8) -> T {
        self.slots[usize::from(pane.min(1))].get_untracked()
    }

    /// Pane `pane`'s value, tracked: for a view that belongs to one pane.
    pub fn track_in(&self, pane: u8) -> T {
        self.slots[usize::from(pane.min(1))].get()
    }

    /// Set pane `pane`'s value.
    pub fn set_in(&self, pane: u8, value: T) {
        self.slots[usize::from(pane.min(1))].set(value);
    }
}

#[cfg(test)]
mod tests {
    use super::pane_of;

    #[test]
    fn a_read_means_the_pane_the_buffer_is_worked_in() {
        // Unsplit: always the first pane.
        assert_eq!(pane_of(7, false, 1, [Some(7), Some(7)]), 0);
        // The same file in both panes: the focused one.
        assert_eq!(pane_of(7, true, 0, [Some(7), Some(7)]), 0);
        assert_eq!(pane_of(7, true, 1, [Some(7), Some(7)]), 1);
        // Shown only in the pane that isn't focused: that pane.
        assert_eq!(pane_of(7, true, 0, [Some(3), Some(7)]), 1);
        assert_eq!(pane_of(7, true, 1, [Some(7), Some(3)]), 0);
        // Shown nowhere: the first.
        assert_eq!(pane_of(7, true, 1, [Some(3), Some(4)]), 0);
    }
}
