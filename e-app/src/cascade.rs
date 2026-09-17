//! Elyra Cascade: the agent panel as a chat, the way Devin Local and Windsurf's
//! Cascade draw theirs — a session tab, a quiet empty state, the transcript,
//! and a composer card carrying the mode, the model and the send button. Elyra
//! runs headless underneath (`elyra --mode rpc`), so every model it knows is on
//! offer, with the API keys from Settings handed to it. Off by default; the
//! terminal panel stays the alternative (Settings → Agents, or the ⋯ menu).

use std::path::{Path, PathBuf};

use floem::menu::{Menu, MenuItem};
use floem::peniko::Color;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use floem::unit::Pct;
use floem::views::{dyn_container, empty, label, stack, Decorators};
use floem::IntoView;
use serde_json::Value;

use floem::views::editor::core::cursor::{Cursor, CursorMode};
use floem::views::editor::core::editor::EditType;
use floem::views::editor::core::selection::Selection;
use floem::views::editor::text::Document;

use crate::agent_native::{composer_input, transcript};
use crate::config;
use crate::secrets::{self, Provider};
use crate::state::AppState;
use crate::theme;

/// Quote one argument for the login shell the agent is run through.
pub fn shell_quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | ':' | '@'))
    {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// elyra's thinking levels, in order.
pub const THINKING_LEVELS: &[&str] = &["off", "minimal", "low", "medium", "high", "xhigh"];

/// A human name for one of elyra's provider ids.
pub fn provider_label(id: &str) -> String {
    match Provider::from_elyra_id(id) {
        Some(p) => p.label().to_string(),
        None => {
            let mut c = id.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        }
    }
}

// ---- Sessions ----------------------------------------------------------------

/// One earlier conversation in this project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionInfo {
    pub path: PathBuf,
    pub title: String,
    /// `2026-07-03 05:04`, from the session header.
    pub when: String,
}

/// elyra keeps a project's sessions under
/// `~/.elyra/agent/sessions/--<cwd with / as ->--/`.
pub fn sessions_dir(cwd: &Path) -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let name = format!("-{}--", cwd.to_string_lossy().replace('/', "-"));
    Some(
        PathBuf::from(home)
            .join(".elyra")
            .join("agent")
            .join("sessions")
            .join(name),
    )
}

/// The newest `limit` sessions in `dir`, titled by their name or first prompt.
pub fn list_sessions(dir: &Path, limit: usize) -> Vec<SessionInfo> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = read
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|p| {
            let modified = std::fs::metadata(&p).ok()?.modified().ok()?;
            Some((modified, p))
        })
        .collect();
    files.sort_by_key(|f| std::cmp::Reverse(f.0));
    files
        .into_iter()
        .take(limit)
        .filter_map(|(_, path)| {
            let text = std::fs::read_to_string(&path).ok()?;
            let (title, when) = session_summary(&text);
            let title = title?;
            Some(SessionInfo { path, title, when })
        })
        .collect()
}

/// The title (a set name, else the first user message) and the start time of a
/// session file. `None` title for a session nothing was said in.
pub fn session_summary(text: &str) -> (Option<String>, String) {
    let mut first_user: Option<String> = None;
    let mut name: Option<String> = None;
    let mut when = String::new();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        match v.get("type").and_then(Value::as_str) {
            Some("session") => {
                if let Some(ts) = v.get("timestamp").and_then(Value::as_str) {
                    when = ts.chars().take(16).collect::<String>().replace('T', " ");
                }
            }
            Some("session_info") => {
                if let Some(n) = v.get("name").and_then(Value::as_str) {
                    if !n.trim().is_empty() {
                        name = Some(n.trim().to_string());
                    }
                }
            }
            Some("message") if first_user.is_none() => {
                let msg = v.get("message").unwrap_or(&v);
                if msg.get("role").and_then(Value::as_str) == Some("user") {
                    let t = content_text(msg.get("content"));
                    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
                    if !t.is_empty() {
                        first_user = Some(truncate(&t, 60));
                    }
                }
            }
            _ => {}
        }
    }
    (name.or(first_user), when)
}

