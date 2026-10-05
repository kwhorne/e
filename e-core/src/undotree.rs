//! A persistent, branching undo *tree*.
//!
//! Unlike a linear undo stack, every distinct document state becomes a node
//! whose parent is the state it was edited from. Undoing then typing again
//! creates a *sibling branch* instead of discarding the redo history, so no
//! edit is ever truly lost — and the whole tree is saved to disk, giving
//! "time travel" across sessions.
//!
//! Only the current state is held as full text. Every other node stores the
//! one splice that turns its parent into it, so a long session on a large file
//! costs the size of the edits rather than a copy of the file per step (which
//! is what it used to cost — up to 500 copies, rewritten to disk as you typed).

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Stop growing new branches past this many nodes (keeps memory/disk bounded).
const MAX_NODES: usize = 500;

/// ... or past this many bytes of stored edits. A whole-file reformat stores
/// the file twice (old and new), so a few hundred of those on a large file
/// would otherwise add up.
const MAX_EDIT_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UndoNode {
    pub id: usize,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    /// Epoch milliseconds when this state was recorded.
    pub ts: u64,
    /// The splice from the parent's text to this node's: at byte `at`, the
    /// parent has `removed` where this node has `inserted`. Empty on the root.
    pub at: usize,
    pub removed: String,
    pub inserted: String,
}

impl UndoNode {
    /// How many bytes longer this state is than its parent's.
    pub fn size_delta(&self) -> i64 {
        self.inserted.len() as i64 - self.removed.len() as i64
    }

    fn edit_bytes(&self) -> usize {
        self.removed.len() + self.inserted.len()
    }

    /// Parent text → this node's text, in place.
    fn apply(&self, text: &mut String) {
        text.replace_range(self.at..self.at + self.removed.len(), &self.inserted);
    }

    /// This node's text → the parent's, in place.
    fn revert(&self, text: &mut String) {
        text.replace_range(self.at..self.at + self.inserted.len(), &self.removed);
    }

    /// Whether `text` has `old` at `at`, on character boundaries, so that the
    /// splice can be applied to it without panicking.
    fn fits(text: &str, at: usize, old: &str) -> bool {
        text.get(at..at + old.len()) == Some(old)
    }

    /// Whether `to` is `from` with this node reverted, without building it.
    fn reverts_to(&self, from: &str, to: &str) -> bool {
        splice_matches(from, self.at, &self.inserted, &self.removed, to)
    }

    /// Whether `to` is `from` with this node applied, without building it.
    fn applies_to(&self, from: &str, to: &str) -> bool {
        splice_matches(from, self.at, &self.removed, &self.inserted, to)
    }
}

/// Whether replacing `old` at `at` in `from` with `new` gives exactly `to`.
fn splice_matches(from: &str, at: usize, old: &str, new: &str, to: &str) -> bool {
    let tail = at + old.len();
    from.len() - old.len() + new.len() == to.len()
        && from.get(at..tail) == Some(old)
        && from.as_bytes()[..at] == to.as_bytes()[..at]
        && to.get(at..at + new.len()) == Some(new)
        && from.as_bytes()[tail..] == to.as_bytes()[at + new.len()..]
}

