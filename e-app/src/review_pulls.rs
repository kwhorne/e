//! Review a pull request the way a session is reviewed: its diff from GitHub,
//! risk-ranked and flagged by the same rules — a colleague's, Dependabot's, a
//! cloud agent's — without checking it out. Its files, for the checks that
//! read whole files, come from its commit, fetched into the repository.

use e_core::github::{self, PullRequest};
use e_review::changeset_from_diff;
use e_review::flags::{self, Flag};
use floem::ext_event::create_ext_action;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};

use crate::state::AppState;

/// What reviewing a pull request found: its diff, the flags that read its
/// files, and a word on what couldn't be checked.
type PullResult = Result<(String, Vec<Flag>, Option<String>), String>;

impl AppState {
    /// The palette's "Review: Pull Requests": the review panel, listing them.
    pub fn review_pick_pull(&self) {
        self.review.open.set(true);
        self.review.picking.set(true);
        self.review_load_pulls();
    }

    /// Ask GitHub for the open pull requests, in the background.
    pub fn review_load_pulls(&self) {
        let Some(root) = self.agent_repo_root() else {
            self.review
                .pulls_status
                .set("Pull requests need a git repository".into());
            return;
        };
        self.review.pulls_status.set("Asking GitHub…".into());
        let pulls = self.review.pulls;
        let status = self.review.pulls_status;
        let send = create_ext_action(
            self.cx,
            move |res: Result<Vec<PullRequest>, String>| match res {
                Ok(list) => {
                    status.set(if list.is_empty() {
                        "No open pull requests".into()
                    } else {
                        String::new()
                    });
                    pulls.set(list);
                }
                Err(e) => status.set(e),
            },
        );
        std::thread::spawn(move || send(github::list_pulls(&root)));
    }

    /// Review pull request `pr`.
    pub fn review_open_pull(&self, pr: PullRequest) {
        self.review.turn.set(None);
        self.review.picking.set(false);
        self.review.pr.set(Some(pr));
        self.review.selected.set(None);
        // Its sign-offs aren't the session's.
        self.review.changeset.set(e_review::Changeset::default());
        self.review.flags.set(Vec::new());
        self.refresh_review();
    }

    /// From a pull request back to what the session changed.
    pub fn review_back_to_session(&self) {
        self.review.pr.set(None);
        self.review.pr_note.set(None);
        self.review.picking.set(false);
        self.review.changeset.set(e_review::Changeset::default());
        self.review.flags.set(Vec::new());
        self.refresh_review();
    }

    /// Re-read the pull request under review: its diff, and the flags.
    pub(crate) fn refresh_pull_review(&self, pr: PullRequest) {
        let Some(root) = self.agent_repo_root() else {
            return;
        };
        self.review.busy.set(true);
        let cs_sig = self.review.changeset;
        let busy = self.review.busy;
        let selected = self.review.selected;
        let flags_sig = self.review.flags;
        let note = self.review.pr_note;
        let routes = self
            .project
            .laravel
            .get_untracked()
            .map(|d| d.all_routes.clone())
            .unwrap_or_default();
        let project = self.root.get_untracked();
        let rules = self.project.rules.get_untracked();
        let rules_root = self.agent_project_root();
        let send = create_ext_action(self.cx, move |res: PullResult| {
            busy.set(false);
            match res {
                Ok((diff, more, why)) => {
                    let mut fresh = changeset_from_diff(&diff);
                    cs_sig.with_untracked(|old| fresh.carry_reviewed_from(old));
                    let mut all = flags::scan_changeset(&fresh);
                    all.extend(more);
                    all.sort_by(|a, b| {
                        b.severity
                            .cmp(&a.severity)
                            .then(a.path.cmp(&b.path))
                            .then(a.line.cmp(&b.line))
                    });
                    flags_sig.set(all);
                    let keep = selected
                        .get_untracked()
                        .filter(|p| fresh.get(p).is_some())
                        .or_else(|| fresh.files.first().map(|f| f.path.clone()));
                    selected.set(keep);
                    cs_sig.set(fresh);
                    note.set(why);
                }
                Err(e) => {
                    note.set(Some(format!("Couldn't read #{}: {e}", pr.number)));
                    cs_sig.set(e_review::Changeset::default());
                    flags_sig.set(Vec::new());
                }
            }
        });
        std::thread::spawn(move || {
            send(github::pull_diff(&root, pr.number).map(|diff| {
                let cs = changeset_from_diff(&diff);
                // `.env.example` as the pull request leaves it, when it
                // changes it; else as it is here.
                let mut more = Vec::new();
                let why = match github::fetch_pull(&root, &pr) {
                    Ok(head) => {
                        let read = |p: &str| github::file_at(&root, &head, p);
                        more.extend(crate::review::unchecked_lookup_flags(
                            &project, &cs, &routes, read,
                        ));
                        more.extend(crate::review::rule_flags_reading(
                            &rules,
                            &cs,
                            &root,
                            &rules_root,
                            read,
                        ));
                        let example = github::file_at(&root, &head, ".env.example");
                        more.extend(flags::env_example_flags(&cs, example.as_deref()));
                        None
                    }
                    Err(e) => Some(format!(
                        "The checks that read whole files were skipped — its commits couldn't be fetched: {e}"
                    )),
                };
                (diff, more, why)
            }))
        });
    }

    /// Open the pull request under review on GitHub.
    pub fn review_pull_on_github(&self) {
        if let Some(pr) = self.review.pr.get_untracked() {
            crate::platform::open_url(&pr.url);
        }
    }

    /// Check the pull request under review out here (`gh pr checkout`),
    /// when the working tree is clean.
    pub fn review_checkout_pull(&self) {
        let Some(pr) = self.review.pr.get_untracked() else {
            return;
        };
        let Some(root) = self.agent_repo_root() else {
            return;
        };
        let state = *self;
        let send = create_ext_action(self.cx, move |res: Result<(), String>| match res {
            Ok(()) => {
                Self::notify(&format!("Checked out #{} ({})", pr.number, pr.head));
                state.refresh_git_status();
            }
            Err(e) => Self::notify(&format!("Couldn't check out #{}: {e}", pr.number)),
        });
        std::thread::spawn(move || send(github::checkout_pull(&root, pr.number)));
    }
}

/// A pull request's line in the list: number, title, author, branch, and
/// its checks and review in a glyph.
pub fn pull_line(pr: &PullRequest) -> (String, String) {
    let checks = match pr.checks.as_str() {
        "pass" => "✓ checks",
        "fail" => "✗ checks",
        "pending" => "● checks",
        _ => "",
    };
    let review = match pr.review.as_str() {
        "APPROVED" => "approved",
        "CHANGES_REQUESTED" => "changes requested",
        _ => "",
    };
    let draft = if pr.draft { "draft · " } else { "" };
    let mut detail = format!("{draft}{} · {} → {}", pr.author, pr.head, pr.base);
    for part in [checks, review] {
        if !part.is_empty() {
            detail.push_str(" · ");
            detail.push_str(part);
        }
    }
    (format!("#{}  {}", pr.number, pr.title), detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_says_who_where_and_how_it_stands() {
        let pr = PullRequest {
            number: 73,
            title: "e --wait".into(),
            author: "kwhorne".into(),
            head: "editor-wait".into(),
            base: "main".into(),
            checks: "fail".into(),
            review: "APPROVED".into(),
            draft: true,
            ..Default::default()
        };
        let (title, detail) = pull_line(&pr);
        assert_eq!(title, "#73  e --wait");
        assert_eq!(
            detail,
            "draft · kwhorne · editor-wait → main · ✗ checks · approved"
        );
    }
}
