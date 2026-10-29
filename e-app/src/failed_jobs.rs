//! Failed queued jobs on the code that runs them. `e` looks at the project's
//! `failed_jobs` table — the local database `.env` names, never one that looks
//! like production — every ten seconds, and a failed job, listener,
//! notification or mail shows in red at the end of its class's `handle()`
//! line, like an exception from the log ([`crate::caught`]): *✖ Failed job
//! SyncOrders: RuntimeException: The warehouse said no · 14:02:11*.
//!
//! `⌘.` there retries it (`php artisan queue:retry`), hands it to the agent
//! with its models and stacktrace, or dismisses it. Several failures of one
//! job share its line, and a retry takes them all. The mark goes when the job
//! leaves the table — retried, or forgotten from a terminal.

use std::path::{Path, PathBuf};

use e_laravel::failed_jobs::{self, FailedJob};
use e_laravel::move_class::Psr4Root;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};

use crate::caught::Caught;
use crate::runtime_lens::{action_target, LensAnchor};
use crate::state::AppState;

/// `failed_jobs_seen` while the database can't be reached: asked again only
/// on the slow beat.
const UNREACHABLE: &str = "unreachable";

/// The failed jobs behind one mark.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobRef {
    /// `App\Jobs\SyncOrders`.
    pub class: String,
    /// What `queue:retry` takes, newest first: one per failure.
    pub retry_ids: Vec<String>,
    pub queue: String,
    /// `App\Models\Order #14` — the newest failure's.
    pub models: String,
}

impl JobRef {
    /// `Failed job SyncOrders`, `Failed job SyncOrders ×3`.
    pub fn title(&self) -> String {
        let short = self.class.rsplit('\\').next().unwrap_or(&self.class);
        match self.retry_ids.len() {
            1 => format!("Failed job {short}"),
            n => format!("Failed job {short} ×{n}"),
        }
    }
}

/// The line to mark for `job`: its class's `handle()` (or `__invoke()`, or
/// the class line for a notification or mail), else the first line of the
/// project's own code it threw from.
fn place(root: &Path, psr4: &[Psr4Root], job: &FailedJob) -> Option<(PathBuf, usize)> {
    let in_class = action_target(root, psr4, &job.class).and_then(|(file, class)| {
        let text = std::fs::read_to_string(&file).ok()?;
        let anchors: Vec<Option<LensAnchor>> = text.lines().map(LensAnchor::of_line).collect();
        let at = |want: &LensAnchor| anchors.iter().position(|a| a.as_ref() == Some(want));
        let line = at(&LensAnchor::Method("handle".into()))
            .or_else(|| at(&LensAnchor::Method("__invoke".into())))
            .or_else(|| at(&class))?;
        Some((file, line))
    });
    in_class.or_else(|| {
        let (path, line) = job.exception.origin(root, |p| p.is_file())?;
        Some((path, line.saturating_sub(1)))
    })
}

/// The failed jobs in `root`'s database, placed and grouped one mark per
/// line, newest first — or `None` when nothing changed since `seen`. The
/// fingerprint comes along, [`UNREACHABLE`] when the table can't be read.
fn read(root: &Path, psr4: &[Psr4Root], seen: Option<&str>) -> (String, Option<Vec<Caught>>) {
    let unreachable = || (UNREACHABLE.to_string(), None);
    let Some((conn, _)) = crate::migrations::local_database(root) else {
        return unreachable();
    };
    let Ok(fp) = e_db::query(&conn, failed_jobs::FINGERPRINT, 1) else {
        return unreachable();
    };
    let fp = format!("{:?}", fp.rows);
    if seen == Some(fp.as_str()) {
        return (fp, None);
    }
    let Ok(rows) = e_db::query(&conn, failed_jobs::QUERY, 20)
        .or_else(|_| e_db::query(&conn, failed_jobs::QUERY_NO_UUID, 20))
    else {
        return unreachable();
    };
    let mut marks: Vec<Caught> = Vec::new();
    for row in &rows.rows {
        let cell = |i: usize| row.get(i).cloned().flatten();
        let Some(job) = failed_jobs::from_row(
            &cell(0).unwrap_or_default(),
            cell(1).as_deref(),
            &cell(2).unwrap_or_default(),
            &cell(3).unwrap_or_default(),
            &cell(4).unwrap_or_default(),
            &cell(5).unwrap_or_default(),
        ) else {
            continue;
        };
        let Some((path, line)) = place(root, psr4, &job) else {
            continue;
        };
        // Newest first, so the one already there is the newest failure.
        if let Some(mark) = marks.iter_mut().find(|m| m.path == path && m.line == line) {
            if let Some(j) = mark.job.as_mut() {
                j.retry_ids.push(job.retry_id().to_string());
            }
            continue;
        }
        let e = &job.exception;
        marks.push(Caught {
            path,
            line,
            class: e.class.clone(),
            message: e.message.clone(),
            time: job
                .failed_at
                .rsplit(' ')
                .next()
                .unwrap_or(&job.failed_at)
                .to_string(),
            text: e.text.clone(),
            job: Some(JobRef {
                class: job.class.clone(),
                retry_ids: vec![job.retry_id().to_string()],
                queue: job.queue.clone(),
                models: job.models_text(),
            }),
        });
    }
    (fp, Some(marks))
}

