//! "Verify the fix" loop.
//!
//! Given a request captured in the Runtime panel, this:
//!   1. takes a git checkpoint of the working tree (so any change is reversible),
//!   2. replays the request and records a **baseline** measurement,
//!   3. waits while you (or the agent) apply a fix,
//!   4. replays again and **compares** before → after, and
//!   5. lets you **Keep** the fix or **Discard** it (restoring the checkpoint).
//!
//! The measurement mapping and the session state machine in this module are
//! pure and unit-tested. The side effects (HTTP replay, git checkpoint/restore)
//! live in the `impl AppState` block, which is exercised through the GUI.

use e_core::git::Checkpoint;
use e_verify::{compare, metrics_of, Comparison, Query, RequestMetrics, RequestSample, Verdict};
use floem::ext_event::create_ext_action;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};

use crate::state::AppState;

/// Build a sample from a fresh replay: `status`, wall-clock `duration_ms`, and
/// the Clockwork query list `(sql, duration_str)`.
pub fn sample_from_replay(
    status: u16,
    duration_ms: f64,
    queries: &[(String, String)],
) -> RequestSample {
    RequestSample {
        status,
        duration_ms,
        queries: queries.iter().map(query_of).collect(),
        // Shape comparison is intentionally left off: dynamic bodies (CSRF
        // tokens, timestamps) would otherwise be flagged as "broke". We rely on
        // the status code to catch a response that regressed into an error.
        response_shape: None,
    }
}

fn query_of((sql, dur): &(String, String)) -> Query {
    Query {
        sql: sql.clone(),
        duration_ms: dur.trim().parse().unwrap_or(0.0),
    }
}

/// Join a base URL and a captured URI into a replayable absolute URL.
pub fn replay_url(base: &str, uri: &str) -> String {
    if uri.starts_with("http://") || uri.starts_with("https://") {
        uri.to_string()
    } else if let Some(rest) = uri.strip_prefix('/') {
        format!("{}/{}", base.trim_end_matches('/'), rest)
    } else {
        format!("{}/{}", base.trim_end_matches('/'), uri)
    }
}

/// Where a verify session is in its lifecycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerifyPhase {
    /// Baseline captured; waiting for a fix to be applied and re-measured.
    AwaitingFix,
    /// After-measurement taken; `comparison` is populated.
    Done,
}

/// One in-progress verification of a single request.
#[derive(Clone)]
pub struct VerifySession {
    pub method: String,
    pub uri: String,
    /// Fully-qualified replay URL.
    pub url: String,
    /// The safety checkpoint to restore on Discard (None if git wasn't available).
    pub checkpoint: Option<Checkpoint>,
    pub before: RequestMetrics,
    pub after: Option<RequestMetrics>,
    pub comparison: Option<Comparison>,
    pub phase: VerifyPhase,
}

impl VerifySession {
    pub fn new(
        method: String,
        uri: String,
        url: String,
        checkpoint: Option<Checkpoint>,
        before: RequestSample,
    ) -> Self {
        Self {
            method,
            uri,
            url,
            checkpoint,
            before: metrics_of(&before),
            after: None,
            comparison: None,
            phase: VerifyPhase::AwaitingFix,
        }
    }

    /// Record the after-fix measurement and compute the verdict.
    pub fn record_after(&mut self, after: RequestSample) {
        let after = metrics_of(&after);
        self.comparison = Some(compare(&self.before, &after));
        self.after = Some(after);
        self.phase = VerifyPhase::Done;
    }
}

/// Human label for a verdict.
pub fn verdict_label(v: Verdict) -> &'static str {
    match v {
        Verdict::Improved => "Improved",
        Verdict::NoChange => "No change",
        Verdict::Regressed => "Regressed",
        Verdict::Broke => "Broke",
    }
}

/// One-line summary of a comparison for the panel.
pub fn summary(c: &Comparison) -> String {
    let mut parts = Vec::new();
    if c.query_delta != 0 {
        parts.push(format!(
            "{} quer{} ({}{})",
            c.queries_after,
            if c.queries_after == 1 { "y" } else { "ies" },
            if c.query_delta > 0 { "+" } else { "" },
            c.query_delta
        ));
    } else {
        parts.push(format!("{} queries", c.queries_after));
    }
    parts.push(format!("{:.0}ms → {:.0}ms", c.ms_before, c.ms_after));
    if c.n1_fixed {
        parts.push("N+1 removed".to_string());
    } else if c.n1_after {
        parts.push("N+1 present".to_string());
    }
    if c.status_changed {
        parts.push("status changed".to_string());
    }
    parts.join(" · ")
}

impl AppState {
    /// Start verifying a captured request: checkpoint the working tree, replay
    /// the request, and record the baseline.
    pub fn verify_begin(&self, id: &str) {
        if self.verify_busy.get_untracked() {
            return;
        }
        let req = self
            .runtime_reqs
            .with_untracked(|list| list.iter().find(|r| r.id == id).cloned());
        let Some(req) = req else {
            return;
        };
        let base = self.app_base();
        let url = replay_url(&base, &req.uri);
        let root = self.root.get_untracked();
        let (method, uri, url_for_session) = (req.method.clone(), req.uri.clone(), url.clone());

        self.verify_busy.set(true);
        self.verify_open.set(true);
        let session_sig = self.verify_session;
        let busy_sig = self.verify_busy;
        let send = create_ext_action(self.cx, move |res: (Option<Checkpoint>, RequestSample)| {
            busy_sig.set(false);
            let (cp, before) = res;
            session_sig.set(Some(VerifySession::new(
                method.clone(),
                uri.clone(),
                url_for_session.clone(),
                cp,
                before,
            )));
        });
        std::thread::spawn(move || {
            let cp = e_core::git::checkpoint(&root).ok();
            let (status, ms, queries) = crate::state::replay_for_verify(&base, &url);
            send((cp, sample_from_replay(status, ms, &queries)));
        });
    }

