//! Turning a reviewed changeset into something shippable: logical commit groups,
//! a branch name, and a PR title/body. Pure — the git/`gh` calls live in the app.

use crate::ship::{Readiness, ShipVerdict, TestStatus};
use crate::Changeset;

/// A set of files that belong in one commit.
#[derive(Debug, Clone, PartialEq)]
pub struct CommitGroup {
    /// Conventional-commit prefix, e.g. `feat(db)`.
    pub prefix: &'static str,
    /// The full suggested subject line, e.g. `feat(db): update database/migrations`.
    pub message: String,
    pub paths: Vec<String>,
}

/// Conventional prefix and commit order for a risk reason. Lower order commits
/// first, so dependencies and schema land before the code that uses them.
fn bucket(reason: &str) -> (&'static str, u8) {
    match reason {
        "dependencies" | "lockfile" => ("chore(deps)", 0),
        "migration" => ("feat(db)", 1),
        "environment" | "config" => ("chore(config)", 2),
        "routes" => ("feat(routes)", 3),
        "auth" => ("feat(auth)", 4),
        "test" => ("test", 6),
        "docs" => ("docs", 7),
        "CI" => ("ci", 8),
        _ => ("feat", 5),
    }
}

fn dir_of(path: &str) -> Option<&str> {
    path.rfind('/').map(|i| &path[..i])
}

fn file_stem(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap_or(path);
    match name.rfind('.') {
        Some(i) if i > 0 => &name[..i],
        _ => name,
    }
}

/// The deepest directory shared by every path, if any.
fn common_dir(paths: &[String]) -> Option<String> {
    let mut iter = paths.iter();
    let mut common: Vec<&str> = dir_of(iter.next()?)?.split('/').collect();
    for p in iter {
        let parts: Vec<&str> = dir_of(p)?.split('/').collect();
        let keep = common
            .iter()
            .zip(parts.iter())
            .take_while(|(a, b)| a == b)
            .count();
        common.truncate(keep);
        if common.is_empty() {
            return None;
        }
    }
    if common.is_empty() {
        None
    } else {
        Some(common.join("/"))
    }
}

fn what_changed(paths: &[String]) -> String {
    match paths.len() {
        0 => "no files".to_string(),
        1 => format!("update {}", file_stem(&paths[0])),
        n => match common_dir(paths) {
            Some(d) => format!("update {d}"),
            None => format!("update {n} files"),
        },
    }
}

/// Group a changeset into logical commits, in the order they should be committed.
pub fn plan_commits(cs: &Changeset) -> Vec<CommitGroup> {
    let mut buckets: Vec<(u8, &'static str, Vec<String>)> = Vec::new();
    for f in &cs.files {
        let (prefix, order) = bucket(f.risk_reason);
        match buckets
            .iter_mut()
            .find(|(o, p, _)| *o == order && *p == prefix)
        {
            Some((_, _, paths)) => paths.push(f.path.clone()),
            None => buckets.push((order, prefix, vec![f.path.clone()])),
        }
    }
    buckets.sort_by_key(|(order, prefix, _)| (*order, *prefix));
    buckets
        .into_iter()
        .map(|(_, prefix, mut paths)| {
            paths.sort();
            let message = format!("{prefix}: {}", what_changed(&paths));
            CommitGroup {
                prefix,
                message,
                paths,
            }
        })
        .collect()
}

fn slug(s: &str) -> String {
    let mut out = String::new();
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let capped: String = out.trim_matches('-').chars().take(32).collect();
    capped.trim_matches('-').to_string()
}

/// A branch name for the session, derived from whichever group touches the most
/// files (deterministic, so re-running gives the same name).
pub fn suggest_branch(cs: &Changeset) -> String {
    let groups = plan_commits(cs);
    let Some(dominant) = groups
        .iter()
        .max_by_key(|g| (g.paths.len(), std::cmp::Reverse(g.prefix)))
    else {
        return "agent/session".to_string();
    };
    let base = match common_dir(&dominant.paths) {
        Some(d) => d,
        None => dominant
            .paths
            .first()
            .map(|p| file_stem(p).to_string())
            .unwrap_or_else(|| "session".to_string()),
    };
    let s = slug(&base);
    if s.is_empty() {
        "agent/session".to_string()
    } else {
        format!("agent/{s}")
    }
}

/// A PR title: the single group's subject, or a summary across groups.
pub fn pr_title(cs: &Changeset) -> String {
    let groups = plan_commits(cs);
    match groups.len() {
        0 => "No changes".to_string(),
        1 => groups[0].message.clone(),
        _ => {
            let n = cs.len();
            let prefix = groups
                .iter()
                .max_by_key(|g| g.paths.len())
                .map(|g| g.prefix)
                .unwrap_or("chore");
            format!("{prefix}: session changes ({n} files)")
        }
    }
}

fn tests_line(t: TestStatus) -> &'static str {
    match t {
        TestStatus::Passing => "passing",
        TestStatus::Failing => "failing",
        TestStatus::Running => "still running",
        TestStatus::Unknown => "not run",
    }
}

