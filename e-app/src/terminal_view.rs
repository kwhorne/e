//! The integrated terminal panel.

use std::ops::Range;

use floem::event::{Event, EventListener, EventPropagation};
use floem::keyboard::{Key, NamedKey};
use floem::menu::{Menu, MenuItem};
use floem::peniko::Color;
use floem::reactive::{SignalGet, SignalUpdate};
use floem::text::{Attrs, AttrsList, FamilyOwned, TextLayout};
use floem::views::{
    container, dyn_container, dyn_stack, empty, label, rich_text, scroll, stack, text_input,
    Decorators,
};
use floem::IntoView;

use crate::app::handle_shortcut;
use crate::state::AppState;
use crate::theme;

/// The tab strip at the top of the terminal panel: one tab per session,
/// plus a "+" to add a new terminal.
fn terminal_tabs(state: AppState) -> impl IntoView {
    let tabs = dyn_stack(
        move || {
            state
                .terminals
                .get()
                .into_iter()
                .enumerate()
                .collect::<Vec<_>>()
        },
        |(_, t)| t.id,
        move |(i, t)| {
            let id = t.id;
            let name = t.name;
            let is_active = move || {
                state.active_terminal.get() == Some(id) || state.active_terminal2.get() == Some(id)
            };
            stack((
                label(move || {
                    let n = name.get();
                    if n.is_empty() {
                        format!("zsh {}", i + 1)
                    } else {
                        n
                    }
                })
                .style(|s| s.color(theme::fg())),
                label(|| "×".to_string())
                    .style(|s| {
                        s.padding_horiz(4.0)
                            .border_radius(4.0)
                            .color(theme::fg_dim())
                            .hover(|s| s.background(theme::bg_hover()).color(theme::fg()))
                    })
                    .on_click_stop(move |_| state.close_terminal(id)),
            ))
            .style(move |s| {
                let s = s
                    .items_center()
                    .gap(6.0)
                    .padding_horiz(10.0)
                    .height(28.0)
                    .font_size(12.0)
                    .cursor(floem::style::CursorStyle::Pointer)
                    .border_right(1.0)
                    .border_color(theme::border());
                if is_active() {
                    s.background(theme::bg_active())
                } else {
                    s.hover(|s| s.background(theme::bg_hover()))
                }
            })
            .on_click_stop(move |_| state.focus_terminal(id))
            .context_menu(move || {
                Menu::new("")
                    .entry(MenuItem::new("Rename").action(move || state.start_term_rename(id)))
                    .entry(MenuItem::new("Split").action(move || state.split_terminal()))
                    .separator()
                    .entry(MenuItem::new("Close").action(move || state.close_terminal(id)))
            })
        },
    )
    .style(|s| s.items_center());

    let icon_btn = |glyph: &'static str| {
        label(move || glyph.to_string()).style(|s| {
            s.width(28.0)
                .height(28.0)
                .items_center()
                .justify_center()
                .font_size(15.0)
                .color(theme::fg_dim())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::bg_hover()).color(theme::fg()))
        })
    };

    let split = icon_btn("⊟").on_click_stop(move |_| state.split_terminal());
    let add = icon_btn("+").on_click_stop(move |_| state.new_terminal());

    stack((tabs, split, add)).style(|s| {
        s.items_center()
            .width_full()
            .height(28.0)
            .background(theme::bg_panel())
            .border_bottom(1.0)
            .border_color(theme::border())
    })
}

/// Pixel size of one monospace cell at the terminal's font size.
pub(crate) fn char_size() -> (f64, f64) {
    let family: Vec<FamilyOwned> = FamilyOwned::parse_list("monospace").collect();
    let attrs = Attrs::new().family(&family).font_size(13.0);
    let mut layout = TextLayout::new();
    layout.set_text("W", AttrsList::new(attrs), None);
    let size = layout.size();
    (size.width.max(1.0), size.height.max(1.0))
}

