//! Turn receipts: what one turn of the agent changed, shown at the end of the
//! turn in Cascade, with **Review** (the session review, narrowed to the turn)
//! and **Undo** (the turn's files put back, unless changed again since).
//!
//! Before a prompt reaches an idle agent the working tree is captured as a
//! tree object ([`e_core::git::snapshot_tree`], which touches neither the
//! index nor HEAD), and the prompt goes out once it is taken — so nothing the
//! agent writes can land before it. When the agent finishes, the tree is
//! captured again and the difference becomes the receipt.

use std::path::Path;

use e_agent::{ChatItem, TurnReceipt};
use e_review::flags::{self, Severity};
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};

use crate::state::AppState;

/// `E_DEBUG_CASCADE=1`: say what the turn snapshots and undos do.
fn debug(what: &str) {
    if std::env::var_os("E_DEBUG_CASCADE").is_some() {
        eprintln!("e: turn: {what}");
    }
}

/// What a turn changed, from its two snapshots: `None` when it changed
/// nothing. Blocking.
fn receipt(
    root: &Path,
    before: String,
    rules: &e_review::rules::Rules,
    project: &Path,
) -> Option<TurnReceipt> {
    let after = e_core::git::snapshot_tree(root).ok()?;
    if after == before {
        return None;
    }
    let changes = e_core::git::tree_changes(root, &before, &after).ok()?;
    if changes.is_empty() {
        return None;
    }
    let flags = e_core::git::tree_diff(root, &before, &after)
        .map(|diff| {
            let cs = e_review::changeset_from_diff(&diff);
            let example = std::fs::read_to_string(root.join(".env.example")).ok();
            flags::scan_changeset(&cs)
                .iter()
                .chain(&flags::env_example_flags(&cs, example.as_deref()))
                .chain(&crate::review::rule_flags(rules, &cs, root, project))
                .filter(|f| f.severity >= Severity::Warn)
                .count()
        })
        .unwrap_or(0);
    Some(TurnReceipt {
        before,
        after,
        files: changes
            .into_iter()
            .map(|c| (c.path, c.added, c.removed))
            .collect(),
        flags,
        undone: None,
    })
}

/// What an undo did, in a line.
fn undo_said(restored: usize, skipped: &[String]) -> String {
    let mut out = match restored {
        0 => "Nothing undone".to_string(),
        1 => "Undone: 1 file put back".to_string(),
        n => format!("Undone: {n} files put back"),
    };
    if !skipped.is_empty() {
        out.push_str(&format!(
            " · left alone, changed since: {}",
            skipped.join(", ")
        ));
    }
    out
}

impl AppState {
    /// Capture the working tree before a prompt to an idle agent, then run
    /// `send`. Without a git repository there is nothing to capture, and it
    /// is sent at once.
    pub(crate) fn turn_begin(&self, send: impl FnOnce() + 'static) {
        let Some(root) = self.agent_repo_root() else {
            send();
            return;
        };
        let before = self.agent.turn_before;
        self.spawn_bg(
            move || e_core::git::snapshot_tree(&root).ok(),
            move |tree: Option<String>| {
                debug(&format!("before {tree:?}"));
                before.set(tree);
                send();
            },
        );
    }

    /// The agent finished: add what the turn changed to the transcript.
    pub(crate) fn turn_end(&self) {
        let Some(before) = self.agent.turn_before.get_untracked() else {
            return;
        };
        self.agent.turn_before.set(None);
        let Some(root) = self.agent_repo_root() else {
            return;
        };
        let chat = self.agent.chat;
        let rules = self.project_rules.get_untracked();
        let project = self.agent_project_root();
        self.spawn_bg(
            move || receipt(&root, before, &rules, &project),
            move |r: Option<TurnReceipt>| {
                debug(&format!(
                    "receipt {:?}",
                    r.as_ref().map(|r| (&r.after, r.files.len()))
                ));
                if let Some(r) = r {
                    chat.update(|c| c.push_receipt(r));
                }
            },
        );
    }

    fn receipt_at(&self, index: usize) -> Option<TurnReceipt> {
        self.agent
            .chat
            .with_untracked(|c| match c.items.get(index) {
                Some(ChatItem::Receipt(r)) => Some(r.clone()),
                _ => None,
            })
    }

    /// Open the session review on just what the turn at `index` changed.
    pub fn turn_review(&self, index: usize) {
        let Some(r) = self.receipt_at(index) else {
            return;
        };
        self.review_turn.set(Some((r.before, r.after)));
        self.review_open.set(true);
        self.refresh_review();
    }

    /// Put the files the turn at `index` changed back as they were before it
    /// — each only if it is still as the turn left it.
    pub fn turn_undo(&self, index: usize) {
        debug(&format!("undo #{index}"));
        let Some(r) = self.receipt_at(index) else {
            return;
        };
        if r.undone.is_some() {
            return;
        }
        let Some(root) = self.agent_repo_root() else {
            return;
        };
        // A file open with unsaved edits is the user's now, whatever is on
        // disk: leave it.
        let dirty: Vec<String> = self.buffers.with_untracked(|bs| {
            bs.iter()
                .filter(|b| b.dirty.get_untracked())
                .filter_map(|b| b.file.path.as_ref()?.strip_prefix(&root).ok())
                .map(|p| p.to_string_lossy().into_owned())
                .collect()
        });
        let paths: Vec<String> = r
            .files
            .iter()
            .map(|f| f.0.clone())
            .filter(|p| !dirty.contains(p))
            .collect();
        let held: Vec<String> = r
            .files
            .iter()
            .map(|f| f.0.clone())
            .filter(|p| dirty.contains(p))
            .collect();
        let app = *self;
        self.spawn_bg(
            move || e_core::git::undo_tree_change(&root, &r.before, &r.after, &paths),
            move |res: Result<e_core::git::UndoReport, String>| match res {
                Ok(report) => {
                    debug(&format!("undid {report:?}"));
                    let mut skipped = held;
                    skipped.extend(report.skipped);
                    let said = undo_said(report.restored.len(), &skipped);
                    app.agent
                        .chat
                        .update(|c| c.mark_undone(index, said.clone()));
                    // Open files that were put back show it.
                    app.fs_rev.update(|r| *r += 1);
                    app.check_external_changes();
                    Self::notify(&said);
                }
                Err(e) => Self::notify(&format!("Couldn't undo the turn: {e}")),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_undo_says_what_it_put_back_and_what_it_left() {
        assert_eq!(undo_said(2, &[]), "Undone: 2 files put back");
        assert_eq!(
            undo_said(1, &["app/Models/Order.php".into()]),
            "Undone: 1 file put back · left alone, changed since: app/Models/Order.php"
        );
    }
}