/// The smallest single splice from `a` to `b`: `(at, removed, inserted)`.
fn splice(a: &str, b: &str) -> (usize, String, String) {
    let (ab, bb) = (a.as_bytes(), b.as_bytes());
    let mut pre = ab.iter().zip(bb).take_while(|(x, y)| x == y).count();
    while !a.is_char_boundary(pre) || !b.is_char_boundary(pre) {
        pre -= 1;
    }
    let max_suf = ab.len().min(bb.len()) - pre;
    let mut suf = ab
        .iter()
        .rev()
        .zip(bb.iter().rev())
        .take(max_suf)
        .take_while(|(x, y)| x == y)
        .count();
    while !a.is_char_boundary(ab.len() - suf) || !b.is_char_boundary(bb.len() - suf) {
        suf -= 1;
    }
    (
        pre,
        a[pre..ab.len() - suf].to_string(),
        b[pre..bb.len() - suf].to_string(),
    )
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UndoTree {
    pub nodes: Vec<UndoNode>,
    pub current: usize,
    /// The full text of the `current` node; every other state is reached from
    /// here by walking splices.
    text: String,
    #[serde(skip)]
    edit_bytes: usize,
    #[serde(skip)]
    last_ms: u64,
    #[serde(skip)]
    last_save_ms: u64,
}

/// The format before splices: every node carried its full text.
#[derive(Deserialize)]
struct LegacyTree {
    nodes: Vec<LegacyNode>,
    current: usize,
}

#[derive(Deserialize)]
struct LegacyNode {
    id: usize,
    parent: Option<usize>,
    children: Vec<usize>,
    ts: u64,
    text: String,
}

impl UndoTree {
    pub fn new(initial: impl Into<String>) -> Self {
        Self {
            nodes: vec![UndoNode {
                id: 0,
                parent: None,
                children: Vec::new(),
                ts: 0,
                at: 0,
                removed: String::new(),
                inserted: String::new(),
            }],
            current: 0,
            text: initial.into(),
            edit_bytes: 0,
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
        &self.text
    }

    /// Record a new document state, branching where necessary.
    ///
    /// Returns `true` if the tree changed (a node was added or `current` moved).
    /// `coalesce_ms` merges a rapid burst of edits into a single leaf node.
    pub fn record(&mut self, text: &str, now_ms: u64, coalesce_ms: u64) -> bool {
        if self.text == text {
            return false;
        }
        let cur = self.current;
        // Undo: the new text is the parent's text.
        if self.nodes[cur].parent.is_some() && self.nodes[cur].reverts_to(&self.text, text) {
            self.text = text.to_string();
            self.current = self.nodes[cur].parent.unwrap_or(0);
            return true;
        }
        // Redo: the new text matches an existing child.
        if let Some(c) = self.nodes[cur]
            .children
            .iter()
            .rev()
            .find(|&&c| self.nodes[c].applies_to(&self.text, text))
            .copied()
        {
            self.text = text.to_string();
            self.current = c;
            self.last_ms = now_ms;
            return true;
        }
        // Coalesce a typing burst into the current leaf.
        let is_leaf = self.nodes[cur].children.is_empty();
        let bursting = now_ms.saturating_sub(self.last_ms) < coalesce_ms;
        let full = self.nodes.len() >= MAX_NODES || self.edit_bytes >= MAX_EDIT_BYTES;
        if cur != 0 && is_leaf && (bursting || full) {
            let mut parent_text = std::mem::take(&mut self.text);
            self.nodes[cur].revert(&mut parent_text);
            let (at, removed, inserted) = splice(&parent_text, text);
            let node = &mut self.nodes[cur];
            self.edit_bytes = self.edit_bytes - node.edit_bytes() + removed.len() + inserted.len();
            node.at = at;
            node.removed = removed;
            node.inserted = inserted;
            node.ts = now_ms;
            self.text = text.to_string();
            self.last_ms = now_ms;
            return true;
        }
        // New branch.
        let id = self.nodes.len();
        let (at, removed, inserted) = splice(&self.text, text);
        self.edit_bytes += removed.len() + inserted.len();
        self.nodes.push(UndoNode {
            id,
            parent: Some(cur),
            children: Vec::new(),
            ts: now_ms,
            at,
            removed,
            inserted,
        });
        self.nodes[cur].children.push(id);
        self.current = id;
        self.text = text.to_string();
        self.last_ms = now_ms;
        true
    }

    /// Move to the parent state.
    pub fn undo(&mut self) -> Option<String> {
        let p = self.nodes[self.current].parent?;
        self.goto(p)
    }

    /// Move to the most-recent child state.
    pub fn redo(&mut self) -> Option<String> {
        let c = *self.nodes[self.current]
            .children
            .iter()
            .max_by_key(|&&c| self.nodes[c].ts)?;
        self.goto(c)
    }

    /// Jump to an arbitrary node.
    pub fn goto(&mut self, id: usize) -> Option<String> {
        self.nodes.get(id)?;
        // Up from the current node to the common ancestor, reverting; then down
        // to `id`, applying.
        let target_path = self.path_from_root(id);
        let mut node = self.current;
        while !target_path.contains(&node) {
            self.nodes[node].revert(&mut self.text);
            node = self.nodes[node].parent?;
        }
        let from = target_path.iter().position(|&n| n == node)? + 1;
        for &n in &target_path[from..] {
            self.nodes[n].apply(&mut self.text);
        }
        self.current = id;
        Some(self.text.clone())
    }

    /// Node ids from the root down to `id`, inclusive.
    fn path_from_root(&self, id: usize) -> Vec<usize> {
        let mut path = vec![id];
        let mut n = id;
        while let Some(p) = self.nodes[n].parent {
            path.push(p);
            n = p;
        }
        path.reverse();
        path
    }

    /// After a reload, point `current` at the node matching `content` (if any).
    pub fn sync_to(&mut self, content: &str) -> bool {
        if self.text == content {
            return true;
        }
        // Walk the whole tree depth-first with one working copy of the text.
        let mut text = self.text.clone();
        for &n in self.path_from_root(self.current)[1..].iter().rev() {
            self.nodes[n].revert(&mut text);
        }
        let mut found = None;
        let mut stack: Vec<(usize, bool)> = vec![(0, false)];
        while let Some((n, leaving)) = stack.pop() {
            if leaving {
                self.nodes[n].revert(&mut text);
                continue;
            }
            if n != 0 {
                self.nodes[n].apply(&mut text);
                stack.push((n, true));
            }
            if text == content {
                found = Some(n);
                break;
            }
            for &c in self.nodes[n].children.iter().rev() {
                stack.push((c, false));
            }
        }
        match found {
            Some(n) => {
                self.text = text;
                self.current = n;
                true
            }
            None => false,
        }
    }

    /// Whether a save is due, throttled to at most once per 1.5s; if so, the
    /// tree is serialised for the caller to write (off the UI thread).
    pub fn json_if_due(&mut self, now_ms: u64) -> Option<String> {
        if now_ms.saturating_sub(self.last_save_ms) < 1500 {
            return None;
        }
        self.last_save_ms = now_ms;
        serde_json::to_string(self).ok()
    }

    pub fn save(&self, path: &Path) {
        if let Ok(json) = serde_json::to_string(self) {
            let _ = crate::buffer::write(path, &json);
        }
    }

    pub fn load(path: &Path) -> Option<Self> {
        let data = std::fs::read_to_string(path).ok()?;
        if let Ok(mut t) = serde_json::from_str::<Self>(&data) {
            t.edit_bytes = t.nodes.iter().map(UndoNode::edit_bytes).sum();
            return t.valid().then_some(t);
        }
        Self::from_legacy(serde_json::from_str(&data).ok()?)
    }

    /// Convert a full-text tree from an older version.
    fn from_legacy(old: LegacyTree) -> Option<Self> {
        let root = old.nodes.first()?;
        let mut t = Self::new(root.text.clone());
        t.nodes.clear();
        for n in &old.nodes {
            let (at, removed, inserted) = match n.parent.and_then(|p| old.nodes.get(p)) {
                Some(p) => splice(&p.text, &n.text),
                None => (0, String::new(), String::new()),
            };
            t.nodes.push(UndoNode {
                id: n.id,
                parent: n.parent,
                children: n.children.clone(),
                ts: n.ts,
                at,
                removed,
                inserted,
            });
        }
        t.current = old.current;
        t.text = old.nodes.get(old.current)?.text.clone();
        t.edit_bytes = t.nodes.iter().map(UndoNode::edit_bytes).sum();
        t.valid().then_some(t)
    }

    /// A tree read from disk must be well-formed before it is walked: the
    /// links must form a tree, and every splice must fit the text it is applied
    /// to — checked by visiting every node once, so later walks can't panic.
    fn valid(&self) -> bool {
        let n = self.nodes.len();
        let linked = n > 0
            && self.current < n
            && self.nodes.iter().enumerate().all(|(i, node)| {
                node.id == i
                    && node.parent.is_none_or(|p| p < i)
                    && (i == 0) == node.parent.is_none()
                    && node
                        .children
                        .iter()
                        .all(|&c| c > i && c < n && self.nodes[c].parent == Some(i))
            });
        if !linked {
            return false;
        }
        let mut text = self.text.clone();
        for &id in self.path_from_root(self.current)[1..].iter().rev() {
            let node = &self.nodes[id];
            if !UndoNode::fits(&text, node.at, &node.inserted) {
                return false;
            }
            node.revert(&mut text);
        }
        let mut stack: Vec<(usize, bool)> = vec![(0, false)];
        while let Some((id, leaving)) = stack.pop() {
            let node = &self.nodes[id];
            if leaving {
                node.revert(&mut text);
                continue;
            }
            if id != 0 {
                if !UndoNode::fits(&text, node.at, &node.removed) {
                    return false;
                }
                node.apply(&mut text);
                stack.push((id, true));
            }
            for &c in node.children.iter().rev() {
                stack.push((c, false));
            }
        }
        true
    }
}

/// Delete stored undo histories not touched in `max_age`, so the store doesn't
/// grow with every file ever opened.
pub fn prune(dir: &Path, max_age: std::time::Duration) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let now = std::time::SystemTime::now();
    for e in entries.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|age| age > max_age);
        if old && e.path().extension().is_some_and(|x| x == "json") {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every node's text, rebuilt by visiting it (and returning to where we were).
    fn texts(t: &UndoTree) -> Vec<String> {
        let mut t = t.clone();
        let back = t.current;
        let out = (0..t.len()).map(|i| t.goto(i).unwrap()).collect();
        t.goto(back);
        out
    }

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
        assert!(texts(&t).iter().any(|s| s == "ab"));
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
        assert_eq!(t.undo().as_deref(), Some(""));
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
        assert_eq!(texts(&loaded), vec!["root", "a", "ab"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn loads_the_full_text_format() {
        let legacy = r#"{"nodes":[
            {"id":0,"parent":null,"children":[1,2],"ts":0,"text":"fn a() {}"},
            {"id":1,"parent":0,"children":[],"ts":1,"text":"fn a() { 1 }"},
            {"id":2,"parent":0,"children":[],"ts":2,"text":"fn b() {}"}
        ],"current":1}"#;
        let dir = std::env::temp_dir().join(format!("e-undo-legacy-{}", std::process::id()));
        let path = dir.join("t.json");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, legacy).unwrap();
        let t = UndoTree::load(&path).expect("converted");
        assert_eq!(t.current_text(), "fn a() { 1 }");
        assert_eq!(texts(&t), vec!["fn a() {}", "fn a() { 1 }", "fn b() {}"]);
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

    #[test]
    fn goto_crosses_branches_and_multibyte_text() {
        let mut t = UndoTree::new("blåbær\n");
        t.record("blåbærsyltetøy\n", 1000, 700); // 1
        t.record("blåbærsyltetøy er godt\n", 3000, 700); // 2
        t.goto(1);
        t.record("blåbærsyltetøy på brød\n", 5000, 700); // 3, sibling of 2
        t.goto(0);
        t.record("æ\n", 7000, 700); // 4, child of root
        assert_eq!(t.goto(2).as_deref(), Some("blåbærsyltetøy er godt\n"));
        assert_eq!(t.goto(4).as_deref(), Some("æ\n"));
        assert_eq!(t.goto(3).as_deref(), Some("blåbærsyltetøy på brød\n"));
        assert_eq!(t.goto(0).as_deref(), Some("blåbær\n"));
    }

    #[test]
    fn sync_to_finds_a_state_on_another_branch() {
        let mut t = UndoTree::new("one\n");
        t.record("one\ntwo\n", 1000, 700);
        t.goto(0);
        t.record("zero\none\n", 3000, 700);
        assert!(t.sync_to("one\ntwo\n"));
        assert_eq!(t.current, 1);
        assert_eq!(t.current_text(), "one\ntwo\n");
        assert!(!t.sync_to("never written"));
        assert_eq!(t.current, 1);
    }

    #[test]
    fn stores_edits_not_copies() {
        let big = "x".repeat(200_000);
        let mut t = UndoTree::new(big.clone());
        for i in 0..100u64 {
            let mut s = t.current_text().to_string();
            s.insert(1000, 'y');
            t.record(&s, i * 10_000, 700);
        }
        assert_eq!(t.len(), 101);
        let stored: usize = t
            .nodes
            .iter()
            .map(|n| n.removed.len() + n.inserted.len())
            .sum();
        assert!(stored < 1000, "stored {stored} bytes of edits");
        assert_eq!(t.goto(0).as_deref(), Some(big.as_str()));
    }

    #[test]
    fn splice_is_minimal_and_char_safe() {
        assert_eq!(splice("abc", "abXc"), (2, "".into(), "X".into()));
        assert_eq!(splice("aaa", "aa"), (2, "a".into(), "".into()));
        // "å" and "ä" share their first UTF-8 byte; the splice must not split it.
        let (at, rem, ins) = splice("blå", "blä");
        assert_eq!((at, rem.as_str(), ins.as_str()), (2, "å", "ä"));
    }

    #[test]
    fn a_corrupt_tree_is_rejected() {
        let dir = std::env::temp_dir().join(format!("e-undo-bad-{}", std::process::id()));
        let path = dir.join("t.json");
        std::fs::create_dir_all(&dir).unwrap();
        let mut t = UndoTree::new("a");
        t.record("ab", 1000, 700);
        let mut v: serde_json::Value = serde_json::to_value(&t).unwrap();
        v["current"] = 7.into();
        std::fs::write(&path, v.to_string()).unwrap();
        assert!(UndoTree::load(&path).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_splice_that_does_not_fit_is_rejected() {
        let dir = std::env::temp_dir().join(format!("e-undo-fit-{}", std::process::id()));
        let path = dir.join("t.json");
        std::fs::create_dir_all(&dir).unwrap();
        let mut t = UndoTree::new("blå");
        t.record("blå bær", 1000, 700);
        let mut v: serde_json::Value = serde_json::to_value(&t).unwrap();
        // Inside the two bytes of "å": would panic in replace_range.
        v["nodes"][1]["at"] = 3.into();
        std::fs::write(&path, v.to_string()).unwrap();
        assert!(UndoTree::load(&path).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
