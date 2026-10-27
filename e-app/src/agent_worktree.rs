//! A Cascade session in a worktree of its own: the agent works on a branch
//! beside your checkout — with its own `.env`, its own copy of the database
//! and its own site — so migrations, test runs and half-done edits stay off
//! the files you are working in and the database you are using.
//!
//! Grove does the heavy lifting (`grove try`, see [`crate::grove::try_new`]).
//! Here: starting a session in one, the line in Cascade that says where the
//! agent is working, and the shelf of earlier ones in the ⋯ menu — each to go
//! back to, or take down once its branch is merged.

use std::path::PathBuf;

use floem::reactive::{SignalGet, SignalUpdate};

use crate::grove::{self, Try};
use crate::state::AppState;

/// `e/agent-1027-1130`: a branch name for a new session, from the time.
fn branch_name() -> String {
    // `YYYY_MM_DD_HHMMSS` → `MMDD-HHMMSS`.
    let ts = crate::codegen::migration_timestamp();
    let digits: String = ts.chars().filter(char::is_ascii_digit).collect();
    format!("e/agent-{}-{}", &digits[4..8], &digits[8..])
}

/// Whether `branch` is merged into the checkout's HEAD.
pub fn merged(root: &std::path::Path, branch: &str) -> bool {
    std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["branch", "--merged", "HEAD", "--format=%(refname:short)"])
        .output()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .any(|l| l.trim() == branch)
        })
        .unwrap_or(false)
}

impl AppState {
    /// Where the agent works: its worktree, or the project.
    pub(crate) fn agent_root(&self) -> PathBuf {
        self.agent
            .worktree
            .get_untracked()
            .map(|t| t.path)
            .unwrap_or_else(|| self.root.get_untracked())
    }

    /// The git repository the agent works in — what its turns and the session
    /// review are measured against.
    pub(crate) fn agent_repo_root(&self) -> Option<PathBuf> {
        match self.agent.worktree.get_untracked() {
            Some(t) => Some(t.path),
            None => self.repo_root_path(),
        }
    }

    /// A line in the Cascade transcript — where someone looking at Cascade
    /// sees it, which a system notification may not be.
    fn cascade_note(&self, text: String, error: bool) {
        self.agent
            .chat
            .update(|c| c.items.push(e_agent::ChatItem::Notice { text, error }));
    }

    /// Start a new Cascade session on a new branch, in a worktree of its own.
    pub fn cascade_new_worktree(&self) {
        let root = self.root.get_untracked();
        let branch = branch_name();
        self.cascade_note(
            format!(
                "Making {branch} beside your checkout: its own worktree, database copy and site…"
            ),
            false,
        );
        let app = *self;
        self.spawn_bg(
            move || {
                // Asked here rather than from the Runtime panel's lookup,
                // which may not have run yet.
                if grove::site_for(&root).is_none() {
                    return Err(
                        "it needs the project served by Grove (grove link), which gives the worktree its own site and database"
                            .to_string(),
                    );
                }
                grove::try_new(&root, &branch)
            },
            move |res: Result<Try, String>| match res {
                Ok(t) => {
                    let ready = format!("Working in {} — its site is {}", t.branch, t.url);
                    app.cascade_enter_worktree(Some(t));
                    app.cascade_note(ready, false);
                }
                Err(e) => app.cascade_note(format!("Couldn't make the worktree: {e}"), true),
            },
        );
    }

    /// Run the agent in `t` (or, with `None`, back in the project), in a new
    /// session there.
    pub fn cascade_enter_worktree(&self, t: Option<Try>) {
        if let Some(client) = self.agent.native_client.get_untracked() {
            client.shutdown();
        }
        self.agent.native_client.set(None);
        self.agent.turn_before.set(None);
        self.review_turn.set(None);
        self.agent.worktree.set(t);
        self.start_native_agent();
        self.agent.focus_pulse.update(|x| *x += 1);
    }

    /// Take a worktree down — site, database, worktree; the branch stays.
    /// Grove refuses while it holds uncommitted work.
    pub fn cascade_take_down(&self, t: Try) {
        if self
            .agent
            .worktree
            .get_untracked()
            .is_some_and(|w| w.path == t.path)
        {
            self.cascade_enter_worktree(None);
        }
        let root = self.root.get_untracked();
        let branch = t.branch.clone();
        let app = *self;
        self.spawn_bg(
            move || grove::try_done(&root, &branch, false),
            move |res: Result<(), String>| match res {
                Ok(()) => app.cascade_note(
                    format!(
                        "{} is taken down: its site, database and worktree. The branch is kept",
                        t.branch
                    ),
                    false,
                ),
                Err(e) => app.cascade_note(format!("{} is still up: {e}", t.branch), true),
            },
        );
    }

    /// After each turn: if the session's worktree is gone — taken down while
    /// the agent worked in it, by `grove try --done` from anywhere — say so,
    /// and stop showing it as where the agent works. The agent can't run
    /// anything there any more; a new session starts somewhere that exists.
    pub(crate) fn worktree_check(&self) {
        let Some(t) = self.agent.worktree.get_untracked() else {
            return;
        };
        if t.path.exists() {
            return;
        }
        self.agent.worktree.set(None);
        self.cascade_note(
            format!(
                "{}'s worktree is gone, so Elyra can't work there any more. Start a new session (⋯ → New session) to go on in the project",
                t.branch
            ),
            true,
        );
    }

    /// Open the worktree's code in a new `e` window.
    pub fn cascade_open_worktree(&self) {
        let Some(t) = self.agent.worktree.get_untracked() else {
            return;
        };
        match crate::platform::current_exe() {
            Ok(exe) => {
                let _ = std::process::Command::new(exe).arg(&t.path).spawn();
            }
            Err(e) => Self::notify(&format!("Couldn't open a new window: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_session_gets_a_branch_named_for_the_time() {
        let b = branch_name();
        assert!(b.starts_with("e/agent-"), "{b}");
        let rest = &b["e/agent-".len()..];
        let (date, time) = rest.split_once('-').unwrap();
        assert_eq!(date.len(), 4);
        assert_eq!(time.len(), 6);
        assert!(date.chars().chain(time.chars()).all(|c| c.is_ascii_digit()));
    }
}
