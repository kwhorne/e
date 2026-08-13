//! Agent **session review**: after an agent changes N files, review the whole
//! changeset locally — risk-ranked, hunk-by-hunk, revertible — instead of
//! pushing and reviewing on GitHub.
//!
//! The diff model and risk ranking are the pure [`e_review`] crate; this module
//! is the `AppState` orchestration (running git, reverting a file, wiring the
//! panel). The panel lives in [`crate::review_view`].

use std::path::PathBuf;

use e_review::flags::{self, Flag};
use e_review::ship::{ship_verdict, ShipCheck, ShipVerdict, TestStatus};
use e_review::{changeset_from_diff, commits, ChangeKind};
use floem::ext_event::create_ext_action;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};

use crate::state::{AppState, TddStatus};

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
        let flags_sig = self.review_flags;
        let send = create_ext_action(self.cx, move |text: Result<String, String>| {
            busy.set(false);
            match text {
                Ok(diff) => {
                    let mut fresh = changeset_from_diff(&diff);
                    cs_sig.with_untracked(|old| fresh.carry_reviewed_from(old));
                    flags_sig.set(flags::scan_changeset(&fresh));
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

    // ---- Automated flags (phase 3) --------------------------------------

    /// Flags for one file, most severe first.
    pub fn review_flags_for(&self, path: &str) -> Vec<Flag> {
        self.review_flags
            .with(|all| all.iter().filter(|f| f.path == path).cloned().collect())
    }

    /// Ask the agent about one specific finding.
    pub fn review_ask_flag(&self, f: &Flag) {
        self.send_to_agent(&format!(
            "In `{}` around line {}, a review check flagged: {} ({}). \
             Explain whether this is intentional, and fix it if not.",
            f.path, f.line, f.message, f.code
        ));
    }

    // ---- Ship gate (phase 4) --------------------------------------------

    /// Map the TDD panel's state onto the review gate's test status.
    fn review_test_status(&self) -> TestStatus {
        match self.tdd_status.get() {
            TddStatus::Passed => TestStatus::Passing,
            TddStatus::Failed => TestStatus::Failing,
            TddStatus::Running => TestStatus::Running,
            TddStatus::Idle => TestStatus::Unknown,
        }
    }

    /// The current readiness verdict (reactive — for the panel).
    pub fn review_ship_verdict(&self) -> ShipVerdict {
        let (danger, warn, _) = self.review_flags.with(|f| flags::counts(f));
        ship_verdict(&ShipCheck {
            reviewed: self.review_changeset.with(|cs| cs.progress()),
            danger_flags: danger,
            warn_flags: warn,
            tests: self.review_test_status(),
        })
    }

    // ---- Evidence: which routes this changeset touches, and what they cost --

    /// The project's route table, reduced to what attribution needs.
    fn review_route_table(&self) -> Vec<e_review::routes::Route> {
        self.laravel
            .get_untracked()
            .map(|d| {
                d.routes
                    .iter()
                    .map(|r| e_review::routes::Route {
                        name: r.name.clone(),
                        uri: r.uri.clone(),
                        methods: r.methods.clone(),
                        action: r.action.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Which routes the current changeset reaches, and why.
    pub fn review_attribution(&self) -> e_review::routes::Attribution {
        let routes = self.review_route_table();
        self.review_changeset
            .with_untracked(|cs| e_review::routes::attribute(&cs.files, &routes))
    }

    /// Replay every affected route, with and without the change, and record
    /// what each one cost. This is what turns the pull request from a
    /// description of the change into a measurement of it.
    ///
    /// The change is already in the working tree, so "after" is measured first;
    /// the tree is then stashed to expose the pre-change code, "before" is
    /// measured, and the stash is popped. PHP is re-read per request, so no
    /// rebuild is needed between the two.
    ///
    /// Write routes are listed but never replayed — firing a PATCH at the app to
    /// see how fast it is would change data.
    pub fn review_measure_evidence(&self) {
        if self.review_evidence_busy.get_untracked() {
            return;
        }
        let attribution = self.review_attribution();
        if attribution.affected.is_empty() {
            self.review_evidence.set(Some(Vec::new()));
            return;
        }
        let base = self.app_base();
        let root = self.root.get_untracked();
        let plan: Vec<crate::evidence::Planned> = attribution
            .affected
            .iter()
            .map(|a| {
                let method = a.route.methods.split('|').next().unwrap_or("GET").trim();
                crate::evidence::Planned {
                    label: format!("{method} /{}", a.route.uri.trim_start_matches('/')),
                    skip: if !a.route.is_safe_to_replay() {
                        Some("not replayed: would write".to_string())
                    } else if a.route.uri.contains('{') {
                        // Guessing a parameter would measure a 404 and call it evidence.
                        Some("not replayed: needs parameters".to_string())
                    } else {
                        None
                    },
                    uri: a.route.uri.clone(),
                }
            })
            .collect();

        self.review_evidence_busy.set(true);
        let busy = self.review_evidence_busy;
        let out = self.review_evidence;
        let send = create_ext_action(self.cx, move |m: crate::evidence::Measured| {
            busy.set(false);
            out.set(Some(m.rows));
            if let Some(w) = m.tree.warning() {
                AppState::notify(&w);
                eprintln!("e: {w}");
            }
        });

        std::thread::spawn(move || {
            let measured = crate::evidence::measure_before_and_after(&root, &plan, |uri| {
                let url = crate::verify::replay_url(&base, uri);
                let (status, ms, queries) = crate::state::replay_for_verify(&base, &url);
                e_verify::metrics_of(&crate::verify::sample_from_replay(status, ms, &queries))
            });
            send(measured);
        });
    }

    /// Reasons the measurement should be read with care.
    ///
    /// Stashing moves the *code* out of the way; it does nothing to the
    /// database. So when a changeset carries a migration, the baseline runs the
    /// old code against the new schema — which produces numbers that look
    /// perfectly plausible and are not a fair comparison. Silence there would be
    /// worse than no evidence at all.
    fn review_evidence_caveats(&self) -> Vec<String> {
        let mut out = Vec::new();
        let migrations = self.review_changeset.with_untracked(|cs| {
            cs.files
                .iter()
                .filter(|f| f.path.contains("database/migrations/"))
                .count()
        });
        if migrations > 0 {
            out.push(format!(
                "This changeset includes {migrations} migration(s). Stashing restores the \
                 code but not the database, so the baseline ran the old code against the \
                 migrated schema — treat the before column with suspicion."
            ));
        }
        out
    }

    /// The "## Evidence" block for the pull request, if anything was measured.
    fn review_evidence_markdown(&self) -> Option<String> {
        let rows = self.review_evidence.get_untracked()?;
        let unattributed = self.review_attribution().unattributed.len();
        let md = e_verify::evidence_markdown(&rows, unattributed, &self.review_evidence_caveats());
        (!md.is_empty()).then_some(md)
    }

    // ---- Ship it (phase 5) ----------------------------------------------

    /// Create a branch, commit the changeset in logical groups, push, and open a
    /// PR — all from the review panel.
    pub fn review_commit_and_pr(&self, open_pr: bool) {
        if self.review_shipping.get_untracked() {
            return;
        }
        let Some(root) = self.repo_root_path() else {
            return;
        };
        let cs = self.review_changeset.get_untracked();
        if cs.is_empty() {
            Self::notify("Nothing to commit");
            return;
        }
        let groups = commits::plan_commits(&cs);
        let branch = commits::suggest_branch(&cs);
        let title = commits::pr_title(&cs);
        let (danger, warn, _) = self.review_flags.with_untracked(|f| flags::counts(f));
        let verdict = self.review_ship_verdict();
        let tests = self.review_test_status();
        let summary = self.review_summary.get_untracked();
        let evidence = self.review_evidence_markdown();
        let body = commits::pr_body(
            &cs,
            &verdict,
            tests,
            danger,
            warn,
            summary.as_deref(),
            evidence.as_deref(),
        );

        self.review_shipping.set(true);
        let state = *self;
        let shipping = self.review_shipping;
        let send = create_ext_action(self.cx, move |res: Result<String, String>| {
            shipping.set(false);
            match res {
                Ok(url) if !url.is_empty() => {
                    Self::notify(&format!("Pull request opened: {url}"));
                    let _ = std::process::Command::new("open").arg(&url).spawn();
                }
                Ok(_) => Self::notify("Committed and pushed"),
                Err(e) => Self::notify(&format!("Ship failed: {e}")),
            }
            state.refresh_git_status();
            state.refresh_review();
        });

        std::thread::spawn(move || {
            let res = (|| -> Result<String, String> {
                // Start from a clean index so each group commits only its files.
                e_core::git::unstage_all(&root)?;
                e_core::git::checkout_new(&root, &branch)?;
                for g in &groups {
                    e_core::git::commit_paths(&root, &g.paths, &g.message)?;
                }
                e_core::git::push_new_branch(&root, &branch)?;
                if open_pr {
                    e_core::git::create_pr(&root, &title, &body)
                } else {
                    Ok(String::new())
                }
            })();
            send(res);
        });
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
             risky or unintended. Finally send the summary back to the editor with \
             {{\"method\":\"review_summary\",\"text\":\"...\"}} on $E_EDITOR_SOCK so it \
             becomes the pull-request description."
        ));
    }
}
