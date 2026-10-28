//! Autonomous TDD loop: run the test suite, and (optionally) drive the agent to
//! fix failures until green. Extracted from the former `state.rs` god-module.

use std::collections::HashMap;
use std::path::Path;

use floem::ext_event::create_ext_action;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use floem::views::editor::text::Document;

use crate::state::{AppState, TddStatus};

impl AppState {
    // ---- Autonomous TDD loop ------------------------------------------

    pub fn toggle_tdd(&self) {
        self.tdd_open.update(|o| *o = !*o);
    }

    /// Run the project's test suite, parse pass/fail, and (when the loop is
    /// active) ask the agent to fix failures.
    pub fn run_tests(&self) {
        let root = self.root.get_untracked();
        let Some(cmd) = crate::tasks::test_command(&root) else {
            self.tdd_output
                .set("No test command detected for this project.".into());
            self.tdd_status.set(TddStatus::Failed);
            self.tdd_open.set(true);
            return;
        };
        // "Run this test" narrows one run; the next run is the whole suite again.
        let cmd = match self.tdd_filter.try_update(|f| f.take()).flatten() {
            Some(extra) => format!("{cmd} {extra}"),
            None => cmd,
        };
        // Safety cap so an unproductive loop can't run forever.
        if self.tdd_loop.get_untracked() && self.tdd_iteration.get_untracked() >= 15 {
            self.tdd_loop.set(false);
            self.tdd_output
                .update(|o| o.push_str("\n\n[stopped: reached 15 iterations]"));
            return;
        }
        self.tdd_open.set(true);
        self.tdd_status.set(TddStatus::Running);
        // Ask the runner for machine-readable results where it can produce
        // them, so a failure has somewhere to jump to instead of being a wall of
        // text. Runners that can't are run exactly as before.
        let report = std::env::temp_dir().join(format!("e-junit-{}.xml", std::process::id()));
        let cmd = crate::testrun::junit_command(&cmd, &report).unwrap_or(cmd);
        let report_for_thread = report.clone();
        let root_for_parse = root.clone();

        let state = *self;
        let send = create_ext_action(
            self.cx,
            move |(code, text, run): (i32, String, crate::testrun::TestRun)| {
                state
                    .test_outcomes
                    .update(|kept| crate::testrun::remember(kept, &run));
                crate::testrun::save_kept(
                    &state.root.get_untracked(),
                    state.test_outcomes.get_untracked(),
                );
                state.sync_test_marks();
                state.tdd_results.set(run);
                state.tdd_iteration.update(|i| *i += 1);
                let passed = code == 0;
                state.tdd_status.set(if passed {
                    TddStatus::Passed
                } else {
                    TddStatus::Failed
                });
                state.tdd_output.set(text.clone());
                if passed {
                    state.tdd_loop.set(false);
                } else if state.tdd_loop.get_untracked() {
                    let tail: String = {
                        let n = text.len().saturating_sub(4000);
                        text[n..].to_string()
                    };
                    state.send_to_agent(&format!(
                        "The test suite is failing. Fix the code so the tests pass. \
                     Use propose_edit over $E_EDITOR_SOCK for your changes so I can review them. \
                     Failing output:\n{tail}"
                    ));
                }
            },
        );
        std::thread::spawn(move || {
            let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
            let (code, text) = match std::process::Command::new(shell)
                .arg("-ilc")
                .arg(&cmd)
                .current_dir(&root)
                .output()
            {
                Ok(o) => {
                    let code = o.status.code().unwrap_or(-1);
                    let mut t = String::from_utf8_lossy(&o.stdout).into_owned();
                    let e = String::from_utf8_lossy(&o.stderr);
                    if !e.trim().is_empty() {
                        t.push_str(&e);
                    }
                    (code, t)
                }
                Err(e) => (-1, format!("failed to run tests: {e}")),
            };
            // The report only exists if the runner wrote one. A missing file
            // means no structured results, not a failed run — every runner this
            // doesn't know how to ask still works exactly as before.
            let run = std::fs::read_to_string(&report_for_thread)
                .map(|xml| crate::testrun::parse_junit(&xml, &root_for_parse))
                .unwrap_or_default();
            let _ = std::fs::remove_file(&report_for_thread);
            send((code, text, run));
        });
    }

    /// Start the autonomous "fix to green" loop.
    /// Run only the test the caret is in — a Pest description or a PHPUnit
    /// method — in the TDD panel, so a failure still has a line to jump to.
    pub fn run_test_at_cursor(&self) {
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let (Some(editor), Some(path)) = (buf.editor.get_untracked(), buf.file.path.clone()) else {
            return;
        };
        let text = buf.doc.text().to_string();
        let offset = editor.cursor.get_untracked().offset();
        let Some(name) = crate::tasks::test_at_cursor(&text, offset) else {
            Self::notify("No test at the caret (it/test(…) or a test_ method)");
            return;
        };
        self.run_single_test(&path, &name);
    }

    /// Run the test declared on `line` (0-based) of the open file at `path` —
    /// a click on its mark in the gutter. `false` when none is declared there.
    pub(crate) fn run_test_at_line(&self, path: &Path, line: usize) -> bool {
        let text = self.buffers.with_untracked(|bs| {
            bs.iter()
                .find(|b| b.file.path.as_deref() == Some(path))
                .map(|b| b.doc.text().to_string())
        });
        let Some(text) = text else {
            return false;
        };
        let Some((_, test)) = crate::tasks::tests_in(&text)
            .into_iter()
            .find(|(l, _)| *l == line)
        else {
            return false;
        };
        self.run_single_test(path, test.name());
        true
    }

    fn run_single_test(&self, path: &Path, name: &str) {
        let root = self.root.get_untracked();
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned();
        self.tdd_filter
            .set(Some(crate::tasks::single_test_args(&rel, name)));
        self.run_tests();
    }

    /// How each test declared in `text`, the file at `path`, last ended, by
    /// 0-based line.
    pub(crate) fn test_marks_for(
        &self,
        path: &Path,
        text: &str,
    ) -> HashMap<usize, crate::testrun::Outcome> {
        self.test_outcomes.with_untracked(|cases| {
            if cases.is_empty() {
                return HashMap::new();
            }
            let root = self.root.get_untracked();
            let rel = path.strip_prefix(&root).unwrap_or(path);
            crate::testrun::outcomes_in(text, path, rel, cases)
        })
    }

    /// Bring every open file's test marks up to date with the results kept.
    fn sync_test_marks(&self) {
        self.buffers.with_untracked(|bs| {
            for b in bs {
                let Some(path) = &b.file.path else { continue };
                let marks = self.test_marks_for(path, &b.doc.text().to_string());
                if *b.test_marks.borrow() != marks {
                    *b.test_marks.borrow_mut() = marks;
                    b.doc.cache_rev().update(|r| *r += 1);
                }
            }
        });
    }

    pub fn tdd_fix_to_green(&self) {
        self.tdd_iteration.set(0);
        self.tdd_loop.set(true);
        self.run_tests();
    }

    pub fn tdd_stop(&self) {
        self.tdd_loop.set(false);
    }
}