    /// Re-measure the current session's request (after a fix was applied).
    pub fn verify_measure(&self) {
        if self.verify_busy.get_untracked() {
            return;
        }
        let Some(url) = self
            .verify_session
            .with_untracked(|s| s.as_ref().map(|s| s.url.clone()))
        else {
            return;
        };
        let base = self.app_base();
        self.verify_busy.set(true);
        let session_sig = self.verify_session;
        let busy_sig = self.verify_busy;
        let send = create_ext_action(self.cx, move |sample: RequestSample| {
            busy_sig.set(false);
            session_sig.update(|s| {
                if let Some(s) = s.as_mut() {
                    s.record_after(sample);
                }
            });
        });
        std::thread::spawn(move || {
            let (status, ms, queries) = crate::state::replay_for_verify(&base, &url);
            send(sample_from_replay(status, ms, &queries));
        });
    }

    /// Keep the applied fix and close the panel (the checkpoint is dropped).
    pub fn verify_keep(&self) {
        self.verify_session.set(None);
        self.verify_busy.set(false);
        self.verify_open.set(false);
        Self::notify("Fix kept");
    }

    /// Revert to the checkpoint taken at `verify_begin` and close the panel.
    pub fn verify_discard(&self) {
        let cp = self
            .verify_session
            .with_untracked(|s| s.as_ref().and_then(|s| s.checkpoint.clone()));
        let root = self.root.get_untracked();
        self.verify_session.set(None);
        self.verify_busy.set(false);
        self.verify_open.set(false);
        let Some(cp) = cp else {
            Self::notify("Nothing to revert (no checkpoint)");
            return;
        };
        match e_core::git::restore_checkpoint(&root, &cp) {
            Ok(()) => {
                // Refresh any open buffers whose file was reset on disk.
                self.check_external_changes();
                Self::notify("Reverted to checkpoint");
            }
            Err(e) => Self::notify(&format!("Revert failed: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_sample_parses_query_durations() {
        let q = vec![("select 1".to_string(), "3.5".to_string())];
        let s = sample_from_replay(200, 88.0, &q);
        assert_eq!(s.queries[0].duration_ms, 3.5);
        assert_eq!(s.duration_ms, 88.0);
    }

    #[test]
    fn builds_replay_urls() {
        assert_eq!(
            replay_url("https://app.test", "/orders"),
            "https://app.test/orders"
        );
        assert_eq!(
            replay_url("https://app.test/", "/orders"),
            "https://app.test/orders"
        );
        assert_eq!(
            replay_url("https://app.test", "orders"),
            "https://app.test/orders"
        );
        assert_eq!(
            replay_url("https://app.test", "https://other/x"),
            "https://other/x"
        );
    }

    #[test]
    fn session_detects_fixed_n_plus_one() {
        // Baseline: one parent query + an N+1 of the same child shape.
        let child = "select * from orders where user_id = 1";
        let mut before_q = vec!["select * from users".to_string()];
        for i in 0..5 {
            before_q.push(child.replace('1', &i.to_string()));
        }
        let before = sample_from_replay(
            200,
            120.0,
            &before_q
                .iter()
                .map(|q| (q.clone(), "2".to_string()))
                .collect::<Vec<_>>(),
        );

        let mut sess = VerifySession::new(
            "GET".into(),
            "/orders".into(),
            "https://app.test/orders".into(),
            None,
            before,
        );
        assert_eq!(sess.phase, VerifyPhase::AwaitingFix);
        assert!(sess.before.has_n_plus_one());

        // After the fix: two eager-loaded queries, faster, no repeats.
        let after = sample_from_replay(
            200,
            40.0,
            &[
                ("select * from users".to_string(), "2".to_string()),
                (
                    "select * from orders where user_id in (?)".to_string(),
                    "3".to_string(),
                ),
            ],
        );
        sess.record_after(after);

        assert_eq!(sess.phase, VerifyPhase::Done);
        let c = sess.comparison.as_ref().unwrap();
        assert_eq!(c.verdict, Verdict::Improved);
        assert!(c.n1_fixed);
        assert!(c.faster);
        assert!(c.query_delta < 0);
    }

    #[test]
    fn summary_mentions_query_delta_and_timing() {
        let q = |n: usize| vec![("a".to_string(), "1".to_string()); n];
        let before = sample_from_replay(200, 100.0, &q(6));
        let after = sample_from_replay(200, 50.0, &q(2));
        let mut sess = VerifySession::new("GET".into(), "/x".into(), "u".into(), None, before);
        sess.record_after(after);
        let s = summary(sess.comparison.as_ref().unwrap());
        assert!(s.contains("→"));
        assert!(s.contains("-4"));
    }
}
