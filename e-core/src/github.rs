//! Pull requests on GitHub, through the GitHub CLI (`gh`): the open ones, a
//! pull request's diff, and its files — so one can be reviewed in the editor
//! without checking it out.

use std::path::Path;
use std::process::Command;

use serde_json::Value;

/// An open pull request, as `gh pr list` describes it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub author: String,
    /// The branch it brings in, and the one it goes into.
    pub head: String,
    pub base: String,
    pub draft: bool,
    pub url: String,
    /// The commit at its tip.
    pub head_oid: String,
    /// Its checks, summed up: `pass`, `fail`, `pending`, or empty for none.
    pub checks: String,
    /// `APPROVED`, `CHANGES_REQUESTED`, `REVIEW_REQUIRED`, or empty.
    pub review: String,
}

/// The fields asked of `gh pr list`.
const FIELDS: &str =
    "number,title,author,headRefName,baseRefName,isDraft,url,headRefOid,statusCheckRollup,reviewDecision";

/// Run `gh` in `root`; its stdout, or what went wrong in words.
fn gh(root: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("gh")
        .current_dir(root)
        .args(args)
        // No pager, no colour, no prompt: this is read, not shown.
        .env("GH_PAGER", "")
        .env("NO_COLOR", "1")
        .env("GH_PROMPT_DISABLED", "1")
        .output()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => {
                "the GitHub CLI (gh) isn't installed: brew install gh, then gh auth login"
                    .to_string()
            }
            _ => format!("gh didn't run: {e}"),
        })?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if err.contains("gh auth login") {
            "gh isn't signed in: run gh auth login in a terminal".to_string()
        } else if err.is_empty() {
            format!("gh {} failed", args.first().copied().unwrap_or(""))
        } else {
            err
        })
    }
}

fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .current_dir(root)
        .args(args)
        // A fetch that wants a password fails, rather than waiting on a
        // terminal nobody sees.
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// The repository's open pull requests, newest first.
pub fn list_pulls(root: &Path) -> Result<Vec<PullRequest>, String> {
    let out = gh(
        root,
        &[
            "pr", "list", "--state", "open", "--limit", "50", "--json", FIELDS,
        ],
    )?;
    parse_pulls(&out)
}

/// `gh pr list --json …`'s answer, read.
pub fn parse_pulls(json: &str) -> Result<Vec<PullRequest>, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("gh's answer: {e}"))?;
    let list = v.as_array().ok_or("gh's answer isn't a list")?;
    Ok(list
        .iter()
        .filter_map(|p| {
            let text = |k: &str| p.get(k).and_then(Value::as_str).unwrap_or("").to_string();
            Some(PullRequest {
                number: p.get("number")?.as_u64()?,
                title: text("title"),
                author: p
                    .pointer("/author/login")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                head: text("headRefName"),
                base: text("baseRefName"),
                draft: p.get("isDraft").and_then(Value::as_bool).unwrap_or(false),
                url: text("url"),
                head_oid: text("headRefOid"),
                checks: checks_of(p.get("statusCheckRollup").unwrap_or(&Value::Null)),
                review: text("reviewDecision"),
            })
        })
        .collect())
}

/// A pull request's checks in a word: any failed is `fail`, else any not
/// done is `pending`, else `pass`; none at all is empty.
fn checks_of(rollup: &Value) -> String {
    let Some(checks) = rollup.as_array().filter(|c| !c.is_empty()) else {
        return String::new();
    };
    let mut pending = false;
    for c in checks {
        // A check run has a status and, once completed, a conclusion; a
        // commit status has a state.
        let state = c
            .get("conclusion")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .or_else(|| c.get("state").and_then(Value::as_str))
            .unwrap_or("");
        let status = c
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("COMPLETED");
        match state {
            "FAILURE" | "ERROR" | "CANCELLED" | "TIMED_OUT" | "ACTION_REQUIRED"
            | "STARTUP_FAILURE" => return "fail".into(),
            "SUCCESS" | "NEUTRAL" | "SKIPPED" if status == "COMPLETED" => {}
            _ => pending = true,
        }
    }
    if pending { "pending" } else { "pass" }.into()
}

/// Pull request `number`'s diff against its base, as GitHub shows it.
pub fn pull_diff(root: &Path, number: u64) -> Result<String, String> {
    gh(
        root,
        &["pr", "diff", &number.to_string(), "--color", "never"],
    )
    .map_err(|e| {
        if e.contains("too_large") || e.contains("too large") {
            format!("#{number} is too large for GitHub to send as one diff")
        } else {
            e
        }
    })
}

/// `owner/repo` from a pull request's URL, lower-cased.
fn repo_of(url: &str) -> Option<String> {
    let rest = url.split("github.com/").nth(1)?;
    let mut parts = rest.split('/');
    let (owner, repo) = (parts.next()?, parts.next()?);
    Some(format!("{owner}/{repo}").to_ascii_lowercase())
}

/// Whether a remote's URL is `owner/repo` on GitHub (https or ssh).
fn remote_is(remote_url: &str, repo: &str) -> bool {
    let url = remote_url.trim().to_ascii_lowercase();
    let Some(rest) = url
        .split("github.com")
        .nth(1)
        .map(|r| r.trim_start_matches([':', '/']))
    else {
        return false;
    };
    rest.trim_end_matches('/').trim_end_matches(".git") == repo
}