/// Translate a key event into the bytes to send to the PTY.
pub(crate) fn key_to_bytes(ke: &floem::keyboard::KeyEvent) -> Option<Vec<u8>> {
    let mods = ke.modifiers;
    match &ke.key.logical_key {
        Key::Character(s) => {
            if mods.control() {
                // Ctrl+<letter> -> control byte.
                let c = s.chars().next()?;
                if c.is_ascii_alphabetic() {
                    return Some(vec![(c.to_ascii_lowercase() as u8) & 0x1f]);
                }
            }
            Some(s.as_bytes().to_vec())
        }
        Key::Named(NamedKey::Enter) => Some(b"\r".to_vec()),
        Key::Named(NamedKey::Backspace) => Some(vec![0x7f]),
        Key::Named(NamedKey::Tab) => Some(b"\t".to_vec()),
        Key::Named(NamedKey::Escape) => Some(vec![0x1b]),
        Key::Named(NamedKey::Space) => Some(b" ".to_vec()),
        Key::Named(NamedKey::ArrowUp) => Some(b"\x1b[A".to_vec()),
        Key::Named(NamedKey::ArrowDown) => Some(b"\x1b[B".to_vec()),
        Key::Named(NamedKey::ArrowRight) => Some(b"\x1b[C".to_vec()),
        Key::Named(NamedKey::ArrowLeft) => Some(b"\x1b[D".to_vec()),
        _ => None,
    }
}

