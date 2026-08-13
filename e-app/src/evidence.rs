//! The before/after measurement loop behind the ship gate's Evidence section.
//!
//! Split out of `review.rs` so it can be tested. What makes it worth testing is
//! not the arithmetic — that lives in `e-verify` — but the sequencing and the
//! stash handling around it: this function moves the user's uncommitted work out
//! of the way and back, and getting that wrong loses their day.
//!
//! `measure` is injected, so a test can drive the whole thing against a real git
//! repository without a running application.

use std::path::Path;

use e_verify::{RequestMetrics, RouteEvidence};

/// One route in a measurement plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    /// `GET /orders`, as a reviewer would recognise it.
    pub label: String,
    /// The path to replay.
    pub uri: String,
    /// `Some(reason)` when this route must not be replayed at all.
    pub skip: Option<String>,
}

/// What happened to the working tree during measurement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeOutcome {
    /// Stashed and restored: both halves were measured.
    Restored,
    /// There was nothing to stash, so there is no "before" to compare against.
    NothingToStash,
    /// The stash could not be taken; only the current state was measured.
    StashFailed(String),
    /// The change was stashed but could **not** be restored. The user's work is
    /// in `git stash` and they have to be told.
    NotRestored(String),
}

impl TreeOutcome {
    /// The message to put in front of the user, if any.
    pub fn warning(&self) -> Option<String> {
        match self {
            TreeOutcome::Restored => None,
            TreeOutcome::NothingToStash => Some(
                "measured the current state only: there was nothing to stash, \
                 so there is no before to compare against"
                    .into(),
            ),
            TreeOutcome::StashFailed(e) => Some(format!(
                "measured the current state only: could not stash to get a baseline ({e})"
            )),
            TreeOutcome::NotRestored(e) => Some(format!(
                "could not restore your changes after measuring ({e}) — they are in \
                 `git stash`; run `git stash pop`"
            )),
        }
    }
}

/// The result of a measurement run.
#[derive(Debug, Clone, PartialEq)]
pub struct Measured {
    pub rows: Vec<RouteEvidence>,
    pub tree: TreeOutcome,
}