fn content_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    }
}

fn truncate(s: &str, limit: usize) -> String {
    if s.chars().count() <= limit {
        return s.to_string();
    }
    let kept: String = s.chars().take(limit - 1).collect();
    format!("{}…", kept.trim_end())
}

// ---- State ---------------------------------------------------------------------

impl AppState {
    /// Turn Cascade on or off (Settings, the ⋯ menu, the palette). Restarts the
    /// agent so the panel changes at once.
    pub fn set_cascade(&self, on: bool) {
        self.settings.update(|s| s.cascade = on);
        config::set_bool("cascade", on);
        self.restart_agent();
        self.agent.focus_pulse.update(|x| *x += 1);
    }

    pub fn toggle_cascade(&self) {
        let on = !self.settings.get_untracked().cascade;
        self.set_cascade(on);
        if !self.agent.open.get_untracked() {
            self.toggle_agent();
        }
    }

    /// `code` or `ask`. Remembered across sessions.
    pub fn set_cascade_mode(&self, mode: &str) {
        let mode = if mode == "ask" { "ask" } else { "code" };
        self.agent.mode.set(mode.to_string());
        self.settings.update(|s| s.cascade_mode = mode.to_string());
        config::set_str("cascade_mode", mode);
    }

    /// Switch Elyra to `provider/model_id` now and remember it for next time.
    pub fn cascade_set_model(&self, provider: &str, model_id: &str) {
        let spec = format!("{provider}/{model_id}");
        self.settings.update(|s| s.cascade_model = spec.clone());
        config::set_str("cascade_model", &spec);
        if let Some(client) = self.agent.native_client.get_untracked() {
            if let Err(e) = client.set_model(provider, model_id) {
                eprintln!("e: cascade: set_model failed: {e:#}");
            }
            let _ = client.get_state();
        }
    }

    /// Set Elyra's thinking level now and remember it.
    pub fn cascade_set_thinking(&self, level: &str) {
        self.settings
            .update(|s| s.cascade_thinking = level.to_string());
        config::set_str("cascade_thinking", level);
        self.agent.chat.update(|c| c.thinking = level.to_string());
        if let Some(client) = self.agent.native_client.get_untracked() {
            if let Err(e) = client.set_thinking_level(level) {
                eprintln!("e: cascade: set_thinking_level failed: {e:#}");
            }
        }
    }

    /// Ask Elyra again for its models and state.
    pub fn cascade_refresh_models(&self) {
        if let Some(client) = self.agent.native_client.get_untracked() {
            let _ = client.get_available_models();
            let _ = client.get_state();
        }
    }

    pub fn cascade_compact(&self) {
        if let Some(client) = self.agent.native_client.get_untracked() {
            let _ = client.compact();
        }
    }

    /// Reopen an earlier session: restart Elyra on its file and redraw the
    /// conversation from `get_messages`.
    pub fn cascade_resume_session(&self, path: PathBuf) {
        if let Some(client) = self.agent.native_client.get_untracked() {
            client.shutdown();
        }
        self.agent.native_client.set(None);
        self.start_native_agent_with(Some(&path));
        self.agent.focus_pulse.update(|x| *x += 1);
    }

