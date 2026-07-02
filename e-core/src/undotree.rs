//! A persistent, branching undo *tree*.
//!
//! Unlike a linear undo stack, every distinct document state becomes a node
//! whose parent is the state it was edited from. Undoing then typing again
//! creates a *sibling branch* instead of discarding the redo history, so no
//! edit is ever truly lost — and the whole tree is saved to disk, giving
//! "time travel" across sessions.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Stop growing new branches past this many nodes (keeps memory/disk bounded).
const MAX_NODES: usize = 500;

#[derive(Clone, Serialize, Deserialize)]
pub struct UndoNode {
    pub id: usize,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    /// Epoch milliseconds when this state was recorded.
    pub ts: u64,
    /// Full document text at this node.
    pub text: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct UndoTree {
    pub nodes: Vec<UndoNode>,
    pub current: usize,
    #[serde(skip)]
    last_ms: u64,
    #[serde(skip)]
    last_save_ms: u64,
}

impl UndoTree {
    pub fn new(initial: impl Into<String>) -> Self {
        Self {
            nodes: vec![UndoNode {
                id: 0,
                parent: None,
                children: Vec::new(),
                ts: 0,
                text: initial.into(),
            }],
            current: 0,
            last_ms: 0,
            last_save_ms: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.len() <= 1
    }

    pub fn current_text(&self) -> &str {
        &self.nodes[self.current].text
    }

    /// Record a new document state, branching where necessary.
    ///
    /// Returns `true` if the tree changed (a node was added or `current` moved).
    /// `coalesce_ms` merges a rapid burst of edits into a single leaf node.
    pub fn record(&mut self, text: &str, now_ms: u64, coalesce_ms: u64) -> bool {
        if self.nodes[self.current].text == text {
            return false;
        }
        // Undo: the new text is the parent's text.
        if let Some(p) = self.nodes[self.current].parent {
            if self.nodes[p].text == text {
                self.current = p;
                return true;
            }
        }
        // Redo: the new text matches an existing child.
        if let Some(c) = self.nodes[self.current]
            .children
            .iter()
            .rev()
            .find(|&&c| self.nodes[c].text == text)
            .copied()
        {
            self.current = c;
            self.last_ms = now_ms;
            return true;
        }
        // Coalesce a typing burst into the current leaf.
        let is_leaf = self.nodes[self.current].children.is_empty();
        let bursting = now_ms.saturating_sub(self.last_ms) < coalesce_ms;
        if self.current != 0 && is_leaf && (bursting || self.nodes.len() >= MAX_NODES) {
            let cur = self.current;
            self.nodes[cur].text = text.to_string();
            self.nodes[cur].ts = now_ms;
            self.last_ms = now_ms;
            return true;
        }
        // New branch.
        let id = self.nodes.len();
        let parent = self.current;
        self.nodes.push(UndoNode {
            id,
            parent: Some(parent),
            children: Vec::new(),
            ts: now_ms,
            text: text.to_string(),
        });
        self.nodes[parent].children.push(id);
        self.current = id;
        self.last_ms = now_ms;
        true
    }

    /// Move to the parent state.
    pub fn undo(&mut self) -> Option<String> {
        let p = self.nodes[self.current].parent?;
        self.current = p;
        Some(self.nodes[p].text.clone())
    }

    /// Move to the most-recent child state.
    pub fn redo(&mut self) -> Option<String> {
        let c = *self.nodes[self.current]
            .children
            .iter()
            .max_by_key(|&&c| self.nodes[c].ts)?;
        self.current = c;
        Some(self.nodes[c].text.clone())
    }

    /// Jump to an arbitrary node.
    pub fn goto(&mut self, id: usize) -> Option<String> {
        let text = self.nodes.get(id)?.text.clone();
        self.current = id;
        Some(text)
    }

    /// After a reload, point `current` at the node matching `content` (if any).
    pub fn sync_to(&mut self, content: &str) -> bool {
        if let Some(id) = self
            .nodes
            .iter()
            .rev()
            .find(|n| n.text == content)
            .map(|n| n.id)
        {
            self.current = id;
            true
        } else {
            false
        }
    }

    /// Persist, throttled to at most once per 1.5s.
    pub fn maybe_save(&mut self, path: &Path, now_ms: u64) {
        if now_ms.saturating_sub(self.last_save_ms) < 1500 {
            return;
        }
        self.last_save_ms = now_ms;
        self.save(path);
    }

    pub fn save(&self, path: &Path) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_string(self) {
            let _ = std::fs::write(path, json);
        }
    }

    pub fn load(path: &Path) -> Option<Self> {
        let data = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&data).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branches_instead_of_discarding() {
        let mut t = UndoTree::new("");
        t.record("a", 1000, 700); // node 1
        t.record("ab", 3000, 700); // node 2 (not bursting)
                                   // Undo back to "a".
        assert_eq!(t.undo().as_deref(), Some("a"));
        // Type something else -> new sibling branch, "ab" preserved.
        t.record("ax", 5000, 700); // node 3, sibling of node 2
        assert_eq!(t.current_text(), "ax");
        // The old "ab" branch still exists.
        assert!(t.nodes.iter().any(|n| n.text == "ab"));
        // Redo picks the most recent child ("ax").
        assert_eq!(t.undo().as_deref(), Some("a"));
        assert_eq!(t.redo().as_deref(), Some("ax"));
    }

    #[test]
    fn coalesces_bursts() {
        let mut t = UndoTree::new("");
        t.record("h", 100, 700);
        t.record("he", 150, 700);
        t.record("hel", 200, 700);
        // One burst -> one node past the root.
        assert_eq!(t.len(), 2);
        assert_eq!(t.current_text(), "hel");
    }

    #[test]
    fn saves_and_loads() {
        let mut t = UndoTree::new("root");
        t.record("a", 1000, 700);
        t.record("ab", 3000, 700);
        let dir = std::env::temp_dir().join(format!("e-undo-test-{}", std::process::id()));
        let path = dir.join("t.json");
        t.save(&path);
        let loaded = UndoTree::load(&path).expect("load");
        assert_eq!(loaded.len(), t.len());
        assert_eq!(loaded.current_text(), "ab");
        assert!(loaded.nodes.iter().any(|n| n.text == "a"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn detects_undo_redo_as_moves() {
        let mut t = UndoTree::new("");
        t.record("a", 1000, 700);
        t.record("ab", 3000, 700);
        assert_eq!(t.len(), 3);
        // Simulating Floem's linear undo (text becomes parent's).
        assert!(t.record("a", 4000, 700));
        assert_eq!(t.current, 1);
        // Redo via observing the child text again.
        assert!(t.record("ab", 5000, 700));
        assert_eq!(t.current, 2);
        assert_eq!(t.len(), 3); // no new nodes
    }
}
