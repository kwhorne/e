//! Driving the editor from the agent socket, so its UI can be tested the way a
//! person uses it — without sending keystrokes, which macOS may not allow.
//!
//! - `commands` / `command`: every named command (the palette's list), run by id.
//! - `code_actions` / `apply_code_action`: the `⌘.` menu at the caret.
//! - `dialog`: confirm or cancel the dialog that is showing.
//! - `cascade`: what the Cascade panel's controls do — send, steer, stop, new
//!   session, worktrees, a turn's Review and Undo.
//! - `wait_idle`: answer once the agent has finished its turn.
//! - `ui_state`: what is on screen — panels, the dialog, Cascade's transcript,
//!   the active editor — and the notifications shown lately.
//!
//! Everything here runs on the UI thread, as the controls themselves do.

use std::collections::VecDeque;
use std::sync::mpsc::Sender;
use std::sync::Mutex;
use std::time::Duration;

use e_agent::ChatItem;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use floem::views::editor::text::Document;
use serde_json::{json, Value};

use crate::state::AppState;

/// The notifications shown lately, newest last: a test reads what a person
/// would have seen in the corner of the screen.
static NOTICES: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());
const NOTICES_KEPT: usize = 20;

/// Remember a notification (called by [`AppState::notify`]).
pub fn remember_notice(message: &str) {
    if let Ok(mut q) = NOTICES.lock() {
        if q.len() == NOTICES_KEPT {
            q.pop_front();
        }
        q.push_back(message.to_string());
    }
}

fn notices() -> Vec<String> {
    NOTICES
        .lock()
        .map(|q| q.iter().cloned().collect())
        .unwrap_or_default()
}

/// Cut `s` to `n` characters, marking the cut.
fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n).collect::<String>())
    }
}

/// Run `then` on the UI thread once `ready` holds, or after `timeout`
/// (`then(false)`), checking every 50 ms.
fn when(ready: impl Fn() -> bool + 'static, timeout: Duration, then: impl FnOnce(bool) + 'static) {
    fn tick(ready: Box<dyn Fn() -> bool>, left: Duration, then: Box<dyn FnOnce(bool)>) {
        if ready() {
            then(true);
            return;
        }
        if left.is_zero() {
            then(false);
            return;
        }
        let step = Duration::from_millis(50).min(left);
        floem::action::exec_after(step, move |_| tick(ready, left - step, then));
    }
    tick(Box::new(ready), timeout, Box::new(then));
}

/// Cascade's transcript, item by item, as a test reads it.
fn transcript(state: AppState) -> Vec<Value> {
    state.agent.chat.with_untracked(|c| {
        c.items
            .iter()
            .enumerate()
            .map(|(i, it)| match it {
                ChatItem::User { text } => json!({"index": i, "kind": "user", "text": clip(text, 300)}),
                ChatItem::Assistant { text, streaming } => {
                    json!({"index": i, "kind": "assistant", "text": clip(text, 600), "streaming": streaming})
                }
                ChatItem::Reasoning { text, .. } => {
                    json!({"index": i, "kind": "reasoning", "text": clip(text, 200)})
                }
                ChatItem::Tool(tc) => json!({
                    "index": i,
                    "kind": "tool",
                    "name": tc.name,
                    "summary": clip(&tc.summary, 200),
                    "status": format!("{:?}", tc.status).to_lowercase(),
                    "result": tc.result.as_deref().map(|r| clip(r, 300)),
                }),
                ChatItem::Notice { text, error } => {
                    json!({"index": i, "kind": "notice", "text": text, "error": error})
                }
                ChatItem::Permission(p) => json!({
                    "index": i,
                    "kind": "permission",
                    "title": p.title,
                    "choices": p.choices.iter().map(|c| json!({"id": c.id, "name": c.name, "kind": c.kind})).collect::<Vec<_>>(),
                    "answered": p.answered,
                }),
                ChatItem::Receipt(r) => json!({
                    "index": i,
                    "kind": "receipt",
                    "summary": r.summary(),
                    "flags": r.flags,
                    "files": r.files.iter().map(|(p, a, d)| json!({"path": p, "added": a, "removed": d})).collect::<Vec<_>>(),
                    "undone": r.undone,
                }),
            })
            .collect()
    })
}

