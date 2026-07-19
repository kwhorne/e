//! The native agent chat panel.
//!
//! Renders [`AppState::agent_chat`] (folded from elyra's RPC event stream by the
//! `e-agent` crate) with native floem views — streaming assistant text, tool-call
//! cards, and a composer — instead of running the agent's terminal UI in a PTY.
//! This is what makes ⌘L feel responsive: we append only the changed text rather
//! than re-parsing a whole ANSI grid every frame.

use e_agent::{ChatItem, ToolStatus};
use floem::peniko::Color;
use floem::reactive::{SignalGet, SignalWith};
use floem::views::{dyn_stack, empty, label, scroll, stack, text_input, Decorators};
use floem::IntoView;

use crate::state::AppState;
use crate::theme;

/// Total characters currently in the transcript — used to drive auto-scroll so
/// the view follows streaming text (not just newly-added items).
fn transcript_size(state: AppState) -> usize {
    state.agent_chat.with(|c| {
        c.items
            .iter()
            .map(|it| match it {
                ChatItem::User { text }
                | ChatItem::Assistant { text, .. }
                | ChatItem::Reasoning { text, .. }
                | ChatItem::Notice { text, .. } => text.len(),
                ChatItem::Tool(tc) => {
                    tc.summary.len() + tc.result.as_ref().map(String::len).unwrap_or(0)
                }
            })
            .sum()
    })
}

/// A reactive getter for the text of item `i`, given its variant is stable.
fn item_text(state: AppState, i: usize) -> String {
    state.agent_chat.with(|c| match c.items.get(i) {
        Some(ChatItem::User { text })
        | Some(ChatItem::Assistant { text, .. })
        | Some(ChatItem::Reasoning { text, .. })
        | Some(ChatItem::Notice { text, .. }) => text.clone(),
        _ => String::new(),
    })
}

fn is_streaming(state: AppState, i: usize) -> bool {
    state.agent_chat.with(|c| {
        matches!(
            c.items.get(i),
            Some(ChatItem::Assistant {
                streaming: true,
                ..
            }) | Some(ChatItem::Reasoning {
                streaming: true,
                ..
            })
        )
    })
}

/// Render one transcript row. The item's *variant* never changes once created
/// (the reducer only appends items or mutates their fields), so we pick the
/// layout once and read the mutable text/status reactively.
fn render_item(state: AppState, i: usize) -> impl IntoView {
    let variant = state.agent_chat.with_untracked(|c| c.items.get(i).cloned());

    match variant {
        Some(ChatItem::User { .. }) => label(move || item_text(state, i))
            .style(|s| {
                s.padding(10.0)
                    .margin_vert(4.0)
                    .margin_horiz(8.0)
                    .border_radius(8.0)
                    .background(theme::bg_active())
                    .color(theme::fg())
                    .width_full()
            })
            .into_any(),

        Some(ChatItem::Assistant { .. }) => floem::views::dyn_container(
            // While streaming, show plain text (cheap per-delta rebuild); once the
            // message is complete, render it as formatted markdown like Zed.
            move || (is_streaming(state, i), item_text(state, i)),
            move |(streaming, text)| {
                if streaming {
                    label(move || format!("{text}\u{258d}"))
                        .style(|s| s.color(theme::fg()).width_full())
                        .into_any()
                } else {
                    crate::markdown_view::markdown_body(&text)
                        .style(|s| s.width_full())
                        .into_any()
                }
            },
        )
        .style(|s| s.padding_horiz(14.0).padding_vert(6.0).width_full())
        .into_any(),

        Some(ChatItem::Reasoning { .. }) => label(move || item_text(state, i))
            .style(|s| {
                s.padding_horiz(12.0)
                    .padding_vert(4.0)
                    .font_size(12.0)
                    .font_style(floem::text::Style::Italic)
                    .color(theme::fg_dim())
                    .width_full()
            })
            .into_any(),

        Some(ChatItem::Tool(_)) => tool_card(state, i).into_any(),

        Some(ChatItem::Notice { error, .. }) => label(move || item_text(state, i))
            .style(move |s| {
                let c = if error {
                    Color::from_rgb8(0xd6, 0x7a, 0x7a)
                } else {
                    theme::fg_dim()
                };
                s.padding_horiz(12.0)
                    .padding_vert(3.0)
                    .font_size(12.0)
                    .color(c)
                    .width_full()
            })
            .into_any(),

        None => empty().into_any(),
    }
}

