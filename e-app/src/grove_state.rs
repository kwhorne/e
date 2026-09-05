//! Grove's mail-catcher and webhook hub, as a panel in `e`.
//!
//! Grove captures every mail the app sends (its SMTP server) and every webhook
//! delivered to `/__grove/hooks/<bucket>`. This module owns the panel state and
//! the `AppState` methods that list, open, re-deliver and hand entries to the
//! agent; the view lives in [`crate::grove_view`].

use floem::reactive::{SignalGet, SignalUpdate, SignalWith};

use crate::grove;
use crate::state::AppState;

/// Which list the Grove panel shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GroveTab {
    Mail,
    Hooks,
}

impl AppState {
    /// Open the Grove panel on `tab` (or close it if it's already showing that tab).
    pub fn toggle_grove_panel(&self, tab: GroveTab) {
        let open = self.grove_panel_open.get_untracked();
        let same = self.grove_tab.get_untracked() == tab;
        if open && same {
            self.grove_panel_open.set(false);
            return;
        }
        self.grove_tab.set(tab);
        self.grove_selected.set(None);
        self.grove_detail.set(String::new());
        self.grove_panel_open.set(true);
        self.refresh_grove_panel();
    }

    /// Re-list mail and webhooks. Called on the idle tick while the panel is open.
    pub fn refresh_grove_panel(&self) {
        if self.grove_refreshing.get_untracked() {
            return;
        }
        if self.grove_site().is_none() && !grove::available() {
            return;
        }
        self.grove_refreshing.set(true);
        let mail_sig = self.grove_mail;
        let hooks_sig = self.grove_hooks;
        let busy = self.grove_refreshing;
        self.spawn_bg(
            move || (grove::mail(), grove::hooks(50)),
            move |(mail, hooks): (Vec<grove::Email>, Vec<grove::Request>)| {
                busy.set(false);
                if mail_sig.with_untracked(|m| *m != mail) {
                    mail_sig.set(mail);
                }
                if hooks_sig.with_untracked(|h| *h != hooks) {
                    hooks_sig.set(hooks);
                }
            },
        );
    }

    /// Select an entry and load what there is to show for it.
    pub fn grove_select(&self, tab: GroveTab, id: u64) {
        self.grove_selected.set(Some((tab, id)));
        match tab {
            GroveTab::Mail => {
                let detail = self.grove_detail;
                self.grove_detail.set("Loading…".into());
                self.spawn_bg(
                    move || grove::mail_show(id),
                    move |body: Option<grove::EmailBody>| {
                        detail.set(
                            body.map(|b| b.readable())
                                .unwrap_or_else(|| "Grove no longer has this message.".into()),
                        );
                    },
                );
            }
            GroveTab::Hooks => {
                // The CLI lists webhooks but doesn't print one's body; what we
                // can show is the entry itself, and what we can do is re-deliver.
                let text = self
                    .grove_hooks
                    .with_untracked(|h| h.iter().find(|r| r.id == id).cloned())
                    .map(|r| {
                        format!(
                            "{} {}\n{} · {} ms · answered {}\n\nRe-deliver sends this exact request \
                             to your handler at the same path on {}.",
                            r.method,
                            r.path,
                            r.time,
                            r.duration_ms,
                            r.status,
                            self.app_base()
                        )
                    })
                    .unwrap_or_default();
                self.grove_detail.set(text);
            }
        }
    }

    /// Re-deliver a captured webhook to the app's handler: same path, on the
    /// app's own URL rather than Grove's capture bucket.
    pub fn grove_replay_hook(&self, id: u64) {
        let Some(hook) = self
            .grove_hooks
            .with_untracked(|h| h.iter().find(|r| r.id == id).cloned())
        else {
            return;
        };
        let path = hook
            .path
            .strip_prefix("/__grove/hooks")
            .unwrap_or(&hook.path)
            .to_string();
        let to = format!("{}{}", self.app_base(), path);
        let shown = to.clone();
        self.spawn_bg(
            move || grove::hook_replay(id, &to),
            move |ok: bool| {
                if ok {
                    Self::notify(&format!("Webhook re-delivered to {shown}"));
                } else {
                    Self::notify("Could not re-deliver the webhook (see `grove hooks`)");
                }
            },
        );
    }

    /// Hand the selected mail or webhook to the agent.
    pub fn grove_to_agent(&self) {
        let Some((tab, id)) = self.grove_selected.get_untracked() else {
            return;
        };
        let detail = self.grove_detail.get_untracked();
        let prompt = match tab {
            GroveTab::Mail => {
                let head = self
                    .grove_mail
                    .with_untracked(|m| m.iter().find(|e| e.id == id).cloned())
                    .map(|e| {
                        format!(
                            "Subject: {}\nFrom: {}\nTo: {}",
                            e.subject,
                            e.from,
                            e.to.join(", ")
                        )
                    })
                    .unwrap_or_default();
                format!(
                    "The app sent this email (captured by Grove's mail-catcher). Check that its \
                     content and recipients are right, and find where in the code it is built.\n\n\
                     {head}\n\n{detail}"
                )
            }
            GroveTab::Hooks => format!(
                "This webhook was delivered to the app (captured by Grove). Find the handler for \
                 its path and check that it copes with this delivery.\n\n{detail}"
            ),
        };
        self.send_to_agent(&prompt);
    }
}
