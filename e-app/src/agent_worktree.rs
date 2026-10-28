//! A Cascade session in a worktree of its own: the agent works on a branch
//! beside your checkout — with its own `.env`, its own copy of the database
//! and its own site — so migrations, test runs and half-done edits stay off
//! the files you are working in and the database you are using.
//!
//! Grove does the heavy lifting (`grove try`, see [`crate::grove::try_new`]).
//! A project Grove doesn't serve — a Rust or Go one, `e` itself, a Laravel
//! app not linked yet — gets a plain git worktree under
//! `~/.config/e/worktrees/` instead: its own branch and files, with `.env` and
//! an SQLite database copied over, but no site. Here: starting a session in
//! one, the line in Cascade that says where the agent is working, and the
//! shelf of earlier ones in the ⋯ menu — each to go back to, or take down once
//! its branch is merged.

use std::path::{Path, PathBuf};

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

/// Where the plain worktrees of the project at `root` go:
/// `~/.config/e/worktrees/<project>-<hash of its path>`.
fn plain_dir(root: &Path) -> Option<PathBuf> {
    use std::hash::{Hash, Hasher};
    let home = std::env::var_os("HOME")?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    root.to_string_lossy().hash(&mut h);
    let name = root.file_name()?.to_string_lossy();
    Some(
        PathBuf::from(home)
            .join(".config/e/worktrees")
            .join(format!("{name}-{:08x}", h.finish() as u32)),
    )
}

/// A plain git worktree for `branch`, made from the checkout's HEAD, for a
/// project Grove doesn't serve. `.env` and a `database/database.sqlite` come
/// along, as they aren't in git; any other database is the checkout's own.
fn plain_new(root: &Path, branch: &str) -> Result<Try, String> {
    plain_new_in(&plain_dir(root).ok_or("no home directory")?, root, branch)
}

/// [`plain_new`], in `dir`.
fn plain_new_in(dir: &Path, root: &Path, branch: &str) -> Result<Try, String> {
    let path = dir.join(branch.replace('/', "-"));
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    e_core::git::worktree_add(root, branch, &path)?;
    for file in [".env", "database/database.sqlite"] {
        if root.join(file).is_file() && !path.join(file).exists() {
            if let Some(parent) = path.join(file).parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::copy(root.join(file), path.join(file));
        }
    }
    Ok(Try {
        site: String::new(),
        branch: branch.to_string(),
        project: root.to_path_buf(),
        path,
        url: String::new(),
    })
}

/// The worktree sessions made from the project at `root`: Grove's tries and
/// the plain worktrees `e` made, by branch.
pub fn worktrees_of(root: &Path) -> Vec<Try> {
    let mut out = if grove::available() {
        grove::tries_of(root)
    } else {
        Vec::new()
    };
    if let Some(dir) = plain_dir(root) {
        for t in plain_in(&dir, root) {
            if !out.iter().any(|o| o.path == t.path) {
                out.push(t);
            }
        }
    }
    out.sort_by(|a, b| a.branch.cmp(&b.branch));
    out
}

/// The repository's worktrees that live in `dir`.
fn plain_in(dir: &Path, root: &Path) -> Vec<Try> {
    let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let dir = canon(dir);
    e_core::git::worktrees(root)
        .into_iter()
        .filter(|(path, _)| canon(path).starts_with(&dir))
        .map(|(path, branch)| Try {
            site: String::new(),
            branch,
            project: root.to_path_buf(),
            path,
            url: String::new(),
        })
        .collect()
}

/// Whether `t` has a site of its own: Grove's tries do, plain worktrees don't.
pub fn has_site(t: &Try) -> bool {
    !t.url.is_empty()
}

