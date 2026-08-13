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
use std::time::Duration;

use e_verify::{RequestMetrics, RouteEvidence};

/// How long to let the interpreter notice that the files changed, before
/// measuring against them.
///
/// PHP's opcache is on by default and only re-stats sources every
/// `opcache.revalidate_freq` seconds — 2 in a stock build. Stashing the change
/// and replaying immediately therefore measures the *previous* bytecode: the
/// baseline comes back identical to the after-measurement, which reads as "this
/// change did nothing" and is entirely an artefact. Measured on a real Laravel
/// app: 4 queries immediately after stashing, 29 three seconds later.
///
/// This is the price of measuring a running interpreter rather than a pure
/// function, and there is no signal to wait on — so we wait out the window.
pub const SETTLE: Duration = Duration::from_millis(2_500);

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
pub fn measure_before_and_after<M>(root: &Path, plan: &[Planned], measure: M) -> Measured
where
    M: FnMut(&str) -> (RequestMetrics, bool),
{
    measure_with_settle(root, plan, measure, SETTLE)
}

/// [`measure_before_and_after`] with the settle delay injected, so tests do not
/// pay it.
pub fn measure_with_settle<M>(
    root: &Path,
    plan: &[Planned],
    mut measure: M,
    settle: Duration,
) -> Measured
where
    M: FnMut(&str) -> (RequestMetrics, bool),
{
    let mut queries_visible = vec![true; plan.len()];
    let measure_all = |m: &mut M, vis: &mut Vec<bool>| -> Vec<Option<RequestMetrics>> {
        plan.iter()
            .enumerate()
            .map(|(i, p)| {
                p.skip.is_none().then(|| {
                    let (metrics, visible) = m(&p.uri);
                    // Either pass being blind makes the pair unusable.
                    vis[i] &= visible;
                    metrics
                })
            })
            .collect()
    };

    // 1. With the change. The user may have edited seconds ago, so the
    //    interpreter needs the same grace here as after the stash.
    std::thread::sleep(settle);
    let after = measure_all(&mut measure, &mut queries_visible);

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
            std::thread::sleep(settle);
            before = measure_all(&mut measure, &mut queries_visible);
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
            queries_visible: queries_visible[i],
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
    fn measure_from_tree(root: &Path) -> impl FnMut(&str) -> (RequestMetrics, bool) + '_ {
        move |_uri| {
            let body = std::fs::read_to_string(root.join("app.php")).unwrap_or_default();
            (metrics(body.trim().parse().unwrap_or(0)), true)
        }
    }

    #[test]
    fn the_baseline_really_is_the_pre_change_code() {
        // Committed state says 40 queries; the uncommitted change says 4.
        let root = repo("40\n");
        std::fs::write(root.join("app.php"), "4\n").unwrap();

        let out = measure_with_settle(
            &root,
            &plan(&["orders"]),
            measure_from_tree(&root),
            Duration::ZERO,
        );

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
        let out = measure_with_settle(
            &root,
            &plan(&["orders"]),
            measure_from_tree(&root),
            Duration::ZERO,
        );

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
        let out = measure_with_settle(
            &root,
            &plan(&["orders"]),
            measure_from_tree(&root),
            Duration::ZERO,
        );
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

        let out = measure_with_settle(
            &dir,
            &plan(&["orders"]),
            measure_from_tree(&dir),
            Duration::ZERO,
        );
        assert!(matches!(out.tree, TreeOutcome::StashFailed(_)));
        assert!(out.rows[0].metrics.is_some());
        assert!(out.rows[0].baseline.is_none());
        assert!(out.tree.warning().unwrap().contains("current state only"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn one_blind_pass_makes_the_pair_blind() {
        // If only one of the two measurements could see queries, the pair cannot
        // be compared on queries at all — claiming otherwise would compare a
        // real count against a phantom zero.
        let root = repo("40\n");
        std::fs::write(root.join("app.php"), "4\n").unwrap();
        let mut call = 0;
        let out = measure_with_settle(
            &root,
            &plan(&["orders"]),
            |_| {
                call += 1;
                (metrics(call), call == 1) // visible on "after", blind on "before"
            },
            Duration::ZERO,
        );
        assert!(!out.rows[0].queries_visible);
        let _ = std::fs::remove_dir_all(&root);
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
        let out = measure_with_settle(
            &root,
            &plan,
            |uri| {
                hit.push(uri.to_string());
                (metrics(1), true)
            },
            Duration::ZERO,
        );
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
        measure_with_settle(
            &root,
            &plan(&["orders"]),
            |uri| {
                let (m, visible) = read(uri);
                seen.push(m.query_count);
                (m, visible)
            },
            Duration::ZERO,
        );
        assert_eq!(seen, [4, 40], "after (changed) first, then before");
        let _ = std::fs::remove_dir_all(&root);
    }
}

/// End-to-end validation against a real, running Laravel application.
///
/// Opt-in, because it needs a server on the other end. It exercises the actual
/// pipeline — git diff → attribution → stash → replay → evidence markdown —
/// rather than a stand-in for it, which is the only way to find the things unit
/// tests cannot see (a project without Clockwork, a route that needs auth, a
/// replay that times out).
///
/// ```sh
/// E_LIVE_ROOT=/path/to/app E_LIVE_BASE=http://127.0.0.1:8391 \
///   cargo test -p e-app live_project -- --ignored --nocapture
/// ```
#[cfg(test)]
mod live_project {
    use super::*;

    fn routes_of(root: &Path) -> Vec<e_review::routes::Route> {
        let out = std::process::Command::new("php")
            .args(["artisan", "route:list", "--json"])
            .current_dir(root)
            .output()
            .expect("php artisan route:list");
        let text = String::from_utf8_lossy(&out.stdout);
        let json: serde_json::Value =
            serde_json::from_str(text.trim_start_matches(|c| c != '[')).expect("route json");
        json.as_array()
            .unwrap()
            .iter()
            .map(|r| {
                let g = |k: &str| r.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
                e_review::routes::Route {
                    name: g("name"),
                    uri: g("uri"),
                    methods: g("method"),
                    action: g("action"),
                }
            })
            .collect()
    }

    #[test]
    #[ignore]
    fn evidence_against_a_running_app() {
        let Ok(root) = std::env::var("E_LIVE_ROOT") else {
            eprintln!("set E_LIVE_ROOT and E_LIVE_BASE to run this");
            return;
        };
        let base = std::env::var("E_LIVE_BASE").expect("E_LIVE_BASE");
        let root = std::path::PathBuf::from(root);

        let diff = e_core::git::diff_since(&root, "HEAD").expect("git diff");
        let cs = e_review::changeset_from_diff(&diff);
        println!(
            "changed files: {:?}",
            cs.files.iter().map(|f| &f.path).collect::<Vec<_>>()
        );

        let attribution = e_review::routes::attribute(&cs.files, &routes_of(&root));
        for a in &attribution.affected {
            println!("attributed {} — {}", a.route.uri, a.reason.describe());
        }
        println!("unattributed: {:?}", attribution.unattributed);
        assert!(!attribution.affected.is_empty(), "nothing attributed");

        let plan: Vec<Planned> = attribution
            .affected
            .iter()
            .map(|a| Planned {
                label: format!(
                    "{} /{}",
                    a.route.methods.split('|').next().unwrap_or("GET"),
                    a.route.uri.trim_start_matches('/')
                ),
                skip: (!a.route.is_safe_to_replay() || a.route.uri.contains('{'))
                    .then(|| "not replayed".to_string()),
                uri: a.route.uri.clone(),
            })
            .collect();

        // The real entry point, settle delay and all — the point is to find out
        // what the editor actually does.
        let measured = measure_before_and_after(&root, &plan, |uri| {
            let url = crate::verify::replay_url(&base, uri);
            let (status, ms, queries, visible) = crate::state::replay_for_verify(&base, &url);
            (
                e_verify::metrics_of(&crate::verify::sample_from_replay(status, ms, &queries)),
                visible,
            )
        });

        println!("\ntree: {:?}", measured.tree);
        println!(
            "\n{}",
            e_verify::evidence_markdown(&measured.rows, attribution.unattributed.len(), &[])
        );
        assert_eq!(
            measured.tree,
            TreeOutcome::Restored,
            "the tree must be restored"
        );
    }
}