    /// Type `text` at the end of the composer without sending, and focus it.
    /// Deferred: the document must not be edited from inside a menu or key
    /// event that is still borrowing the editor.
    pub fn cascade_insert(&self, text: &str) {
        if !self.agent.open.get_untracked() {
            self.agent.open.set(true);
        }
        let Some(doc) = self.agent.composer_doc.get_untracked() else {
            self.agent.composer.update(|c| c.push_str(text));
            return;
        };
        let text = text.to_string();
        let st = *self;
        floem::action::exec_after(std::time::Duration::ZERO, move |_| {
            let current = doc.text().to_string();
            let len = current.len();
            let sep = if len > 0 && !current.ends_with([' ', '\n']) {
                " "
            } else {
                ""
            };
            let insert = format!("{sep}{text}");
            doc.edit_single(Selection::caret(len), &insert, EditType::InsertChars);
            if let Some(editor) = st.agent.composer_editor.get_untracked() {
                editor.cursor.set(Cursor::new(
                    CursorMode::Insert(Selection::caret(len + insert.len())),
                    None,
                    None,
                ));
            }
            st.agent.focus_pulse.update(|x| *x += 1);
        });
    }

    fn rel_of(&self, path: &Path) -> String {
        let root = self.root.get_untracked();
        path.strip_prefix(&root)
            .unwrap_or(path)
            .display()
            .to_string()
    }

    /// `＋ → Active file`: an `@path` reference the agent reads on its own.
    pub fn cascade_attach_active_file(&self) {
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let Some(path) = buf.file.path.clone() else {
            Self::notify("Save the file first so the agent can read it");
            return;
        };
        let rel = self.rel_of(&path);
        self.cascade_insert(&format!("@{rel} "));
    }

    /// `＋ → Selection`: the file and line range under the selection.
    pub fn cascade_attach_selection(&self) {
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let Some(path) = buf.file.path.clone() else {
            Self::notify("Save the file first so the agent can read it");
            return;
        };
        let rel = self.rel_of(&path);
        let doc_text = buf.doc.text().to_string();
        let (start, end) = buf
            .editor
            .get_untracked()
            .and_then(|editor| {
                let cursor = editor.cursor.get_untracked();
                if let CursorMode::Insert(sel) = &cursor.mode {
                    sel.regions()
                        .iter()
                        .find(|r| r.min() != r.max())
                        .or_else(|| sel.regions().first())
                        .map(|r| (r.min(), r.max()))
                } else {
                    None
                }
            })
            .unwrap_or((0, 0));
        let msg = crate::agent_links::format_selection_context(
            &rel,
            crate::agent_links::line_of(&doc_text, start),
            crate::agent_links::line_of(&doc_text, end),
        );
        self.cascade_insert(&msg);
    }

    /// `＋ → All open files`: every open, saved file as an `@path`.
    pub fn cascade_attach_open_files(&self) {
        let refs: Vec<String> = self.buffers.with_untracked(|bs| {
            bs.iter()
                .filter_map(|b| b.file.path.as_ref().map(|p| format!("@{}", self.rel_of(p))))
                .collect()
        });
        if refs.is_empty() {
            Self::notify("No saved files are open");
            return;
        }
        self.cascade_insert(&format!("{} ", refs.join(" ")));
    }
}

// ---- The panel -----------------------------------------------------------------

fn icon_button(glyph: &'static str) -> floem::views::Label {
    label(move || glyph.to_string()).style(|s| {
        s.width(30.0)
            .height(30.0)
            .items_center()
            .justify_center()
            .font_size(15.0)
            .border_radius(6.0)
            .color(theme::fg_dim())
            .cursor(floem::style::CursorStyle::Pointer)
            .hover(|s| s.background(theme::bg_hover()).color(theme::fg()))
    })
}

fn chip_style(s: floem::style::Style) -> floem::style::Style {
    s.padding_horiz(8.0)
        .height(26.0)
        .items_center()
        .border_radius(6.0)
        .font_size(12.0)
        .color(theme::fg_dim())
        .cursor(floem::style::CursorStyle::Pointer)
        .hover(|s| s.background(theme::bg_hover()).color(theme::fg()))
}