impl AppState {
    /// `ui_state`: what is on screen.
    pub(crate) fn ui_state(&self) -> Value {
        let panels: Vec<(&str, bool)> = vec![
            ("agent", self.agent.open.get_untracked()),
            ("review", self.review_open.get_untracked()),
            ("runtime", self.runtime_open.get_untracked()),
            ("grove", self.grove_panel_open.get_untracked()),
            ("migrations", self.migrations_open.get_untracked()),
            ("verify", self.verify_open.get_untracked()),
            ("contract", self.contract_open.get_untracked()),
            ("related", self.related_open.get_untracked()),
            ("relations", self.rel_open.get_untracked()),
            ("events", self.event_open.get_untracked()),
            ("schema_diff", self.schema_diff_open.get_untracked()),
            ("undo", self.undo_open.get_untracked()),
            ("map", self.map_open.get_untracked()),
            ("semantic", self.sem_open.get_untracked()),
            ("tinker", self.tinker_open.get_untracked()),
            ("terminal", self.terminal_open.get_untracked()),
            ("tdd", self.tdd_open.get_untracked()),
            ("debug", self.debug_open.get_untracked()),
            ("settings", self.settings_open.get_untracked()),
            ("git", self.git_panel_open.get_untracked()),
            ("goto_file", self.palette_open.get_untracked()),
            ("command_palette", self.cmd.open.get_untracked()),
        ];
        let open: Vec<&str> = panels.iter().filter(|(_, o)| *o).map(|(n, _)| *n).collect();

        // The dialog showing, if any: a plan (a rename, a field carried
        // through) or a yes/no question.
        let dialog = self
            .rename_plan
            .with_untracked(|p| {
                p.as_ref().map(|p| {
                    json!({
                        "kind": "plan",
                        "title": p.title.clone().unwrap_or_else(|| format!("Rename “{}” to “{}”?", p.old_name, p.new_name)),
                        "summary": p.summary(),
                        "button": p.verb.clone().unwrap_or_else(|| "Rename".into()),
                        "files": p.files.iter().map(|f| json!({
                            "path": f.path,
                            "sites": f.sites.iter().map(|s| json!({"line": s.line + 1, "before": s.before, "after": s.after})).collect::<Vec<_>>(),
                        })).collect::<Vec<_>>(),
                        "creates": p.creates.iter().map(|(path, _)| path.clone()).collect::<Vec<_>>(),
                        "notes": p.notes,
                    })
                })
            })
            .or_else(|| {
                self.confirm.with_untracked(|c| {
                    c.as_ref().map(|c| {
                        json!({"kind": "confirm", "title": c.title, "detail": c.detail, "button": c.action})
                    })
                })
            });

        let code_actions = self.code_actions_open.get_untracked().then(|| {
            self.code_actions.with_untracked(|l| {
                l.iter()
                    .enumerate()
                    .map(|(i, a)| json!({"index": i, "title": a.title, "from": a.server}))
                    .collect::<Vec<_>>()
            })
        });

        let editor = self.active_buffer().map(|b| {
            let (line, col) = b
                .editor
                .get_untracked()
                .map(|ed| {
                    let (l, c) = ed.offset_to_line_col(ed.cursor.get_untracked().offset());
                    (l + 1, c + 1)
                })
                .unwrap_or((0, 0));
            // What the gutter and the line ends show, by 1-based line.
            let mut breakpoints: Vec<usize> = b.bp_marks.borrow().iter().map(|l| l + 1).collect();
            breakpoints.sort_unstable();
            let mut tests: Vec<(usize, String)> = b
                .test_marks
                .borrow()
                .iter()
                .map(|(l, o)| (l + 1, format!("{o:?}").to_lowercase()))
                .collect();
            tests.sort();
            let changes: Vec<(usize, String)> = b
                .git_marks
                .borrow()
                .iter()
                .enumerate()
                .filter_map(|(l, m)| Some((l + 1, format!("{:?}", (*m)?).to_lowercase())))
                .collect();
            let exceptions: Vec<(usize, String)> = b
                .alerts
                .get_untracked()
                .into_iter()
                .map(|(l, t)| (l + 1, t))
                .collect();
            json!({
                "path": b.file.path,
                "line": line,
                "col": col,
                "dirty": b.dirty.get_untracked(),
                "length": b.doc.text().len(),
                "breakpoints": breakpoints,
                "tests": tests,
                "changes": changes,
                "exceptions": exceptions,
                "evals": b.evals.with_untracked(|m| m
                    .iter()
                    .map(|e| (e.line + 1, e.label.clone()))
                    .collect::<Vec<_>>()),
            })
        });

        let tests_on_save = self.save_run.get_untracked().map(|r| match r {
            crate::test_on_save::SaveRun::Running { files } => json!({"running": files}),
            crate::test_on_save::SaveRun::Done { passed, failed } => {
                json!({"passed": passed, "failed": failed})
            }
            crate::test_on_save::SaveRun::TooWide { files } => json!({"too_wide": files}),
        });
        let hunk_peek = self.hunk_peek.get_untracked().map(|p| {
            json!({
                "line": p.line + 1,
                "old_lines": p.old_lines,
                "staged": p.staged,
            })
        });

        let review = self.review_open.get_untracked().then(|| {
            self.review_changeset.with_untracked(|cs| {
                json!({
                    "turn": self.review_turn.get_untracked().is_some(),
                    "summary": cs.summary(),
                    "files": cs.files.iter().map(|f| f.path.clone()).collect::<Vec<_>>(),
                    "flags": self.review_flags.with_untracked(|f| f
                        .iter()
                        .map(|f| json!({
                            "path": f.path,
                            "line": f.line,
                            "severity": format!("{:?}", f.severity),
                            "code": f.code,
                            "message": f.message,
                        }))
                        .collect::<Vec<_>>()),
                    "ship": format!("{:?}", floem::reactive::untrack(|| self.review_ship_verdict()).readiness),
                })
            })
        });

        let cascade = self.agent.chat.with_untracked(|c| {
            json!({
                "running": c.running,
                "agent": self.cascade_agent_name(),
                "model": c.model.as_ref().map(|m| format!("{}/{}", m.provider, m.id)),
                "usage": c.usage_label(),
                "steering": c.steering,
                "follow_up": c.follow_up,
                "worktree": self.agent.worktree.get_untracked().map(|t| json!({"branch": t.branch, "url": t.url, "path": t.path})),
            })
        });

        json!({
            "ok": true,
            "open": open,
            "dialog": dialog,
            "code_actions": code_actions,
            "editor": editor,
            "tests_on_save": tests_on_save,
            "migration_drift": self.migration_drift.get_untracked().map(|d| json!({
                "pending": d.pending,
                "foreign": d.foreign,
                "summary": d.summary(),
            })),
            "hunk_peek": hunk_peek,
            "review": review,
            "cascade": cascade,
            "transcript": transcript(*self),
            "notifications": notices(),
        })
    }