/// Render one terminal pane: a scrollable coloured screen + cursor + input.
fn term_pane(state: AppState, pane_idx: u8) -> impl IntoView {
    let id_sig = if pane_idx == 1 {
        state.active_terminal2
    } else {
        state.active_terminal
    };

    let content = rich_text(move || {
        state.term_tick.get();
        let runs = state.term_runs_of(id_sig.get());
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

    // Background-colour rectangles (drawn behind the text), since the text
    // layout engine only supports foreground colour.
    let bg_layer = dyn_stack(
        move || {
            state.term_tick.get();
            let runs = state.term_runs_of(id_sig.get());
            let mut rects: Vec<(usize, usize, usize, u8, u8, u8)> = Vec::new();
            for (li, line) in runs.iter().enumerate() {
                let mut col = 0usize;
                for (seg, _fg, bg) in line {
                    let w = seg.chars().count();
                    if let Some((r, g, b)) = bg {
                        rects.push((li, col, w, *r, *g, *b));
                    }
                    col += w;
                }
            }
            rects.into_iter().enumerate().collect::<Vec<_>>()
        },
        |(i, _)| *i,
        move |(_, (li, col, w, r, g, b))| {
            let (cw, lh) = char_size();
            empty().style(move |s| {
                s.absolute()
                    .inset_left(8.0 + col as f64 * cw)
                    .inset_top(8.0 + li as f64 * lh)
                    .width(w as f64 * cw)
                    .height(lh)
                    .background(Color::from_rgb8(r, g, b))
            })
        },
    )
    .style(|s| s.absolute().inset(0.0));

    let cursor_block = empty().style(move |s| {
        state.term_tick.get();
        let (cw, lh) = char_size();
        match state.term_cursor_of(id_sig.get()) {
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

    let body = stack((bg_layer, content, cursor_block)).style(|s| s.size_full());

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
            state.resize_terminal(rows, cols);
        })
        .on_event_stop(EventListener::PointerWheel, move |e| {
            if let Event::PointerWheel(pe) = e {
                let dy = pe.delta.y;
                if dy != 0.0 {
                    let lines = ((dy.abs() / 24.0).ceil() as usize).max(1);
                    // Wheel up (negative dy) scrolls into the scrollback history.
                    state.term_scroll(id_sig.get_untracked(), dy < 0.0, lines);
                }
            }
        })
        .keyboard_navigable()
        .request_focus(move || {
            state.terminal_open.get();
            state.term_split.get();
        })
        .on_event_cont(EventListener::FocusGained, move |_| {
            state.terminal_focused.set(true);
            state.term_focus_pane.set(pane_idx);
        })
        .on_event_cont(EventListener::FocusLost, move |_| {
            state.terminal_focused.set(false)
        })
        .on_event(EventListener::KeyDown, move |e| {
            if let Event::KeyDown(ke) = e {
                if ke.modifiers.meta() && handle_shortcut(state, &ke.key.logical_key, ke.modifiers)
                {
                    return EventPropagation::Stop;
                }
                if let Some(bytes) = key_to_bytes(ke) {
                    if let Some(id) = id_sig.get_untracked() {
                        // Typing snaps the view back to the live bottom.
                        state.term_scroll_bottom(Some(id));
                        state.term_input_to(id, &bytes);
                    }
                    return EventPropagation::Stop;
                }
            }
            EventPropagation::Continue
        })
}

pub fn terminal_panel(state: AppState) -> impl IntoView {
    let panes = dyn_container(
        move || state.term_split.get(),
        move |split| {
            if split {
                stack((
                    term_pane(state, 0).style(|s| {
                        s.flex_grow(1.0)
                            .height_full()
                            .border_right(1.0)
                            .border_color(theme::border())
                    }),
                    term_pane(state, 1).style(|s| s.flex_grow(1.0).height_full()),
                ))
                .style(|s| s.flex_row().size_full())
                .into_any()
            } else {
                term_pane(state, 0).style(|s| s.size_full()).into_any()
            }
        },
    )
    .style(|s| s.flex_grow(1.0).width_full());

    stack((term_resize_handle(state), terminal_tabs(state), panes)).style(move |s| {
        let s = s
            .flex_col()
            .width_full()
            .height(state.term_height.get())
            .border_top(1.0)
            .border_color(theme::border());
        if state.terminal_open.get() {
            s
        } else {
            s.hide()
        }
    })
}

/// A thin horizontal handle along the top edge of the terminal panel that
/// drag-resizes its height. Mirrors the side-panel handle but on the Y axis.
fn term_resize_handle(state: AppState) -> impl IntoView {
    let drag_start: floem::reactive::RwSignal<Option<f64>> = floem::reactive::RwSignal::new(None);
    let view = empty();
    let id = floem::View::id(&view);
    view.on_event_stop(EventListener::PointerDown, move |e| {
        id.request_active();
        if let Event::PointerDown(pe) = e {
            drag_start.set(Some(pe.pos.y));
        }
    })
    .on_event_stop(EventListener::PointerMove, move |e| {
        if let Event::PointerMove(pe) = e {
            if let Some(start) = drag_start.get_untracked() {
                let delta = pe.pos.y - start;
                let cur = state.term_height.get_untracked();
                // Dragging up (negative delta) makes the panel taller.
                state.term_height.set((cur - delta).clamp(120.0, 900.0));
            }
        }
    })
    .on_event_stop(EventListener::PointerUp, move |_| drag_start.set(None))
    .style(|s| {
        s.height(6.0)
            .width_full()
            .cursor(floem::style::CursorStyle::RowResize)
            .hover(|s| s.background(theme::accent()))
    })
}

/// The terminal rename prompt overlay.
pub fn term_rename_prompt(state: AppState) -> impl IntoView {
    let input = text_input(state.term_rename_input)
        .on_enter(move || state.confirm_term_rename())
        .style(|s| {
            theme::input_colors(s)
                .width(280.0)
                .height(30.0)
                .padding_horiz(8.0)
                .border(1.0)
                .border_radius(4.0)
        })
        .request_focus(move || {
            state.term_rename_id.get();
        })
        .on_key_down(
            Key::Named(NamedKey::Escape),
            |_| true,
            move |_| state.term_rename_id.set(None),
        );

    let box_ = stack((
        label(|| "Rename terminal:".to_string())
            .style(|s| s.color(theme::fg_dim()).font_size(12.0)),
        input,
    ))
    .style(|s| {
        s.flex_col()
            .gap(8.0)
            .padding(14.0)
            .background(theme::bg_panel())
            .border(1.0)
            .border_color(theme::border())
            .border_radius(8.0)
    });

    container(box_)
        .style(move |s| {
            let s = s
                .absolute()
                .inset(0.0)
                .size_full()
                .justify_center()
                .padding_top(120.0);
            if state.term_rename_id.get().is_some() {
                s
            } else {
                s.hide()
            }
        })
        .on_click_stop(move |_| state.term_rename_id.set(None))
}
