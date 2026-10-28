//! Running the tests a save touches. With **Run affected tests on save** on,
//! saving a PHP file runs the test files that exercise it
//! ([`e_laravel::test_impact`]) — the file itself when it is a test — in the
//! background, without the Tests panel. The gutter's marks update when it
//! finishes, and the status bar says how it went; a click there opens the
//! panel with the output.

use std::path::{Path, PathBuf};
use std::time::Duration;

use e_core::language::Language;
use e_laravel::test_impact;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use floem::views::{label, Decorators};
use floem::IntoView;

use crate::state::{AppState, TddStatus};
use crate::theme;

/// Saves this close together run as one: ⌘S on several files, or a save that
/// formats and writes twice.
const SETTLE: Duration = Duration::from_millis(400);

/// More test files than this, and a save is wider than "the tests for this
/// file": the status bar says so and leaves it to a full run.
const MAX_FILES: usize = 12;

/// How the last run after a save went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SaveRun {
    Running {
        files: usize,
    },
    Done {
        passed: usize,
        failed: usize,
    },
    /// Too many test files touched to run on a save.
    TooWide {
        files: usize,
    },
}

/// The extra arguments that run just `files` (relative to the project): the
/// file itself when there is one, else a filter on the test classes' names.
pub fn run_args(files: &[PathBuf]) -> String {
    if let [one] = files {
        return one.to_string_lossy().into_owned();
    }
    let names: Vec<String> = files
        .iter()
        .filter_map(|f| f.file_stem().and_then(|s| s.to_str()))
        .map(crate::tasks::filter_pattern)
        .collect();
    format!("--filter=\"{}\"", names.join("|"))
}

impl AppState {
    /// After `path` is written: note it for a run of the tests it touches.
    pub(crate) fn tests_after_save(&self, path: &Path, language: Language) {
        if !self.settings.get_untracked().test_on_save || language != Language::Php {
            return;
        }
        let first = self.save_run_queue.with_untracked(Vec::is_empty);
        self.save_run_queue.update(|q| {
            if !q.iter().any(|p| p == path) {
                q.push(path.to_path_buf());
            }
        });
        if first {
            let state = *self;
            floem::action::exec_after(SETTLE, move |_| state.start_save_run());
        }
    }

    /// Find the tests the queued saves touch, and run them.
    fn start_save_run(&self) {
        // One run at a time, and never under the agent's fix-to-green loop:
        // try again once the running one is done.
        if self.tdd_status.get_untracked() == TddStatus::Running || self.tdd_loop.get_untracked() {
            let state = *self;
            floem::action::exec_after(Duration::from_secs(1), move |_| state.start_save_run());
            return;
        }
        let saved = self
            .save_run_queue
            .try_update(std::mem::take)
            .unwrap_or_default();
        if saved.is_empty() {
            return;
        }
        let root = self.root.get_untracked();
        let routes = self
            .laravel
            .get_untracked()
            .map(|d| d.all_routes.clone())
            .unwrap_or_default();
        let state = *self;
        self.spawn_bg(
            move || {
                let tests = test_impact::test_files(&root);
                let mut files: Vec<PathBuf> = Vec::new();
                for path in &saved {
                    let rel = path.strip_prefix(&root).unwrap_or(path);
                    let src = std::fs::read_to_string(path).unwrap_or_default();
                    files.extend(test_impact::affected_tests(rel, &src, &tests, &routes));
                }
                files.sort();
                files.dedup();
                files
            },
            move |files: Vec<PathBuf>| state.run_saved_tests(files),
        );
    }

    fn run_saved_tests(&self, files: Vec<PathBuf>) {
        if files.is_empty() {
            return;
        }
        if files.len() > MAX_FILES {
            self.save_run
                .set(Some(SaveRun::TooWide { files: files.len() }));
            return;
        }
        let Some(cmd) = crate::tasks::test_command(&self.root.get_untracked()) else {
            return;
        };
        let cmd = format!("{cmd} {}", run_args(&files));
        self.save_run
            .set(Some(SaveRun::Running { files: files.len() }));
        self.tdd_status.set(TddStatus::Running);
        let state = *self;
        self.spawn_test_run(cmd, move |code, text, run| {
            state.keep_test_results(&run);
            state.save_run.set(Some(SaveRun::Done {
                passed: run.passed(),
                failed: run.failed(),
            }));
            // The Tests panel shows this run when opened from the status bar.
            state.tdd_status.set(if code == 0 {
                TddStatus::Passed
            } else {
                TddStatus::Failed
            });
            state.tdd_output.set(text);
            state.tdd_results.set(run);
        });
    }
}

/// The status bar's word on the last run after a save. Click to open the
/// Tests panel with its output.
pub fn save_run_status(state: AppState) -> impl IntoView {
    label(move || match state.save_run.get() {
        Some(SaveRun::Running { files }) => {
            format!("⟳ tests: {files} file{}", if files == 1 { "" } else { "s" })
        }
        Some(SaveRun::Done { passed, failed: 0 }) => format!("✓ {passed} passed"),
        Some(SaveRun::Done { passed, failed }) => format!("✗ {failed} failed, {passed} passed"),
        Some(SaveRun::TooWide { files }) => format!("{files} test files touched — run the suite"),
        None => String::new(),
    })
    .style(move |s| {
        let color = match state.save_run.get() {
            Some(SaveRun::Done { failed: 0, .. }) => theme::success(),
            Some(SaveRun::Done { .. }) => theme::error(),
            _ => theme::fg_dim(),
        };
        s.color(color)
            .cursor(floem::style::CursorStyle::Pointer)
            .hover(|s| s.color(theme::fg()))
    })
    .on_click_stop(move |_| state.tdd_open.set(true))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_file_runs_by_path_and_several_by_their_names() {
        assert_eq!(
            run_args(&[PathBuf::from("tests/Feature/OrderTest.php")]),
            "tests/Feature/OrderTest.php"
        );
        assert_eq!(
            run_args(&[
                PathBuf::from("tests/Feature/CheckoutTest.php"),
                PathBuf::from("tests/Unit/OrderTest.php"),
            ]),
            "--filter=\"CheckoutTest|OrderTest\""
        );
    }
}