/// The session tab and the ＋ ◷ ⋯ ✕ controls.
fn header(state: AppState) -> impl IntoView {
    let tab = label(move || {
        state.agent.chat.with(|c| {
            if c.items.is_empty() {
                "Elyra Cascade".to_string()
            } else {
                "Elyra Cascade Session".to_string()
            }
        })
    })
    .style(|s| {
        s.padding_horiz(14.0)
            .height_full()
            .items_center()
            .font_size(13.0)
            .color(theme::fg())
            .border_top(2.0)
            .border_color(theme::accent())
            .background(theme::bg())
    });

    let new_session = icon_button("＋").on_click_stop(move |_| state.native_agent_new_session());

    let history = icon_button("◷").popout_menu(move || {
        let root = state.root.get_untracked();
        let sessions = sessions_dir(&root)
            .map(|d| list_sessions(&d, 15))
            .unwrap_or_default();
        let mut menu = Menu::new("");
        if sessions.is_empty() {
            return menu.entry(MenuItem::new("No earlier sessions in this project").enabled(false));
        }
        let current = state
            .agent
            .chat
            .with_untracked(|c| c.session_file.clone().map(PathBuf::from));
        for s in sessions {
            let mark = if current.as_ref() == Some(&s.path) {
                "● "
            } else {
                "   "
            };
            let path = s.path.clone();
            menu = menu.entry(
                MenuItem::new(format!("{mark}{}   {}", s.title, s.when))
                    .action(move || state.cascade_resume_session(path.clone())),
            );
        }
        menu
    });

    let more = icon_button("⋯").popout_menu(move || {
        Menu::new("")
            .entry(MenuItem::new("New session").action(move || state.native_agent_new_session()))
            .entry(MenuItem::new("Compact context").action(move || state.cascade_compact()))
            .entry(MenuItem::new("Restart Elyra").action(move || state.restart_agent()))
            .separator()
            .entry(
                MenuItem::new("Use the terminal panel instead")
                    .action(move || state.set_cascade(false)),
            )
            .entry(MenuItem::new("Settings…  (⌘,)").action(move || state.open_settings()))
    });

    let close = icon_button("✕").on_click_stop(move |_| state.agent.open.set(false));
    let spacer = empty().style(|s| s.flex_grow(1.0_f32));

    stack((tab, spacer, new_session, history, more, close)).style(|s| {
        s.items_center()
            .width_full()
            .height(38.0)
            .padding_right(6.0)
            .gap(2.0)
            .background(theme::bg_panel())
            .border_bottom(1.0)
            .border_color(theme::border())
    })
}

/// The empty state: a mark, a name, an invitation — and, before any key is
/// stored, where to add one.
fn hero(state: AppState) -> impl IntoView {
    let line = || {
        if theme::is_dark() {
            Color::from_rgba8(0xff, 0xff, 0xff, 0x0e)
        } else {
            Color::from_rgba8(0x00, 0x00, 0x00, 0x0c)
        }
    };
    let ring = move |size: f64, left: Option<f64>, right: Option<f64>, top: Pct| {
        empty().style(move |s| {
            let s = s
                .absolute()
                .width(size)
                .height(size)
                .border(1.0)
                .border_color(line())
                .border_radius(size / 2.0)
                .inset_top(top);
            let s = match left {
                Some(l) => s.inset_left(l),
                None => s,
            };
            match right {
                Some(r) => s.inset_right(r),
                None => s,
            }
        })
    };
    let rings = (
        ring(360.0, Some(-200.0), None, Pct(6.0)),
        ring(260.0, None, Some(-140.0), Pct(38.0)),
        ring(300.0, Some(-120.0), None, Pct(66.0)),
        ring(200.0, None, Some(-60.0), Pct(84.0)),
    );

    let mark = label(|| "⬡".to_string()).style(|s| s.font_size(46.0).color(theme::fg()));
    let title = label(|| "Elyra Cascade".to_string()).style(|s| {
        s.font_size(21.0)
            .font_bold()
            .color(theme::fg())
            .margin_top(10.0)
    });
    let sub = label(|| "Describe your task to Elyra".to_string())
        .style(|s| s.font_size(14.0).color(theme::fg_dim()).margin_top(4.0));
    // Decided once per build of the empty state rather than in the label's
    // closure, which floem re-runs: each check spawns the Keychain tool.
    let needs_key = !(secrets::any_configured() || secrets::elyra_has_credentials());
    if std::env::var_os("E_DEBUG_CASCADE").is_some() {
        eprintln!("e: cascade: a key is needed: {needs_key}");
    }
    let hint = label(move || {
        if needs_key {
            "Add an API key in Settings → Agents to get started".to_string()
        } else {
            String::new()
        }
    })
    .style(|s| {
        s.font_size(12.0)
            .color(theme::accent())
            .margin_top(18.0)
            .cursor(floem::style::CursorStyle::Pointer)
    })
    .on_click_stop(move |_| state.open_settings());

    let column = stack((mark, title, sub, hint)).style(|s| s.flex_col().items_center());

    stack((rings.0, rings.1, rings.2, rings.3, column)).style(|s| {
        s.size_full()
            .flex_grow(1.0_f32)
            .min_height(0.0)
            .items_center()
            .justify_center()
            .background(theme::bg())
    })
}