/// Whether the project at `root` keeps its data somewhere a plain worktree
/// shares: a Laravel app whose database isn't SQLite.
fn shares_database(root: &Path) -> bool {
    if !root.join("artisan").is_file() {
        return false;
    }
    let env = std::fs::read_to_string(root.join(".env")).unwrap_or_default();
    !env.lines().any(|l| {
        l.trim()
            .strip_prefix("DB_CONNECTION=")
            .is_some_and(|v| v.trim().trim_matches('"') == "sqlite")
    })
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
        self.cascade_note(format!("Making {branch} beside your checkout…"), false);
        let app = *self;
        self.spawn_bg(
            move || {
                // Asked here rather than from the Runtime panel's lookup,
                // which may not have run yet. With Grove serving the project
                // the worktree gets its own site and database; without, it's
                // a plain git worktree.
                if grove::site_for(&root).is_some() {
                    return grove::try_new(&root, &branch).map(|t| (t, false));
                }
                plain_new(&root, &branch).map(|t| (t, shares_database(&root)))
            },
            move |res: Result<(Try, bool), String>| match res {
                Ok((t, shared)) => {
                    let ready = if has_site(&t) {
                        format!(
                            "Working in {} — its own database, and its site is {}",
                            t.branch, t.url
                        )
                    } else if shared {
                        format!(
                            "Working in {}, a worktree of its own. It shares your database: Grove isn't serving this project (grove link gives a worktree a copy of its own)",
                            t.branch
                        )
                    } else {
                        format!("Working in {}, a worktree of its own", t.branch)
                    };
                    app.cascade_enter_worktree(Some(t));
                    // Shown as a warning: migrations there reach your data.
                    app.cascade_note(ready, shared);
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
    /// Grove, and git for a plain one, refuse while it holds uncommitted work.
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
        let app = *self;
        let down = t.clone();
        self.spawn_bg(
            move || {
                if has_site(&down) {
                    grove::try_done(&root, &down.branch, false)
                } else {
                    e_core::git::worktree_remove(&root, &down.path)
                }
            },
            move |res: Result<(), String>| match res {
                Ok(()) => app.cascade_note(
                    if has_site(&t) {
                        format!(
                            "{} is taken down: its site, database and worktree. The branch is kept",
                            t.branch
                        )
                    } else {
                        format!("{}'s worktree is removed. The branch is kept", t.branch)
                    },
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

    fn git(dir: &Path, args: &[&str]) {
        let ok = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .is_ok_and(|o| o.status.success());
        assert!(ok, "git {args:?} failed");
    }

    #[test]
    fn a_project_without_grove_gets_a_plain_worktree() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!("e-wt-{}-{nanos}", std::process::id()));
        let root = base.join("app");
        let dir = base.join("worktrees");
        std::fs::create_dir_all(root.join("database")).unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        git(&root, &["config", "user.email", "t@example.com"]);
        git(&root, &["config", "user.name", "Test"]);
        git(&root, &["config", "commit.gpgsign", "false"]);
        std::fs::write(root.join(".gitignore"), ".env\n*.sqlite\n").unwrap();
        std::fs::write(root.join("artisan"), "").unwrap();
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-qm", "init"]);
        std::fs::write(root.join(".env"), "DB_CONNECTION=sqlite\n").unwrap();
        std::fs::write(root.join("database/database.sqlite"), "rows").unwrap();

        let t = plain_new_in(&dir, &root, "e/agent-1028-120000").unwrap();
        assert!(!has_site(&t));
        assert_eq!(t.path, dir.join("e-agent-1028-120000"));
        assert!(t.path.join("artisan").is_file());
        // What git doesn't carry comes along.
        assert_eq!(
            std::fs::read_to_string(t.path.join(".env")).unwrap(),
            "DB_CONNECTION=sqlite\n"
        );
        assert_eq!(
            std::fs::read_to_string(t.path.join("database/database.sqlite")).unwrap(),
            "rows"
        );
        assert!(!shares_database(&root));

        // It is on the shelf, and the checkout itself isn't.
        let shelf = plain_in(&dir, &root);
        assert_eq!(shelf.len(), 1);
        assert_eq!(shelf[0].branch, "e/agent-1028-120000");

        // Taking it down removes the worktree and keeps the branch.
        e_core::git::worktree_remove(&root, &t.path).unwrap();
        assert!(!t.path.exists());
        assert!(plain_in(&dir, &root).is_empty());
        assert!(e_core::git::branches(&root).contains(&"e/agent-1028-120000".to_string()));
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn only_a_laravel_app_off_sqlite_shares_its_database() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("e-wt-db-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        // Not Laravel: nothing to share.
        assert!(!shares_database(&root));
        std::fs::write(root.join("artisan"), "").unwrap();
        std::fs::write(root.join(".env"), "DB_CONNECTION=mysql\n").unwrap();
        assert!(shares_database(&root));
        std::fs::write(root.join(".env"), "DB_CONNECTION=\"sqlite\"\n").unwrap();
        assert!(!shares_database(&root));
        std::fs::remove_dir_all(&root).ok();
    }
}