    /// `code_actions`: open the `⌘.` menu at the caret and answer with what it
    /// offers, once the language servers have had their say.
    pub(crate) fn agent_code_actions(&self, reply: Sender<Value>) {
        self.code_actions_open.set(false);
        self.code_actions.set(Vec::new());
        let before = notices().len();
        self.request_code_actions();
        let open = self.code_actions_open;
        let list = self.code_actions;
        when(
            move || open.get_untracked(),
            Duration::from_secs(8),
            move |shown| {
                let actions: Vec<Value> = list.with_untracked(|l| {
                    l.iter()
                        .enumerate()
                        .map(|(i, a)| json!({"index": i, "title": a.title, "from": a.server}))
                        .collect()
                });
                let said: Vec<String> = notices().into_iter().skip(before).collect();
                let _ = reply.send(
                    json!({"ok": true, "shown": shown, "actions": actions, "notifications": said}),
                );
            },
        );
    }

    /// `cascade`: what Cascade's controls do.
    pub(crate) fn agent_cascade(&self, req: &Value) -> Value {
        let s = |k: &str| req.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let index = req.get("index").and_then(Value::as_u64).map(|i| i as usize);
        if !self.agent.open.get_untracked() {
            self.agent.open.set(true);
        }
        match s("action").as_str() {
            "send" | "steer" | "next" => {
                let text = s("text");
                if text.trim().is_empty() {
                    return json!({"ok": false, "error": "missing text"});
                }
                self.send_native_prompt_as(&text, s("action") == "next");
            }
            "stop" => self.native_agent_abort(),
            "new_session" => self.native_agent_new_session(),
            "compact" => self.cascade_compact(),
            "new_worktree" => self.cascade_new_worktree(),
            "back_to_project" => self.cascade_enter_worktree(None),
            "continue_in" | "take_down" => {
                let branch = s("branch");
                let root = self.root.get_untracked();
                let Some(t) = crate::agent_worktree::worktrees_of(&root)
                    .into_iter()
                    .find(|t| t.branch == branch)
                else {
                    return json!({"ok": false, "error": format!("no worktree {branch} for this project")});
                };
                if s("action") == "take_down" {
                    self.cascade_take_down(t);
                } else {
                    self.cascade_enter_worktree(Some(t));
                }
            }
            "answer" => {
                let Some(i) = index else {
                    return json!({"ok": false, "error": "missing index (of the permission request in the transcript)"});
                };
                return match self.answer_permission(i, &s("choice")) {
                    Ok(name) => json!({"ok": true, "answered": name}),
                    Err(e) => json!({"ok": false, "error": e}),
                };
            }
            "agent" => self.set_cascade_agent(&s("id")),
            "review_turn" | "undo_turn" => {
                let Some(i) = index else {
                    return json!({"ok": false, "error": "missing index (of the receipt in the transcript)"});
                };
                if s("action") == "review_turn" {
                    self.turn_review(i);
                } else {
                    self.turn_undo(i);
                }
            }
            other => {
                return json!({
                    "ok": false,
                    "error": format!("unknown action {other:?}"),
                    "actions": ["send", "steer", "next", "stop", "new_session", "compact", "new_worktree",
                                "back_to_project", "continue_in", "take_down", "review_turn", "undo_turn",
                                "answer", "agent"],
                })
            }
        }
        json!({"ok": true})
    }

