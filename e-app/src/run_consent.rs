//! Asking before an agent runs something on the machine.
//!
//! `run` (a shell command) and `tinker` (PHP in the app) over `$E_EDITOR_SOCK`
//! execute as you. Until now they ran without a word, so an agent — or a
//! prompt injected into what it reads — could do anything a shell can. Each
//! now waits for a card: **Run once**, **Allow for this session** (what the
//! autonomous test-fix loop wants after its first command) or **Deny**.
//! `ask_before_agent_commands: false` in settings goes back to running
//! everything.
//!
//! Requests queue, one card at a time, so several in flight each get their
//! own answer. The agent can't click the card itself: the socket's UI
//! automation is off for agents.

use std::sync::mpsc::Sender;

use floem::peniko::Color;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use floem::views::{container, empty, label, stack, Decorators};
use floem::IntoView;
use serde_json::{json, Value};

use crate::state::AppState;
use crate::theme;

/// What the agent asked to run.
#[derive(Clone, Debug, PartialEq)]
pub enum AgentRun {
    /// A shell command, in `cwd`.
    Shell { command: String, cwd: String },
    /// PHP through `php artisan tinker`.
    Tinker { code: String },
}

impl AgentRun {
    fn title(&self) -> &'static str {
        match self {
            AgentRun::Shell { .. } => "Agent wants to run a command",
            AgentRun::Tinker { .. } => "Agent wants to run PHP in your app",
        }
    }

    fn subtitle(&self) -> String {
        match self {
            AgentRun::Shell { cwd, .. } => format!("in {cwd} — as you, with your shell"),
            AgentRun::Tinker { .. } => {
                "through php artisan tinker — with the app's database and services".into()
            }
        }
    }

    fn text(&self) -> &str {
        match self {
            AgentRun::Shell { command, .. } => command,
            AgentRun::Tinker { code } => code,
        }
    }
}

/// A request waiting for its card.
#[derive(Clone)]
pub struct RunConsent {
    pub run: AgentRun,
    pub reply: Sender<Value>,
}

/// Run `run` off the UI thread and reply with `{ok, code, stdout, stderr}`.
fn execute(state: &AppState, run: AgentRun, reply: Sender<Value>) {
    let root = state.root.get_untracked();
    std::thread::spawn(move || {
        let resp = match run {
            AgentRun::Shell { command, cwd } => crate::agent_sync::run_command(&command, &cwd),
            AgentRun::Tinker { code } => match crate::phptools::tinker(&root, &code) {
                Ok(o) => json!({
                    "ok": true,
                    "code": o.status.code(),
                    "stdout": String::from_utf8_lossy(&o.stdout),
                    "stderr": String::from_utf8_lossy(&o.stderr),
                }),
                Err(e) => json!({"ok": false, "error": e}),
            },
        };
        let _ = reply.send(resp);
    });
}

/// Whether `run` may go ahead without a card.
pub fn runs_unasked(ask_setting: bool, allowed_this_session: bool) -> bool {
    !ask_setting || allowed_this_session
}

impl AppState {
    /// An agent asked to run something: run it, or queue the card.
    pub(crate) fn agent_run(&self, run: AgentRun, reply: Sender<Value>) {
        if runs_unasked(
            self.settings.get_untracked().ask_before_agent_commands,
            self.agent.runs_allowed.get_untracked(),
        ) {
            execute(self, run, reply);
            return;
        }
        self.agent
            .run_consents
            .update(|q| q.push(RunConsent { run, reply }));
    }

    fn take_run_consent(&self) -> Option<RunConsent> {
        let mut first = None;
        self.agent.run_consents.update(|q| {
            if !q.is_empty() {
                first = Some(q.remove(0));
            }
        });
        first
    }

    /// Run the request on the card, this once.
    pub fn run_consent_once(&self) {
        if let Some(c) = self.take_run_consent() {
            execute(self, c.run, c.reply);
        }
    }

    /// Run it, and everything the agent asks to run until e restarts —
    /// including what is already queued.
    pub fn run_consent_session(&self) {
        self.agent.runs_allowed.set(true);
        while let Some(c) = self.take_run_consent() {
            execute(self, c.run, c.reply);
        }
    }

    /// Refuse the request on the card.
    pub fn run_consent_deny(&self) {
        if let Some(c) = self.take_run_consent() {
            let _ = c.reply.send(json!({
                "ok": false,
                "error": "denied by the user — ask them, or do it another way",
            }));
        }
    }
}

/// The card for the first queued request.
pub fn run_consent_dialog(state: AppState) -> impl IntoView {
    let first = move || {
        state
            .agent
            .run_consents
            .with(|q| q.first().map(|c| c.run.clone()))
    };
    let title = label(move || first().map(|r| r.title()).unwrap_or_default().to_string())
        .style(|s| s.font_size(14.0).font_bold().color(theme::fg()));
    let subtitle = label(move || {
        let more = state.agent.run_consents.with(Vec::len).saturating_sub(1);
        let tail = if more > 0 {
            format!(" · {more} more waiting")
        } else {
            String::new()
        };
        first().map(|r| r.subtitle() + &tail).unwrap_or_default()
    })
    .style(|s| s.font_size(12.0).color(theme::fg_dim()).margin_bottom(8.0));
    let text =
        label(move || first().map(|r| r.text().to_string()).unwrap_or_default()).style(|s| {
            theme::input_colors(s)
                .width_full()
                .max_height(260.0)
                .font_family(theme::mono_family())
                .font_size(12.0)
                .padding(10.0)
                .margin_bottom(12.0)
        });

    let button = |text: &'static str, primary: bool| {
        label(move || text.to_string()).style(move |s| {
            let s = s
                .padding_horiz(14.0)
                .height(30.0)
                .items_center()
                .border_radius(5.0)
                .font_size(12.0)
                .cursor(floem::style::CursorStyle::Pointer);
            if primary {
                s.background(theme::accent()).color(theme::on_accent())
            } else {
                s.border(1.0)
                    .border_color(theme::border())
                    .color(theme::fg())
                    .hover(|s| s.background(theme::bg_hover()))
            }
        })
    };
    let buttons = stack((
        empty().style(|s| s.flex_grow(1.0_f32)),
        button("Deny", false).on_click_stop(move |_| state.run_consent_deny()),
        button("Allow for this session", false).on_click_stop(move |_| state.run_consent_session()),
        button("Run once", true).on_click_stop(move |_| state.run_consent_once()),
    ))
    .style(|s| s.flex_row().gap(8.0).items_center().width_full());

    let card = stack((title, subtitle, text, buttons)).style(|s| {
        s.flex_col()
            .width(560.0)
            .padding(18.0)
            .border(1.0)
            .border_color(theme::border())
            .border_radius(10.0)
            .background(theme::bg())
    });

    container(card).style(move |s| {
        let s = s
            .absolute()
            .inset(0.0)
            .size_full()
            .items_center()
            .justify_center()
            .background(Color::from_rgba8(0, 0, 0, 140));
        if state.agent.run_consents.with(|q| !q.is_empty()) {
            s
        } else {
            s.hide()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::runs_unasked;

    #[test]
    fn a_card_unless_turned_off_or_allowed_for_the_session() {
        assert!(!runs_unasked(true, false));
        assert!(runs_unasked(true, true));
        assert!(runs_unasked(false, false));
    }
}