/// The transcript, or the empty state before the first message.
fn body(state: AppState) -> impl IntoView {
    dyn_container(
        move || state.agent.chat.with(|c| c.items.is_empty()),
        move |is_empty| {
            if is_empty {
                hero(state).into_any()
            } else {
                transcript(state).into_any()
            }
        },
    )
    .style(|s| {
        s.flex_col()
            .width_full()
            .flex_grow(1.0_f32)
            .flex_basis(0.0)
            .min_height(0.0)
    })
}

/// `Claude Opus 5 · high`, or a prompt to pick one.
fn model_label(state: AppState) -> String {
    state.agent.chat.with(|c| match &c.model {
        // Without any credentials elyra reports a placeholder model.
        Some(m) if m.name.is_empty() || m.name == "unknown" => "Choose a model  ▾".to_string(),
        Some(m) if m.reasoning && !c.thinking.is_empty() => {
            format!("{} · {}  ▾", m.name, c.thinking)
        }
        Some(m) => format!("{}  ▾", m.name),
        None => "Model  ▾".to_string(),
    })
}

/// Every model elyra offers, grouped by provider, plus the thinking levels.
fn model_menu(state: AppState) -> Menu {
    let (models, current, thinking) = state
        .agent
        .chat
        .with_untracked(|c| (c.models.clone(), c.model.clone(), c.thinking.clone()));
    let mut menu = Menu::new("");
    if models.is_empty() {
        menu = menu.entry(MenuItem::new("Asking Elyra for its models…").enabled(false));
        // Ask again: the first request may have raced the agent's start.
        state.cascade_refresh_models();
    }
    let mut providers: Vec<String> = Vec::new();
    for m in &models {
        if !providers.contains(&m.provider) {
            providers.push(m.provider.clone());
        }
    }
    for provider in providers {
        let mut sub = Menu::new(provider_label(&provider));
        for m in models.iter().filter(|m| m.provider == provider) {
            let is_current = current
                .as_ref()
                .is_some_and(|c| c.id == m.id && c.provider == m.provider);
            let mark = if is_current { "● " } else { "   " };
            let (p, id) = (m.provider.clone(), m.id.clone());
            sub = sub.entry(
                MenuItem::new(format!("{mark}{}", m.name))
                    .action(move || state.cascade_set_model(&p, &id)),
            );
        }
        menu = menu.entry(sub);
    }
    if current.as_ref().is_some_and(|m| m.reasoning) {
        let mut think = Menu::new("Thinking");
        for level in THINKING_LEVELS {
            let mark = if *level == thinking { "● " } else { "   " };
            let level = level.to_string();
            think = think.entry(
                MenuItem::new(format!("{mark}{level}"))
                    .action(move || state.cascade_set_thinking(&level)),
            );
        }
        menu = menu.separator().entry(think);
    }
    menu
}