    /// `wait_idle`: answer once the agent is not working, or after the
    /// timeout, with how the transcript ends.
    pub(crate) fn agent_wait_idle(&self, timeout_ms: u64, reply: Sender<Value>) {
        let chat = self.agent.chat;
        let app = *self;
        // A turn just sent may not have flipped `running` on yet: give it a
        // moment before taking idle for an answer.
        floem::action::exec_after(Duration::from_millis(300), move |_| {
            when(
                move || !chat.with_untracked(|c| c.running),
                Duration::from_millis(timeout_ms),
                move |idle| {
                    let tail: Vec<Value> =
                        transcript(app).into_iter().rev().take(4).rev().collect();
                    let _ = reply.send(json!({"ok": true, "idle": idle, "tail": tail}));
                },
            )
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notifications_are_kept_newest_last_and_bounded() {
        for i in 0..(NOTICES_KEPT + 5) {
            remember_notice(&format!("n{i}"));
        }
        let kept = notices();
        assert_eq!(kept.len(), NOTICES_KEPT);
        assert_eq!(
            kept.last().map(String::as_str),
            Some(&*format!("n{}", NOTICES_KEPT + 4))
        );
    }

    #[test]
    fn long_text_is_clipped_with_a_mark() {
        assert_eq!(clip("abcdef", 3), "abc…");
        assert_eq!(clip("abc", 3), "abc");
    }
}
