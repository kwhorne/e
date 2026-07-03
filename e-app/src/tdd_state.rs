//! Autonomous TDD loop: run the test suite, and (optionally) drive the agent to
//! fix failures until green. Extracted from the former `state.rs` god-module.

use floem::ext_event::create_ext_action;
use floem::reactive::{SignalGet, SignalUpdate};

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
        // Safety cap so an unproductive loop can't run forever.
        if self.tdd_loop.get_untracked() && self.tdd_iteration.get_untracked() >= 15 {
            self.tdd_loop.set(false);
            self.tdd_output
                .update(|o| o.push_str("\n\n[stopped: reached 15 iterations]"));
            return;
        }
        self.tdd_open.set(true);
        self.tdd_status.set(TddStatus::Running);
        let state = *self;
        let send = create_ext_action(self.cx, move |(code, text): (i32, String)| {
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
        });
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
            send((code, text));
        });
    }

    /// Start the autonomous "fix to green" loop.
    pub fn tdd_fix_to_green(&self) {
        self.tdd_iteration.set(0);
        self.tdd_loop.set(true);
        self.run_tests();
    }

    pub fn tdd_stop(&self) {
        self.tdd_loop.set(false);
    }
}