/// A collapsible-looking card for one tool call: status glyph + name + summary,
/// and a dimmed monospace preview of the result.
fn tool_card(state: AppState, i: usize) -> impl IntoView {
    let header = label(move || {
        state.agent_chat.with(|c| match c.items.get(i) {
            Some(ChatItem::Tool(tc)) => {
                let glyph = match tc.status {
                    ToolStatus::Running => "\u{25cf}", // ●
                    ToolStatus::Done => "\u{2713}",    // ✓
                    ToolStatus::Error => "\u{2717}",   // ✗
                };
                if tc.summary.is_empty() {
                    format!("{glyph}  {}", tc.name)
                } else {
                    format!("{glyph}  {}  {}", tc.name, tc.summary)
                }
            }
            _ => String::new(),
        })
    })
    .style(move |s| {
        let color = state.agent_chat.with(|c| match c.items.get(i) {
            Some(ChatItem::Tool(tc)) => match tc.status {
                ToolStatus::Running => theme::accent(),
                ToolStatus::Done => theme::fg_dim(),
                ToolStatus::Error => Color::from_rgb8(0xd6, 0x7a, 0x7a),
            },
            _ => theme::fg_dim(),
        });
        s.font_family("monospace".to_string())
            .font_size(12.0)
            .color(color)
    });

    let result = label(move || {
        state.agent_chat.with(|c| match c.items.get(i) {
            Some(ChatItem::Tool(tc)) => {
                let r = tc.result.clone().unwrap_or_default();
                // Show only the first few lines to keep cards compact.
                let mut lines: Vec<&str> = r.lines().take(8).collect();
                if r.lines().count() > 8 {
                    lines.push("…");
                }
                lines.join("\n")
            }
            _ => String::new(),
        })
    })
    .style(|s| {
        s.font_family("monospace".to_string())
            .font_size(11.0)
            .color(theme::fg_dim())
            .margin_top(4.0)
    });

    stack((header, result)).style(|s| {
        s.flex_col()
            .margin_vert(4.0)
            .margin_horiz(8.0)
            .padding(8.0)
            .border(1.0)
            .border_color(theme::border())
            .border_radius(6.0)
            .background(theme::bg_panel())
            .width_full()
    })
}

/// The scrollable transcript.
fn transcript(state: AppState) -> impl IntoView {
    let rows = dyn_stack(
        move || 0..state.agent_chat.with(|c| c.items.len()),
        |i| *i,
        move |i| render_item(state, i),
    )
    .style(|s| s.flex_col().width_full().padding_vert(10.0).gap(2.0));

    scroll(rows)
        .style(|s| s.size_full().flex_grow(1.0).background(theme::bg()))
        // Follow the tail as content streams in.
        .scroll_to_percent(move || {
            let _ = transcript_size(state);
            1.0
        })
}

/// The composer: a padded input box with a toolbar row below (model + send/stop),
/// styled after Zed's agent panel so it sits comfortably above the window edge
/// instead of being flush at the very bottom.
fn composer(state: AppState) -> impl IntoView {
    let input = text_input(state.agent_composer)
        .placeholder("Message the agent…   @ for context, / for commands")
        .on_enter(move || {
            let t = state.agent_composer.get_untracked();
            state.send_native_prompt(&t);
        })
        .style(|s| {
            theme::input_colors(s)
                .width_full()
                .height(44.0)
                .padding_horiz(12.0)
                .border(1.0)
                .border_color(theme::border())
                .border_radius(10.0)
                .background(theme::bg_panel())
        });

    // The active agent's name, shown bottom-left like Zed's model picker.
    let model = label(move || {
        let id = state.agent_current.get();
        state
            .agents
            .with(|l| l.iter().find(|a| a.id == id).map(|a| a.name.clone()))
            .unwrap_or_else(|| "Agent".to_string())
    })
    .style(|s| s.font_size(12.0).color(theme::fg_dim()).items_center());

    let spacer = empty().style(|s| s.flex_grow(1.0));

    // Stop while running, Send otherwise.
    let action_btn = label(move || {
        if state.agent_chat.with(|c| c.running) {
            "\u{25a0}  Stop".to_string()
        } else {
            "Send  \u{2191}".to_string()
        }
    })
    .style(move |s| {
        let running = state.agent_chat.with(|c| c.running);
        let bg = if running {
            Color::from_rgb8(0x6a, 0x3a, 0x3a)
        } else {
            theme::accent()
        };
        s.height(28.0)
            .items_center()
            .justify_center()
            .padding_horiz(12.0)
            .border_radius(6.0)
            .font_size(12.0)
            .color(Color::WHITE)
            .background(bg)
            .cursor(floem::style::CursorStyle::Pointer)
            .hover(move |s| {
                s.background(if running {
                    Color::from_rgb8(0x7a, 0x44, 0x44)
                } else {
                    Color::from_rgb8(0x4a, 0x7c, 0xe0)
                })
            })
    })
    .on_click_stop(move |_| {
        if state.agent_chat.with_untracked(|c| c.running) {
            state.native_agent_abort();
        } else {
            let t = state.agent_composer.get_untracked();
            state.send_native_prompt(&t);
        }
    });

    let toolbar =
        stack((model, spacer, action_btn)).style(|s| s.items_center().width_full().margin_top(8.0));

    stack((input, toolbar)).style(|s| {
        s.flex_col()
            .width_full()
            .padding(10.0)
            .border_top(1.0)
            .border_color(theme::border())
            .background(theme::bg())
    })
}

/// The native agent panel body (transcript + composer).
pub fn agent_native_body(state: AppState) -> impl IntoView {
    stack((transcript(state), composer(state))).style(|s| s.flex_col().size_full())
}