impl AppState {
    /// Look at the failed jobs again. Called every ten seconds on the idle
    /// tick; `slow` on every minute's, the only one that asks a database
    /// that couldn't be reached last time.
    pub fn poll_failed_jobs(&self, slow: bool) {
        let root = self.root.get_untracked();
        if !root.join("artisan").is_file() || self.failed_jobs_busy.get_untracked() {
            return;
        }
        let seen = self.failed_jobs_seen.get_untracked();
        if seen.as_deref() == Some(UNREACHABLE) && !slow {
            return;
        }
        self.failed_jobs_busy.set(true);
        let psr4 = self.psr4();
        let state = *self;
        self.spawn_bg(
            move || read(&root, &psr4, seen.as_deref()),
            move |(fp, found): (String, Option<Vec<Caught>>)| {
                state.failed_jobs_busy.set(false);
                state.failed_jobs_seen.set(Some(fp));
                if let Some(found) = found {
                    state.keep_failed_jobs(found);
                }
            },
        );
    }

    /// The failed jobs are `found` now: those gone from the table go from the
    /// editor too.
    fn keep_failed_jobs(&self, found: Vec<Caught>) {
        let same = self.caught.with_untracked(|kept| {
            let jobs: Vec<&Caught> = kept.iter().filter(|c| c.job.is_some()).collect();
            jobs.len() == found.len() && jobs.iter().all(|j| found.contains(j))
        });
        if same {
            return;
        }
        self.caught.update(|kept| {
            kept.retain(|c| c.job.is_none());
            // Older than the log's exceptions, so a new request's one is the
            // status bar's.
            let logged = std::mem::take(kept);
            kept.extend(found.into_iter().rev());
            kept.extend(logged);
        });
        self.sync_caught_marks();
    }

    /// Push the failed job on `line` of `path` back onto its queue — every
    /// failure of it there — and look at the table again.
    pub fn retry_failed_job(&self, path: &Path, line: usize) {
        let Some(job) = self.caught_at(path, line).and_then(|c| c.job) else {
            return;
        };
        let root = self.root.get_untracked();
        let state = *self;
        let title = job.title();
        self.spawn_bg(
            move || {
                let mut args = vec!["artisan", "queue:retry"];
                args.extend(job.retry_ids.iter().map(String::as_str));
                args.push("--no-ansi");
                let out = crate::phptools::php(&root, &args, std::time::Duration::from_secs(60))?;
                let text = format!(
                    "{}{}",
                    String::from_utf8_lossy(&out.stdout),
                    String::from_utf8_lossy(&out.stderr)
                );
                if out.status.success() {
                    Ok(())
                } else {
                    Err(text
                        .lines()
                        .map(str::trim)
                        .find(|l| !l.is_empty())
                        .unwrap_or("queue:retry failed")
                        .to_string())
                }
            },
            move |res: Result<(), String>| {
                match res {
                    Ok(()) => AppState::notify(&format!("{title}: pushed back onto the queue")),
                    Err(e) => AppState::notify(&format!("Couldn't retry {title}: {e}")),
                }
                state.failed_jobs_seen.set(None);
                state.poll_failed_jobs(true);
            },
        );
    }

    /// Hand the failed job on `line` of `path` to the agent: its class,
    /// models and stacktrace.
    pub fn failed_job_to_agent(&self, path: &Path, line: usize) {
        let Some(c) = self.caught_at(path, line) else {
            return;
        };
        let Some(job) = &c.job else {
            return;
        };
        let models = if job.models.is_empty() {
            String::new()
        } else {
            format!(", dispatched with {}", job.models)
        };
        let failures = match job.retry_ids.len() {
            1 => "once".to_string(),
            n => format!("{n} times"),
        };
        let trace: String = c.text.chars().take(6000).collect();
        self.send_to_agent(&format!(
            "The queued job {} failed {failures} on the `{}` queue{models}. The latest threw {}: {}\n\n\
             Find the cause and fix it. Once fixed it can be retried with \
             `php artisan queue:retry {}`. The exception from failed_jobs:\n\n{trace}",
            job.class,
            job.queue,
            c.class,
            c.message,
            job.retry_ids.join(" "),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_job_is_marked_on_its_handle_line() {
        let root = std::env::temp_dir().join(format!("e-failed-jobs-{}", std::process::id()));
        std::fs::create_dir_all(root.join("app/Jobs")).unwrap();
        std::fs::write(
            root.join("app/Jobs/SyncOrders.php"),
            "<?php\n\nnamespace App\\Jobs;\n\nclass SyncOrders implements ShouldQueue\n{\n    public function __construct(public Order $order) {}\n\n    public function handle(Warehouse $w): void\n    {\n    }\n}\n",
        )
        .unwrap();
        let payload = r#"{"displayName":"App\\Jobs\\SyncOrders","data":{"command":""}}"#;
        let job =
            failed_jobs::from_row("1", None, "default", payload, "Exception: no", "x").unwrap();
        assert_eq!(
            place(&root, &[], &job),
            Some((root.join("app/Jobs/SyncOrders.php"), 8))
        );

        // A class the project doesn't have: where it threw in the project.
        let exception = format!(
            "Exception: no in {}\nStack trace:\n#0 {{main}}",
            root.join("app/Jobs/SyncOrders.php:10").display()
        );
        let payload = r#"{"displayName":"Vendor\\Thing","data":{"command":""}}"#;
        let job = failed_jobs::from_row("2", None, "default", payload, &exception, "x").unwrap();
        assert_eq!(
            place(&root, &[], &job),
            Some((root.join("app/Jobs/SyncOrders.php"), 9))
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn several_failures_are_counted_in_the_title() {
        let mut job = JobRef {
            class: "App\\Jobs\\SyncOrders".into(),
            retry_ids: vec!["a".into()],
            queue: "default".into(),
            models: String::new(),
        };
        assert_eq!(job.title(), "Failed job SyncOrders");
        job.retry_ids.push("b".into());
        assert_eq!(job.title(), "Failed job SyncOrders ×2");
    }
}