/// A PR body: what changed, grouped, plus the review evidence.
pub fn pr_body(
    cs: &Changeset,
    verdict: &ShipVerdict,
    tests: TestStatus,
    danger: usize,
    warn: usize,
    summary: Option<&str>,
    // A pre-rendered "## Evidence" block from `e_verify::evidence_markdown`.
    // Taken as text so this crate stays independent of the measurement crate;
    // each half is tested where it lives.
    evidence: Option<&str>,
) -> String {
    let mut s = String::from("## Summary\n\n");
    match summary {
        Some(t) if !t.trim().is_empty() => {
            s.push_str(t.trim());
            s.push('\n');
        }
        _ => {
            s.push_str(&cs.summary());
            s.push('\n');
        }
    }

    s.push_str("\n## Changes\n\n");
    for g in plan_commits(cs) {
        s.push_str(&format!("### {}\n\n", g.message));
        for p in &g.paths {
            s.push_str(&format!("- `{p}`\n"));
        }
        s.push('\n');
    }

    if let Some(e) = evidence {
        let e = e.trim();
        if !e.is_empty() {
            s.push_str(e);
            s.push_str("\n\n");
        }
    }

    let (done, total) = cs.progress();
    s.push_str("## Review\n\n");
    s.push_str(&format!("- {done}/{total} files reviewed\n"));
    s.push_str(&format!("- Tests: {}\n", tests_line(tests)));
    s.push_str(&format!("- Flags: {danger} danger, {warn} warning(s)\n"));
    let state = match verdict.readiness {
        Readiness::Ready => "ready",
        Readiness::Warn => "shippable with notes",
        Readiness::Blocked => "needs attention",
    };
    s.push_str(&format!("- Verdict: {state}\n"));
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::changeset_from_diff;
    use crate::ship::{ship_verdict, ShipCheck};

    fn diff_of(paths: &[&str]) -> String {
        let mut d = String::new();
        for p in paths {
            d.push_str(&format!(
                "diff --git a/{p} b/{p}\nindex 1..2 100644\n--- a/{p}\n+++ b/{p}\n@@ -1,1 +1,2 @@\n+x\n"
            ));
        }
        d
    }

    #[test]
    fn groups_by_type_in_logical_order() {
        let cs = changeset_from_diff(&diff_of(&[
            "app/Models/User.php",
            "tests/Feature/UserTest.php",
            "database/migrations/2026_x.php",
            "composer.json",
        ]));
        let groups = plan_commits(&cs);
        let prefixes: Vec<&str> = groups.iter().map(|g| g.prefix).collect();
        // deps → db → source → test
        assert_eq!(prefixes, vec!["chore(deps)", "feat(db)", "feat", "test"]);
    }

    #[test]
    fn single_file_group_names_the_file() {
        let cs = changeset_from_diff(&diff_of(&["app/Models/User.php"]));
        let groups = plan_commits(&cs);
        assert_eq!(groups[0].message, "feat: update User");
    }

    #[test]
    fn multi_file_group_uses_common_dir() {
        let cs = changeset_from_diff(&diff_of(&["app/Models/User.php", "app/Models/Post.php"]));
        let groups = plan_commits(&cs);
        assert_eq!(groups[0].message, "feat: update app/Models");
        assert_eq!(groups[0].paths.len(), 2);
    }

    #[test]
    fn branch_name_from_dominant_group() {
        let cs = changeset_from_diff(&diff_of(&[
            "app/Models/User.php",
            "app/Models/Post.php",
            "README.md",
        ]));
        assert_eq!(suggest_branch(&cs), "agent/app-models");
    }

    #[test]
    fn branch_name_is_stable_and_slugged() {
        let cs = changeset_from_diff(&diff_of(&["packages/server-laravel/src/GridEngine.php"]));
        let b = suggest_branch(&cs);
        assert!(b.starts_with("agent/"), "{b}");
        assert!(!b.contains('/') || b.matches('/').count() == 1, "{b}");
        assert_eq!(b, suggest_branch(&cs));
    }

    #[test]
    fn pr_title_single_vs_multi() {
        let one = changeset_from_diff(&diff_of(&["docs/readme.md"]));
        assert_eq!(pr_title(&one), "docs: update readme");

        let many = changeset_from_diff(&diff_of(&["app/A.php", "database/migrations/m.php"]));
        let t = pr_title(&many);
        assert!(t.contains("session changes (2 files)"), "{t}");
    }

    #[test]
    fn pr_body_has_summary_changes_and_review_sections() {
        let mut cs = changeset_from_diff(&diff_of(&[
            "app/Models/User.php",
            "tests/Feature/UserTest.php",
        ]));
        let p = cs.files[0].path.clone();
        cs.mark_reviewed(&p, true);
        let verdict = ship_verdict(&ShipCheck {
            reviewed: cs.progress(),
            danger_flags: 0,
            warn_flags: 1,
            tests: TestStatus::Passing,
        });
        let body = pr_body(
            &cs,
            &verdict,
            TestStatus::Passing,
            0,
            1,
            Some("Renamed email."),
            None,
        );

        assert!(body.starts_with("## Summary\n\nRenamed email."), "{body}");
        assert!(body.contains("## Changes"));
        assert!(body.contains("- `app/Models/User.php`"));
        assert!(body.contains("## Review"));
        assert!(body.contains("- 1/2 files reviewed"));
        assert!(body.contains("- Tests: passing"));
        assert!(body.contains("shippable with notes"));
    }

    #[test]
    fn pr_body_carries_evidence_between_changes_and_review() {
        let cs = changeset_from_diff(&diff_of(&["app/Http/Controllers/OrderController.php"]));
        let verdict = ship_verdict(&ShipCheck {
            reviewed: cs.progress(),
            danger_flags: 0,
            warn_flags: 0,
            tests: TestStatus::Passing,
        });
        let ev = "## Evidence\n\n| Route | Status |\n| --- | --- |\n| `GET /orders` | 200 |\n";
        let body = pr_body(&cs, &verdict, TestStatus::Passing, 0, 0, None, Some(ev));
        let changes = body.find("## Changes").unwrap();
        let evidence = body.find("## Evidence").unwrap();
        let review = body.find("## Review").unwrap();
        assert!(changes < evidence && evidence < review, "{body}");
        assert!(body.contains("`GET /orders`"));

        // Without it the body is the same minus that section.
        let plain = pr_body(&cs, &verdict, TestStatus::Passing, 0, 0, None, None);
        assert!(!plain.contains("## Evidence"));
        assert_eq!(plain, body.replace(&format!("{ev}\n"), ""));
    }

    #[test]
    fn an_empty_evidence_block_adds_no_heading() {
        let cs = changeset_from_diff(&diff_of(&["a.rs"]));
        let verdict = ship_verdict(&ShipCheck {
            reviewed: cs.progress(),
            danger_flags: 0,
            warn_flags: 0,
            tests: TestStatus::Unknown,
        });
        let body = pr_body(&cs, &verdict, TestStatus::Unknown, 0, 0, None, Some(""));
        assert!(!body.contains("## Evidence"));
    }

    #[test]
    fn pr_body_falls_back_to_auto_summary() {
        let cs = changeset_from_diff(&diff_of(&["app/A.php"]));
        let verdict = ship_verdict(&ShipCheck {
            reviewed: (0, 1),
            danger_flags: 0,
            warn_flags: 0,
            tests: TestStatus::Unknown,
        });
        let body = pr_body(&cs, &verdict, TestStatus::Unknown, 0, 0, None, None);
        assert!(body.contains("1 file · +"), "{body}");
    }

    #[test]
    fn empty_changeset_degrades_gracefully() {
        let cs = changeset_from_diff("");
        assert!(plan_commits(&cs).is_empty());
        assert_eq!(suggest_branch(&cs), "agent/session");
        assert_eq!(pr_title(&cs), "No changes");
    }
}