/// The composer card: the input, then ＋ · mode · model on the left and the
/// agent badge and send button on the right.
fn composer_card(state: AppState) -> impl IntoView {
    let input = composer_input(
        state,
        "Describe a task for Elyra — ↵ to send, ⇧↵ for a new line",
        false,
    );

    let attach = label(|| "＋".to_string())
        .style(|s| chip_style(s).font_size(16.0).padding_horiz(6.0))
        .popout_menu(move || {
            Menu::new("")
                .entry(
                    MenuItem::new("Active file").action(move || state.cascade_attach_active_file()),
                )
                .entry(MenuItem::new("Selection").action(move || state.cascade_attach_selection()))
                .entry(
                    MenuItem::new("All open files")
                        .action(move || state.cascade_attach_open_files()),
                )
        });

    let mode = label(move || {
        if state.agent.mode.get() == "ask" {
            "◇ Ask  ▾".to_string()
        } else {
            "‹/› Code  ▾".to_string()
        }
    })
    .style(chip_style)
    .popout_menu(move || {
        let cur = state.agent.mode.get_untracked();
        let mark = |m: &str| if cur == m { "● " } else { "   " };
        Menu::new("")
            .entry(
                MenuItem::new(format!("{}Code — read, edit and run", mark("code")))
                    .action(move || state.set_cascade_mode("code")),
            )
            .entry(
                MenuItem::new(format!(
                    "{}Ask — answer and plan, change nothing",
                    mark("ask")
                ))
                .action(move || state.set_cascade_mode("ask")),
            )
    });

    let model = label(move || model_label(state))
        .style(chip_style)
        .popout_menu(move || model_menu(state));

    let spacer = empty().style(|s| s.flex_grow(1.0_f32));

    let badge = label(|| "⬡ Elyra".to_string())
        .style(|s| s.font_size(12.0).color(theme::fg_dim()).padding_horiz(6.0));

    let send = label(move || {
        if state.agent.chat.with(|c| c.running) {
            "■".to_string()
        } else {
            "↑".to_string()
        }
    })
    .style(move |s| {
        let running = state.agent.chat.with(|c| c.running);
        let (bg, fg) = if running {
            (Color::from_rgb8(0x8a, 0x3c, 0x3c), Color::WHITE)
        } else if theme::is_dark() {
            (theme::fg(), Color::from_rgb8(0x14, 0x16, 0x1b))
        } else {
            (theme::fg(), Color::WHITE)
        };
        s.width(30.0)
            .height(30.0)
            .border_radius(15.0)
            .items_center()
            .justify_center()
            .font_size(15.0)
            .font_bold()
            .margin_left(4.0)
            .background(bg)
            .color(fg)
            .cursor(floem::style::CursorStyle::Pointer)
    })
    .on_click_stop(move |_| {
        if state.agent.chat.with_untracked(|c| c.running) {
            state.native_agent_abort();
        } else {
            state.send_composer();
        }
    });

    let toolbar = stack((attach, mode, model, spacer, badge, send))
        .style(|s| s.items_center().width_full().gap(2.0).padding_top(6.0));

    stack((input, toolbar)).style(|s| {
        s.flex_col()
            .width_full()
            .padding(8.0)
            .border(1.0)
            .border_color(theme::border())
            .border_radius(14.0)
            .background(theme::bg_panel())
    })
}

/// `⌂ Local   ▭ <project>` under the composer.
fn footer(state: AppState) -> impl IntoView {
    label(move || {
        let root = state.root.get();
        let name = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.to_string_lossy().into_owned());
        format!("⌂ Local     ▭ {name}")
    })
    .style(|s| {
        s.font_size(12.0)
            .color(theme::fg_dim())
            .padding_horiz(14.0)
            .padding_top(8.0)
            .padding_bottom(10.0)
    })
}