/// Measure every planned route with the change applied and again without it.
///
/// The change is already in the working tree, so "after" is measured first, then
/// the tree is stashed to expose the pre-change code for "before". Ordering it
/// the other way round would mean restoring before the first measurement, which
/// leaves a longer window where the user's work is not in their tree.
pub fn measure_before_and_after<M>(root: &Path, plan: &[Planned], mut measure: M) -> Measured
where
    M: FnMut(&str) -> RequestMetrics,
{
    let measure_all = |m: &mut M| -> Vec<Option<RequestMetrics>> {
        plan.iter()
            .map(|p| p.skip.is_none().then(|| m(&p.uri)))
            .collect()
    };

    // 1. With the change.
    let after = measure_all(&mut measure);

    // 2. Without it.
    let mut before: Vec<Option<RequestMetrics>> = vec![None; plan.len()];
    let stashed_before = e_core::git::stash_count(root);
    let tree = match e_core::git::stash_push(root) {
        Err(e) => TreeOutcome::StashFailed(e),
        Ok(()) if e_core::git::stash_count(root) == stashed_before => {
            // `git stash push` succeeds and does nothing when the tree is clean.
            // Popping now would take someone *else's* stash entry, or fail with
            // "no stash entries" and look like we had lost the user's work.
            TreeOutcome::NothingToStash
        }
        Ok(()) => {
            before = measure_all(&mut measure);
            // 3. Put the change back. The step that must never fail quietly.
            match e_core::git::stash_pop(root) {
                Ok(()) => TreeOutcome::Restored,
                Err(e) => TreeOutcome::NotRestored(e),
            }
        }
    };

    let rows = plan
        .iter()
        .enumerate()
        .map(|(i, p)| RouteEvidence {
            label: p.label.clone(),
            metrics: after[i].clone(),
            baseline: before[i].clone(),
            note: p.skip.clone(),
        })
        .collect();

    Measured { rows, tree }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static N: AtomicU32 = AtomicU32::new(0);

    fn git(root: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .expect("git");
        assert!(out.status.success(), "git {args:?}: {out:?}");
    }

    /// A real repository with one committed file. Real, because the whole point
    /// is to find out what `git stash` actually does here.
    fn repo(contents: &str) -> std::path::PathBuf {
        let n = N.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("e-ev-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "--quiet"]);
        git(&dir, &["config", "user.email", "t@example.com"]);
        git(&dir, &["config", "user.name", "t"]);
        std::fs::write(dir.join("app.php"), contents).unwrap();
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "--quiet", "-m", "base"]);
        dir
    }

    fn metrics(queries: usize) -> RequestMetrics {
        e_verify::metrics_of(&e_verify::RequestSample {
            status: 200,
            duration_ms: queries as f64 * 10.0,
            queries: (0..queries)
                .map(|_| e_verify::Query {
                    sql: "select * from items where id = 1".into(),
                    duration_ms: 1.0,
                })
                .collect(),
            response_shape: None,
        })
    }

    fn plan(uris: &[&str]) -> Vec<Planned> {
        uris.iter()
            .map(|u| Planned {
                label: format!("GET /{u}"),
                uri: u.to_string(),
                skip: None,
            })
            .collect()
    }

    /// The measurement reads the working tree, so before/after genuinely differ
    /// by what the stash exposed — exactly as a replay against the running app
    /// would.
    fn measure_from_tree(root: &Path) -> impl FnMut(&str) -> RequestMetrics + '_ {
        move |_uri| {
            let body = std::fs::read_to_string(root.join("app.php")).unwrap_or_default();
            metrics(body.trim().parse().unwrap_or(0))
        }
    }

    #[test]
    fn the_baseline_really_is_the_pre_change_code() {
        // Committed state says 40 queries; the uncommitted change says 4.
        let root = repo("40\n");
        std::fs::write(root.join("app.php"), "4\n").unwrap();

        let out = measure_before_and_after(&root, &plan(&["orders"]), measure_from_tree(&root));

        assert_eq!(out.tree, TreeOutcome::Restored);
        assert_eq!(out.rows[0].baseline.as_ref().unwrap().query_count, 40);
        assert_eq!(out.rows[0].metrics.as_ref().unwrap().query_count, 4);
        assert_eq!(
            out.rows[0].comparison().unwrap().verdict,
            e_verify::Verdict::Improved
        );

        // And the user's change is back where they left it.
        assert_eq!(
            std::fs::read_to_string(root.join("app.php")).unwrap(),
            "4\n"
        );
        assert_eq!(e_core::git::stash_count(&root), 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_clean_tree_is_not_mistaken_for_lost_work() {
        // `git stash push` succeeds and stashes nothing when there is nothing to
        // stash. Popping then fails, which used to look exactly like "we lost
        // your changes".
        let root = repo("7\n");
        let out = measure_before_and_after(&root, &plan(&["orders"]), measure_from_tree(&root));

        assert_eq!(out.tree, TreeOutcome::NothingToStash);
        let warning = out.tree.warning().unwrap();
        assert!(warning.contains("nothing to stash"), "{warning}");
        assert!(
            !warning.contains("git stash pop"),
            "must not tell the user to recover work that was never stashed: {warning}"
        );
        assert!(out.rows[0].metrics.is_some());
        assert!(out.rows[0].baseline.is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_unrelated_stash_entry_is_left_alone() {
        // Someone else's stash must not be popped into the user's tree.
        let root = repo("40\n");
        std::fs::write(root.join("app.php"), "99\n").unwrap();
        git(&root, &["stash", "push", "-u", "-m", "someone else's"]);
        assert_eq!(e_core::git::stash_count(&root), 1);

        // Tree is clean again; measuring must not touch that entry.
        let out = measure_before_and_after(&root, &plan(&["orders"]), measure_from_tree(&root));
        assert_eq!(out.tree, TreeOutcome::NothingToStash);
        assert_eq!(e_core::git::stash_count(&root), 1, "entry must survive");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn outside_a_repository_it_measures_the_current_state_and_says_so() {
        let dir = std::env::temp_dir().join(format!("e-ev-nogit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("app.php"), "5\n").unwrap();

        let out = measure_before_and_after(&dir, &plan(&["orders"]), measure_from_tree(&dir));
        assert!(matches!(out.tree, TreeOutcome::StashFailed(_)));
        assert!(out.rows[0].metrics.is_some());
        assert!(out.rows[0].baseline.is_none());
        assert!(out.tree.warning().unwrap().contains("current state only"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn skipped_routes_are_never_replayed() {
        let root = repo("40\n");
        std::fs::write(root.join("app.php"), "4\n").unwrap();
        let plan = vec![
            Planned {
                label: "GET /orders".into(),
                uri: "orders".into(),
                skip: None,
            },
            Planned {
                label: "PATCH /orders/{order}".into(),
                uri: "orders/{order}".into(),
                skip: Some("not replayed: would write".into()),
            },
        ];
        let mut hit: Vec<String> = Vec::new();
        let out = measure_before_and_after(&root, &plan, |uri| {
            hit.push(uri.to_string());
            metrics(1)
        });
        assert_eq!(hit, ["orders", "orders"], "once after, once before");
        assert!(out.rows[1].metrics.is_none());
        assert_eq!(
            out.rows[1].note.as_deref(),
            Some("not replayed: would write")
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn after_is_measured_before_the_tree_is_disturbed() {
        // If this ever flipped, the first measurement would run against stashed
        // code and every number would be wrong in a plausible-looking way.
        let root = repo("40\n");
        std::fs::write(root.join("app.php"), "4\n").unwrap();
        let mut seen: Vec<usize> = Vec::new();
        let mut read = measure_from_tree(&root);
        measure_before_and_after(&root, &plan(&["orders"]), |uri| {
            let m = read(uri);
            seen.push(m.query_count);
            m
        });
        assert_eq!(seen, [4, 40], "after (changed) first, then before");
        let _ = std::fs::remove_dir_all(&root);
    }
}
