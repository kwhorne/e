//! Agent **session review**: after an agent changes N files, review the whole
//! changeset locally — risk-ranked, hunk-by-hunk, revertible — instead of
//! pushing and reviewing on GitHub.
//!
//! The diff model and risk ranking are the pure [`e_review`] crate; this module
//! is the `AppState` orchestration (running git, reverting a file, wiring the
//! panel). The panel lives in [`crate::review_view`].

use std::path::PathBuf;

use e_review::{changeset_from_diff, ChangeKind};
use floem::ext_event::create_ext_action;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};

use crate::state::AppState;

impl AppState {
    /// Remember where the current agent session started, so the review shows
    /// exactly what the session changed. Called when an agent is started.
    pub fn mark_review_session(&self) {
        let Some(root) = self.repo_root_path() else {
            return;
        };
        if let Ok(cp) = e_core::git::checkpoint(&root) {
            self.review_base.set(Some(cp.head));
        }
    }

    /// The repository root for the open project, if it is a git repo.
    pub(crate) fn repo_root_path(&self) -> Option<PathBuf> {
        e_core::git::repo_root(&self.root.get_untracked())
    }

    pub fn toggle_review(&self) {
        let open = !self.review_open.get_untracked();
        self.review_open.set(open);
        if open {
            self.refresh_review();
        }
    }

    /// Re-read the session diff and rebuild the changeset (keeping sign-offs).
    pub fn refresh_review(&self) {
        if self.review_busy.get_untracked() {
            return;
        }
        let Some(root) = self.repo_root_path() else {
            Self::notify("Session review needs a git repository");
            return;
        };
        // Without a session checkpoint, review everything uncommitted — which is
        // also what you want when the agent ran outside the editor.
        let base = self
            .review_base
            .get_untracked()
            .unwrap_or_else(|| "HEAD".to_string());

        self.review_busy.set(true);
        let cs_sig = self.review_changeset;
        let busy = self.review_busy;
        let selected = self.review_selected;
        let send = create_ext_action(self.cx, move |text: Result<String, String>| {
            busy.set(false);
            match text {
                Ok(diff) => {
                    let mut fresh = changeset_from_diff(&diff);
                    cs_sig.with_untracked(|old| fresh.carry_reviewed_from(old));
                    // Keep the selection if that file is still in the changeset.
                    let keep = selected
                        .get_untracked()
                        .filter(|p| fresh.get(p).is_some())
                        .or_else(|| fresh.files.first().map(|f| f.path.clone()));
                    selected.set(keep);
                    cs_sig.set(fresh);
                }
                Err(e) => Self::notify(&format!("Review failed: {e}")),
            }
        });
        std::thread::spawn(move || send(e_core::git::diff_since(&root, &base)));
    }

    pub fn review_select(&self, path: String) {
        self.review_selected.set(Some(path));
    }

    /// Sign a file off (or un-sign it).
    pub fn review_mark(&self, path: &str, reviewed: bool) {
        self.review_changeset
            .update(|cs| cs.mark_reviewed(path, reviewed));
    }

    /// Mark the selected file reviewed and jump to the next one needing review.
    pub fn review_mark_and_next(&self) {
        let Some(path) = self.review_selected.get_untracked() else {
            return;
        };
        self.review_mark(&path, true);
        let next = self
            .review_changeset
            .with_untracked(|cs| cs.next_unreviewed(Some(&path)).map(|f| f.path.clone()));
        if let Some(next) = next {
            self.review_selected.set(Some(next));
        }
    }

    /// Open the selected/〈path〉 file in the editor at the first changed line.
    pub fn review_open_file(&self, path: &str) {
        let Some(root) = self.repo_root_path() else {
            return;
        };
        let line = self.review_changeset.with_untracked(|cs| {
            cs.get(path)
                .and_then(|f| f.hunks.first().map(|h| h.new_start))
                .unwrap_or(1)
        });
        let abs = root.join(path);
        if abs.is_file() {
            self.jump_to(&format!("file://{}", abs.display()), line.max(1), 1);
        }
    }

    /// Undo one file from the session: delete it if the session created it,
    /// otherwise restore it from `HEAD`.
    pub fn review_revert_file(&self, path: &str) {
        let Some(root) = self.repo_root_path() else {
            return;
        };
        let kind = self
            .review_changeset
            .with_untracked(|cs| cs.get(path).map(|f| f.kind));
        let res = match kind {
            Some(ChangeKind::Added) => {
                std::fs::remove_file(root.join(path)).map_err(|e| e.to_string())
            }
            _ => e_core::git::discard(&root, path),
        };
        match res {
            Ok(()) => {
                self.review_changeset.update(|cs| cs.remove(path));
                self.check_external_changes();
                self.refresh_git_status();
                Self::notify(&format!("Reverted {path}"));
                self.refresh_review();
            }
            Err(e) => Self::notify(&format!("Revert failed: {e}")),
        }
    }

    /// Ask the agent to explain its own change to this file.
    pub fn review_ask_agent(&self, path: &str) {
        self.send_to_agent(&format!(
            "You changed `{path}` in this session. Explain what you changed and why, \
             and flag anything risky about it."
        ));
    }

    /// Ask the agent for a review of the whole session changeset.
    pub fn review_ask_summary(&self) {
        let (summary, paths) = self.review_changeset.with_untracked(|cs| {
            (
                cs.summary(),
                cs.files
                    .iter()
                    .map(|f| f.path.clone())
                    .collect::<Vec<_>>()
                    .join(", "),
            )
        });
        if paths.is_empty() {
            Self::notify("Nothing to review");
            return;
        }
        self.send_to_agent(&format!(
            "Review the changes in this session ({summary}). Files: {paths}. \
             Summarize what changed and why, grouped logically, then flag anything \
             risky or unintended."
        ));
    }
}