/// The whole panel: header, body, composer card, footer.
pub fn cascade_panel(state: AppState) -> impl IntoView {
    let bottom = stack((composer_card(state), footer(state))).style(|s| {
        s.flex_col()
            .width_full()
            .padding_horiz(12.0)
            .padding_top(8.0)
            .background(theme::bg())
    });
    stack((header(state), body(state), bottom)).style(|s| s.flex_col().size_full())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_dir_follows_elyras_naming() {
        let d = sessions_dir(Path::new("/Users/kh/Code/e")).unwrap();
        assert!(
            d.ends_with(".elyra/agent/sessions/--Users-kh-Code-e--"),
            "{}",
            d.display()
        );
    }

    #[test]
    fn a_session_is_titled_by_its_name_or_first_prompt() {
        let text = concat!(
            r#"{"type":"session","version":3,"id":"x","timestamp":"2026-07-03T05:04:56.804Z","cwd":"/p"}"#,
            "\n",
            r#"{"type":"model_change","provider":"anthropic","modelId":"claude-opus-5"}"#,
            "\n",
            r#"{"type":"message","id":"m1","message":{"role":"user","content":[{"type":"text","text":"  Fix the   login bug\nplease "}]}}"#,
            "\n",
            r#"{"type":"message","id":"m2","message":{"role":"assistant","content":[{"type":"text","text":"Sure"}]}}"#,
            "\n",
        );
        let (title, when) = session_summary(text);
        assert_eq!(title.as_deref(), Some("Fix the login bug please"));
        assert_eq!(when, "2026-07-03 05:04");

        let named = format!(
            "{text}{}\n",
            r#"{"type":"session_info","name":"Auth refactor"}"#
        );
        assert_eq!(session_summary(&named).0.as_deref(), Some("Auth refactor"));

        let empty = r#"{"type":"session","version":3,"id":"x","timestamp":"2026-07-03T05:04:56.804Z","cwd":"/p"}"#;
        assert_eq!(session_summary(empty).0, None);
    }

    #[test]
    fn long_first_prompts_are_cut() {
        let long = "x".repeat(100);
        let text =
            format!(r#"{{"type":"message","message":{{"role":"user","content":"{long}"}}}}"#);
        let (title, _) = session_summary(&text);
        let title = title.unwrap();
        assert_eq!(title.chars().count(), 60);
        assert!(title.ends_with('…'));
    }

    #[test]
    fn list_sessions_is_newest_first_and_skips_empty_ones() {
        let dir = std::env::temp_dir().join(format!("e-cascade-sessions-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let header = r#"{"type":"session","version":3,"id":"a","timestamp":"2026-07-01T10:00:00.000Z","cwd":"/p"}"#;
        std::fs::write(
            dir.join("2026-07-01_a.jsonl"),
            format!("{header}\n{{\"type\":\"message\",\"message\":{{\"role\":\"user\",\"content\":\"older\"}}}}\n"),
        )
        .unwrap();
        std::fs::write(dir.join("empty.jsonl"), format!("{header}\n")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(30));
        std::fs::write(
            dir.join("2026-07-02_b.jsonl"),
            format!("{header}\n{{\"type\":\"message\",\"message\":{{\"role\":\"user\",\"content\":\"newer\"}}}}\n"),
        )
        .unwrap();
        std::fs::write(dir.join("notes.checkpoints.json"), "{}").unwrap();
        let list = list_sessions(&dir, 10);
        let titles: Vec<&str> = list.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles, vec!["newer", "older"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn provider_labels() {
        assert_eq!(provider_label("google"), "Gemini");
        assert_eq!(provider_label("xai"), "Grok (xAI)");
        assert_eq!(provider_label("mistral"), "Mistral");
    }

    #[test]
    fn shell_quote_leaves_plain_words_and_quotes_the_rest() {
        assert_eq!(shell_quote("claude-opus-5"), "claude-opus-5");
        assert_eq!(shell_quote("/Users/kh/a b.jsonl"), "'/Users/kh/a b.jsonl'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
        assert_eq!(shell_quote(""), "''");
    }
}
