//! Watching the project on disk, so changes made elsewhere — a terminal, an
//! agent, `git checkout`, `composer` — reach `e` as they happen, instead of
//! being looked for every half second.
//!
//! The project root is watched without recursion, and each folder in it
//! recursively, except the ones too big or too busy to be worth it
//! (`vendor`, `node_modules`, build output, Laravel's `storage`): on Linux a
//! recursive watch costs one inotify watch per folder, and `node_modules`
//! alone can use up the limit. From `.git` only what tells a commit or a
//! checkout (`logs/HEAD`, `HEAD`, `index`) is watched, and from `storage`
//! only `logs`. A folder that appears at the top level is watched from then
//! on.
//!
//! Events are gathered for a moment and handed to the UI thread as one
//! batch: a checkout that touches four hundred files is one refresh. When
//! the watch can't be set up, [`active`] stays false and the idle tick goes
//! on polling as before.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use notify::{EventKind, RecursiveMode, Watcher};

/// Folders at the top of a project that aren't watched.
const SKIPPED: &[&str] = &[
    "vendor",
    "node_modules",
    ".git",
    "storage",
    "target",
    "dist",
    "build",
    ".next",
    ".nuxt",
    ".idea",
    ".vscode",
    ".DS_Store",
];

/// How long a burst of events may run on before it's handed over.
const QUIET: Duration = Duration::from_millis(150);
/// …and at most this long after its first event.
const MAX_WAIT: Duration = Duration::from_millis(600);

static ACTIVE: AtomicBool = AtomicBool::new(false);

/// Whether the project is being watched (else the idle tick polls).
pub fn active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

/// What changed on disk in one burst.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Changes {
    pub paths: BTreeSet<PathBuf>,
    /// Something was created, removed or renamed (the file tree changes).
    pub structure: bool,
}

/// Watch `root`. Batches of changes arrive on the receiver; `None` when the
/// watch couldn't be set up.
pub fn start(root: &Path) -> Option<Receiver<Changes>> {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let (raw_tx, raw_rx) = mpsc::channel::<notify::Event>();
    let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(event) = res {
            let _ = raw_tx.send(event);
        }
    })
    .ok()?;
    let watcher = Arc::new(Mutex::new(watcher));
    for (path, mode) in watch_list(&root) {
        let added = watcher.lock().ok()?.watch(&path, mode);
        // The root and its folders must be watched; the extras (the reflog,
        // the log folder) may not exist yet.
        if added.is_err() && (path == root || mode == RecursiveMode::Recursive) {
            eprintln!(
                "e: can't watch {} ({:?}); looking for changes every half second instead",
                path.display(),
                added.err()
            );
            return None;
        }
    }
    let (tx, rx) = mpsc::channel::<Changes>();
    std::thread::spawn(move || gather(&root, raw_rx, watcher, tx));
    ACTIVE.store(true, Ordering::Relaxed);
    Some(rx)
}

/// What to watch under `root`, and how.
fn watch_list(root: &Path) -> Vec<(PathBuf, RecursiveMode)> {
    let mut out = vec![(root.to_path_buf(), RecursiveMode::NonRecursive)];
    if let Ok(rd) = std::fs::read_dir(root) {
        for e in rd.flatten() {
            let path = e.path();
            if path.is_dir() && !skipped(root, &path) {
                out.push((path, RecursiveMode::Recursive));
            }
        }
    }
    out.push((root.join(".git"), RecursiveMode::NonRecursive));
    out.push((root.join(".git/logs"), RecursiveMode::NonRecursive));
    out.push((root.join("storage/logs"), RecursiveMode::NonRecursive));
    out
}

/// Whether `path`, directly under `root`, is a folder left unwatched.
fn skipped(root: &Path, path: &Path) -> bool {
    path.parent() == Some(root)
        && path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| SKIPPED.contains(&n))
}

/// Whether a change at `path` is worth telling the editor about: not in an
/// unwatched folder (a watch on `.git` sees all of it), not editor or OS
/// litter.
pub fn relevant(root: &Path, path: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(root) else {
        return false;
    };
    let rel = rel.to_string_lossy();
    if rel.starts_with(".git/") || rel == ".git" {
        return matches!(rel.as_ref(), ".git/HEAD" | ".git/index" | ".git/logs/HEAD");
    }
    if rel.starts_with("storage/") {
        return rel.starts_with("storage/logs/") && rel.ends_with(".log");
    }
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let litter = name == ".DS_Store"
        || name.ends_with('~')
        || name.ends_with(".swp")
        || name.starts_with(".e-pint-")
        || name.ends_with(".tmp");
    let top = rel.split('/').next().unwrap_or("");
    !litter && !(rel.contains('/') && SKIPPED.contains(&top))
}

fn structural(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_)
            | EventKind::Remove(_)
            | EventKind::Modify(notify::event::ModifyKind::Name(_))
    )
}

