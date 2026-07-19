//! The right-hand Agent panel: runs a CLI coding agent (Elyra, Claude Code,
//! Codex …) in a PTY, with a header to switch agent / restart.

use std::ops::Range;

use floem::event::{Event, EventListener, EventPropagation};
use floem::menu::{Menu, MenuItem};
use floem::peniko::Color;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use floem::text::{Attrs, AttrsList, FamilyOwned, TextLayout};
use floem::views::{dyn_container, empty, label, rich_text, scroll, stack, Decorators};
use floem::IntoView;

use crate::agent_native::agent_native_body;
use crate::app::handle_shortcut;
use crate::state::AppState;
use crate::terminal_view::{char_size, key_to_bytes};
use crate::theme;

/// Header: agent selector (popout menu) + restart button.
fn agent_header(state: AppState) -> impl IntoView {
    let title = label(move || {
        let id = state.agent_current.get();
        let name = state
            .agents
            .with(|list| list.iter().find(|a| a.id == id).map(|a| a.name.clone()))
            .unwrap_or_else(|| "Agent".to_string());
        format!("{name}  ▾")
    })
    .style(|s| {
        s.padding_horiz(10.0)
            .height(28.0)
            .items_center()
            .font_size(12.0)
            .color(theme::fg())
            .cursor(floem::style::CursorStyle::Pointer)
            .hover(|s| s.background(theme::bg_hover()))
    })
    .popout_menu(move || {
        let current = state.agent_current.get_untracked();
        let mut menu = Menu::new("");
        for a in state.agents.get_untracked() {
            let id = a.id.clone();
            let mark = if id == current { "● " } else { "   " };
            menu = menu.entry(
                MenuItem::new(format!("{mark}{}", a.name)).action(move || state.select_agent(&id)),
            );
        }
        menu.separator()
            .entry(MenuItem::new("New Chat").action(move || state.native_agent_new_session()))
            .entry(MenuItem::new("Restart Agent").action(move || state.restart_agent()))
            .entry(MenuItem::new("Settings…  (⌘,)").action(move || state.open_settings()))
    });

    let icon_btn = |glyph: &'static str| {
        label(move || glyph.to_string()).style(|s| {
            s.width(28.0)
                .height(28.0)
                .items_center()
                .justify_center()
                .font_size(14.0)
                .color(theme::fg_dim())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::bg_hover()).color(theme::fg()))
        })
    };

    let restart = icon_btn("⟳").on_click_stop(move |_| state.restart_agent());
    let close = icon_btn("×").on_click_stop(move |_| state.agent_open.set(false));

    let spacer = empty().style(|s| s.flex_grow(1.0));

    stack((title, spacer, restart, close)).style(|s| {
        s.items_center()
            .width_full()
            .height(28.0)
            .background(theme::bg_panel())
            .border_bottom(1.0)
            .border_color(theme::border())
    })
}

/// The scrollable agent screen + cursor + keyboard input.
fn agent_body(state: AppState) -> impl IntoView {
    let content = rich_text(move || {
        state.term_tick.get();
        let runs = state.agent_runs();
        let family: Vec<FamilyOwned> = FamilyOwned::parse_list("monospace").collect();
        let default = Attrs::new()
            .family(&family)
            .font_size(13.0)
            .color(theme::fg());
        let mut attrs_list = AttrsList::new(default);
        let mut text = String::new();
        let mut spans: Vec<(Range<usize>, Color)> = Vec::new();
        for (li, line) in runs.iter().enumerate() {
            if li > 0 {
                text.push('\n');
            }
            for (seg, fg, _bg) in line {
                let start = text.len();
                text.push_str(seg);
                if let Some((r, g, b)) = fg {
                    spans.push((start..text.len(), Color::from_rgb8(*r, *g, *b)));
                }
            }
        }
        for (range, color) in spans {
            attrs_list.add_span(
                range,
                Attrs::new().family(&family).font_size(13.0).color(color),
            );
        }
        let mut layout = TextLayout::new();
        layout.set_text(&text, attrs_list, None);
        layout
    })
    .style(|s| s.padding(8.0));

    let cursor_block = empty().style(move |s| {
        state.term_tick.get();
        let (cw, lh) = char_size();
        match state.agent_cursor() {
            Some((row, col)) => s
                .absolute()
                .inset_left(8.0 + col as f64 * cw)
                .inset_top(8.0 + row as f64 * lh)
                .width(cw)
                .height(lh)
                .background(Color::from_rgba8(0xe8, 0xee, 0xfc, 0x88)),
            None => s.absolute().hide(),
        }
    });

    let body = stack((content, cursor_block)).style(|s| s.size_full());

    scroll(body)
        .style(|s| {
            s.size_full()
                .flex_grow(1.0)
                .background(Color::from_rgb8(0x14, 0x16, 0x1b))
        })
        .on_resize(move |rect| {
            let (cw, lh) = char_size();
            let cols = (((rect.width() - 16.0) / cw).floor() as i64).max(20) as usize;
            let rows = (((rect.height() - 16.0) / lh).floor() as i64).max(5) as usize;
            state.resize_agent(rows, cols);
        })
        .keyboard_navigable()
        .request_focus(move || {
            state.agent_focus_pulse.get();
        })
        .on_event_cont(EventListener::FocusGained, move |_| {
            state.agent_focused.set(true)
        })
        .on_event_cont(EventListener::FocusLost, move |_| {
            state.agent_focused.set(false)
        })
        .on_event(EventListener::KeyDown, move |e| {
            if let Event::KeyDown(ke) = e {
                if ke.modifiers.meta() && handle_shortcut(state, &ke.key.logical_key, ke.modifiers)
                {
                    return EventPropagation::Stop;
                }
                if let Some(bytes) = key_to_bytes(ke) {
                    state.agent_input(&bytes);
                    return EventPropagation::Stop;
                }
            }
            EventPropagation::Continue
        })
}

/// True when the currently-selected agent should render as the native chat
/// panel (elyra RPC) rather than a terminal PTY. Reactive: recomputes when the
/// setting or the selected agent changes.
fn native_selected(state: AppState) -> bool {
    if !state.settings.with(|s| s.native_agent) {
        return false;
    }
    let id = state.agent_current.get();
    let program = state
        .agents
        .with(|l| {
            l.iter().find(|a| a.id == id).map(|a| {
                a.command
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_string()
            })
        })
        .unwrap_or_default();
    id == "elyra" || program == "elyra" || program.rsplit('/').next() == Some("elyra")
}

pub fn agent_panel(state: AppState) -> impl IntoView {
    let body = dyn_container(
        move || native_selected(state),
        move |native| {
            if native {
                agent_native_body(state).into_any()
            } else {
                agent_body(state).into_any()
            }
        },
    )
    .style(|s| s.flex_col().size_full());
    stack((agent_header(state), body)).style(move |s| {
        let s = s
            .flex_col()
            .width(state.agent_width.get())
            .height_full()
            .border_left(1.0)
            .border_color(theme::border());
        if state.agent_open.get() {
            s
        } else {
            s.hide()
        }
    })
}