/// Bring pull request `pr`'s commits into the repository, without touching
/// a branch or the working tree: fetched from the remote its repository is,
/// as `refs/pull/<n>/head`. The commit at its tip, once it is here.
pub fn fetch_pull(root: &Path, pr: &PullRequest) -> Result<String, String> {
    if git(
        root,
        &["cat-file", "-e", &format!("{}^{{commit}}", pr.head_oid)],
    )
    .is_ok()
    {
        return Ok(pr.head_oid.clone());
    }
    let repo = repo_of(&pr.url).ok_or("the pull request's repository isn't on GitHub")?;
    let remotes = git(root, &["remote", "-v"])?;
    let remote = remotes
        .lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            Some((f.next()?, f.next()?))
        })
        .find(|(_, url)| remote_is(url, &repo))
        .map(|(name, _)| name.to_string())
        .ok_or_else(|| format!("no remote here is {repo}"))?;
    git(
        root,
        &[
            "fetch",
            "--no-tags",
            "--quiet",
            &remote,
            &format!("refs/pull/{}/head", pr.number),
        ],
    )?;
    git(
        root,
        &["cat-file", "-e", &format!("{}^{{commit}}", pr.head_oid)],
    )
    .map(|_| pr.head_oid.clone())
    .map_err(|_| "the fetched branch has moved on: refresh the list".to_string())
}

/// `path` as it is at commit `rev`.
pub fn file_at(root: &Path, rev: &str, path: &str) -> Option<String> {
    git(root, &["show", &format!("{rev}:{path}")]).ok()
}

/// Check pull request `number` out, as `gh pr checkout` does: its branch,
/// here. Refused while the working tree has changes.
pub fn checkout_pull(root: &Path, number: u64) -> Result<(), String> {
    if !git(root, &["status", "--porcelain"])?.trim().is_empty() {
        return Err("the working tree has changes: commit or stash them first".into());
    }
    gh(root, &["pr", "checkout", &number.to_string()]).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_is_read_with_its_checks() {
        let json = r#"[
          {"number": 73, "title": "e --wait", "author": {"login": "kwhorne"},
           "headRefName": "editor-wait", "baseRefName": "main", "isDraft": false,
           "url": "https://github.com/kwhorne/e/pull/73", "headRefOid": "abc",
           "reviewDecision": "",
           "statusCheckRollup": [
             {"__typename": "CheckRun", "status": "COMPLETED", "conclusion": "SUCCESS"},
             {"__typename": "CheckRun", "status": "IN_PROGRESS", "conclusion": ""}]},
          {"number": 9, "title": "Bump", "author": {"login": "dependabot"},
           "headRefName": "deps", "baseRefName": "main", "isDraft": true,
           "url": "https://github.com/kwhorne/e/pull/9", "headRefOid": "def",
           "statusCheckRollup": [
             {"__typename": "StatusContext", "state": "SUCCESS"},
             {"__typename": "CheckRun", "status": "COMPLETED", "conclusion": "FAILURE"}]},
          {"number": 8, "title": "Docs", "author": {"login": "a"}, "statusCheckRollup": []}
        ]"#;
        let pulls = parse_pulls(json).unwrap();
        assert_eq!(pulls.len(), 3);
        assert_eq!(pulls[0].author, "kwhorne");
        assert_eq!(pulls[0].checks, "pending");
        assert!(pulls[1].draft);
        assert_eq!(pulls[1].checks, "fail");
        assert_eq!(pulls[2].checks, "");
        assert!(parse_pulls("not json").is_err());
    }

    #[test]
    fn the_remote_is_found_by_its_repository() {
        let repo = repo_of("https://github.com/KWHorne/e/pull/73").unwrap();
        assert_eq!(repo, "kwhorne/e");
        assert!(remote_is("git@github.com:kwhorne/e.git", &repo));
        assert!(remote_is("https://github.com/kwhorne/e", &repo));
        assert!(remote_is("https://github.com/kwhorne/e.git/", &repo));
        assert!(!remote_is("https://github.com/kwhorne/elyra.git", &repo));
        assert!(!remote_is("https://gitlab.com/kwhorne/e.git", &repo));
    }
}

/// Against the repository this is built in, with `gh` signed in:
/// `cargo test -p e-core github_live -- --ignored --nocapture`.
#[cfg(test)]
mod live {
    use super::*;

    #[test]
    #[ignore = "needs gh signed in and an open pull request"]
    fn github_live_a_pull_request_is_read_without_a_checkout() {
        let root = crate::git::repo_root(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap();
        let pulls = list_pulls(&root).unwrap();
        let Some(pr) = pulls.first() else {
            eprintln!("no open pull request to read");
            return;
        };
        eprintln!(
            "#{} {} by {} ({})",
            pr.number, pr.title, pr.author, pr.checks
        );
        let diff = pull_diff(&root, pr.number).unwrap();
        assert!(
            diff.starts_with("diff --git"),
            "{}",
            &diff[..diff.len().min(200)]
        );
        let head = fetch_pull(&root, pr).unwrap();
        let first = diff
            .lines()
            .find_map(|l| l.strip_prefix("+++ b/"))
            .expect("a changed file");
        let text = file_at(&root, &head, first).expect("the file at its tip");
        eprintln!("{first}: {} lines at {}", text.lines().count(), &head[..8]);
    }
}