/// Gather raw events into batches, and watch new top-level folders.
fn gather(
    root: &Path,
    raw: Receiver<notify::Event>,
    watcher: Arc<Mutex<notify::RecommendedWatcher>>,
    tx: mpsc::Sender<Changes>,
) {
    let mut batch = Changes::default();
    let mut first: Option<Instant> = None;
    loop {
        let wait = match first {
            None => Duration::from_secs(3600),
            Some(t) => QUIET.min(MAX_WAIT.saturating_sub(t.elapsed())),
        };
        match raw.recv_timeout(wait) {
            Ok(event) => {
                if matches!(event.kind, EventKind::Access(_)) {
                    continue;
                }
                let structure = structural(&event.kind);
                for path in event.paths {
                    // A folder created at the top level: watch it from now on.
                    if structure
                        && path.parent() == Some(root)
                        && path.is_dir()
                        && !skipped(root, &path)
                    {
                        if let Ok(mut w) = watcher.lock() {
                            let _ = w.watch(&path, RecursiveMode::Recursive);
                        }
                    }
                    if relevant(root, &path) {
                        batch.structure |= structure;
                        batch.paths.insert(path);
                        first.get_or_insert_with(Instant::now);
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                if !batch.paths.is_empty() && tx.send(std::mem::take(&mut batch)).is_err() {
                    break;
                }
                first = None;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if first.is_some_and(|t| t.elapsed() >= MAX_WAIT) && !batch.paths.is_empty() {
            if tx.send(std::mem::take(&mut batch)).is_err() {
                break;
            }
            first = None;
        }
    }
    ACTIVE.store(false, Ordering::Relaxed);
}

/// Whether a changed file is one the Laravel data is read from (the same
/// places [`crate::state::laravel_fingerprint`] looks at): `.env`, routes,
/// config, translations, class components, and views appearing or going.
pub fn laravel_relevant(rel: &str, structure: bool) -> bool {
    rel == ".env"
        || rel.starts_with("routes/")
        || rel.starts_with("config/")
        || rel.starts_with("lang/")
        || rel.starts_with("resources/lang/")
        || rel.starts_with("app/View/Components/")
        || (structure && rel.starts_with("resources/views/"))
}

impl crate::state::AppState {
    /// A batch of changes made on disk outside `e` (or by it): bring what
    /// depends on them up to date.
    pub(crate) fn on_disk_changes(&self, changes: Changes) {
        let root = self.root.get_untracked();
        let root = root.canonicalize().unwrap_or(root);
        let rels: Vec<String> = changes
            .paths
            .iter()
            .filter_map(|p| p.strip_prefix(&root).ok())
            .map(|r| r.to_string_lossy().into_owned())
            .collect();
        let in_tree = rels
            .iter()
            .any(|r| !r.starts_with(".git/") && !r.starts_with("storage/"));
        if changes.structure && in_tree {
            self.fs_rev.update(|r| *r += 1);
        }
        if in_tree {
            // A `stat` per open file: which of them changed is its business.
            self.check_external_changes();
            self.refresh_git_status();
        }
        if rels
            .iter()
            .any(|r| r == ".git/logs/HEAD" || r == ".git/HEAD")
        {
            self.check_head_moved();
        } else if rels.iter().any(|r| r == ".git/index") {
            self.refresh_git_status();
        }
        if self.project.laravel.get_untracked().is_some()
            && rels.iter().any(|r| laravel_relevant(r, changes.structure))
        {
            eprintln!("e: Laravel project files changed on disk — refreshing");
            self.load_laravel();
        }
        if rels.iter().any(|r| r.starts_with("storage/logs/")) {
            self.poll_laravel_log();
            if self.log.open.get_untracked() {
                self.refresh_laravel_log();
            }
        }
        if rels.iter().any(|r| r == ".e/rules.toml") {
            self.load_project_rules();
        }
    }

    /// Whether a move of HEAD reaches `e` through the watch: the repository's
    /// `.git` is inside the watched project, and where HEAD was has been
    /// noted (the first look only notes it, so a move seen first by the
    /// watch would be taken for the starting point). Until then HEAD is
    /// looked at on the idle tick.
    pub(crate) fn head_is_watched(&self) -> bool {
        active()
            && self.head_moved.get_untracked().is_some()
            && self
                .git
                .root
                .get_untracked()
                .is_some_and(|repo| repo == self.root.get_untracked())
    }

    /// Whether what the Laravel log gains reaches `e` through the watch:
    /// where the log ended has been noted (as with HEAD, the first look only
    /// notes it).
    pub(crate) fn log_is_watched(&self) -> bool {
        active() && self.log.read.with_untracked(Option::is_some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_reaches_the_editor() {
        let root = Path::new("/p");
        let yes = |p: &str| relevant(root, &root.join(p));
        assert!(yes("app/Models/User.php"));
        assert!(yes(".env"));
        assert!(yes(".git/logs/HEAD"));
        assert!(yes(".git/index"));
        assert!(yes("storage/logs/laravel.log"));
        assert!(!yes(".git/objects/ab/cdef"));
        assert!(!yes(".git/index.lock"));
        assert!(!yes("storage/framework/views/x.php"));
        assert!(!yes("vendor/laravel/framework/src/x.php"));
        assert!(!yes("node_modules/vue/index.js"));
        assert!(!yes("app/.DS_Store"));
        assert!(!yes("app/Models/.e-pint-1-0.php"));
        // A top-level file named like a skipped folder is still a file.
        assert!(yes("build"));
        assert!(!relevant(root, Path::new("/elsewhere/x.php")));
    }

    #[test]
    fn what_the_laravel_data_comes_from() {
        assert!(laravel_relevant("routes/web.php", false));
        assert!(laravel_relevant(".env", false));
        assert!(laravel_relevant("lang/nb/messages.php", false));
        assert!(laravel_relevant(
            "resources/views/orders/show.blade.php",
            true
        ));
        assert!(!laravel_relevant(
            "resources/views/orders/show.blade.php",
            false
        ));
        assert!(!laravel_relevant("app/Models/User.php", false));
    }

    #[test]
    fn a_commit_reaches_the_editor_through_the_reflog() {
        let dir = std::env::temp_dir().join(format!("e-fs-watch-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("app")).unwrap();
        let git = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .args(["-c", "user.name=e", "-c", "user.email=e@example.com"])
                .args(args)
                .current_dir(&dir)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            assert!(ok, "git {args:?}");
        };
        git(&["init", "-q"]);
        std::fs::write(dir.join("app/a.php"), "<?php").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-qm", "first"]);
        let rx = start(&dir).expect("watch");
        let root = dir.canonicalize().unwrap();
        std::thread::sleep(Duration::from_millis(300));
        while rx.try_recv().is_ok() {}
        git(&["commit", "-q", "--allow-empty", "-m", "second"]);
        let reflog = root.join(".git/logs/HEAD");
        let mut seen = false;
        let until = Instant::now() + Duration::from_secs(5);
        while Instant::now() < until && !seen {
            if let Ok(b) = rx.recv_timeout(Duration::from_millis(500)) {
                seen = b.paths.contains(&reflog);
            }
        }
        assert!(seen, "the reflog's change");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_change_arrives_as_one_batch() {
        let dir = std::env::temp_dir().join(format!("e-fs-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("app")).unwrap();
        std::fs::create_dir_all(dir.join("vendor/pkg")).unwrap();
        let rx = start(&dir).expect("watch");
        let root = dir.canonicalize().unwrap();
        std::thread::sleep(Duration::from_millis(300));
        for i in 0..5 {
            std::fs::write(root.join(format!("app/f{i}.php")), "<?php").unwrap();
        }
        std::fs::write(root.join("vendor/pkg/x.php"), "<?php").unwrap();
        // FSEvents may first hand over the folders this test made just before
        // the watch began: gather batches until the files show up.
        let mut all = Changes::default();
        let until = Instant::now() + Duration::from_secs(5);
        while Instant::now() < until && !all.paths.iter().any(|p| p.ends_with("app/f4.php")) {
            if let Ok(b) = rx.recv_timeout(Duration::from_millis(500)) {
                all.structure |= b.structure;
                all.paths.extend(b.paths);
            }
        }
        assert!(all.structure, "{all:?}");
        assert!(
            all.paths.iter().any(|p| p.ends_with("app/f0.php")),
            "{all:?}"
        );
        assert!(
            !all.paths
                .iter()
                .any(|p| p.starts_with(root.join("vendor/pkg"))),
            "{all:?}"
        );
        // A folder made at the top level is watched from then on.
        // Wait until the watch has seen the folder appear (and so started
        // watching it) before writing into it; a fixed pause raced that under
        // load.
        std::fs::create_dir_all(root.join("modules")).unwrap();
        let mut folder = false;
        let until = Instant::now() + Duration::from_secs(5);
        while Instant::now() < until && !folder {
            if let Ok(b) = rx.recv_timeout(Duration::from_millis(500)) {
                folder = b.paths.iter().any(|p| p.ends_with("modules"));
            }
        }
        assert!(folder, "the new folder");
        std::thread::sleep(Duration::from_millis(100));
        std::fs::write(root.join("modules/a.php"), "<?php").unwrap();
        let mut seen = false;
        let until = Instant::now() + Duration::from_secs(5);
        while Instant::now() < until && !seen {
            if let Ok(b) = rx.recv_timeout(Duration::from_millis(500)) {
                seen = b.paths.iter().any(|p| p.ends_with("modules/a.php"));
            }
        }
        assert!(seen, "a change in a new top-level folder");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
