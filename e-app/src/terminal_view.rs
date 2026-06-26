//! The integrated terminal panel.

use std::ops::Range;

use floem::event::{Event, EventListener, EventPropagation};
use floem::keyboard::{Key, NamedKey};
use floem::peniko::Color;
use floem::reactive::SignalGet;
use floem::text::{Attrs, AttrsList, FamilyOwned, TextLayout};
use floem::views::{rich_text, scroll, Decorators};
use floem::IntoView;

use crate::state::AppState;
use crate::theme;

/// Pixel size of one monospace cell at the terminal's font size.
fn char_size() -> (f64, f64) {
    let family: Vec<FamilyOwned> = FamilyOwned::parse_list("monospace").collect();
    let attrs = Attrs::new().family(&family).font_size(13.0);
    let mut layout = TextLayout::new();
    layout.set_text("W", AttrsList::new(attrs), None);
    let size = layout.size();
    (size.width.max(1.0), size.height.max(1.0))
}

/// Translate a key event into the bytes to send to the PTY.
fn key_to_bytes(ke: &floem::keyboard::KeyEvent) -> Option<Vec<u8>> {
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

pub fn terminal_panel(state: AppState) -> impl IntoView {
    let content = rich_text(move || {
        // Track output ticks so the screen repaints.
        state.term_tick.get();
        let runs = state.terminal_runs();

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
            for (seg, fg) in line {
                let start = text.len();
                text.push_str(seg);
                if let Some((r, g, b)) = fg {
                    spans.push((start..text.len(), Color::from_rgb8(*r, *g, *b)));
                }
            }
        }
        for (range, color) in spans {
            attrs_list.add_span(range, Attrs::new().family(&family).font_size(13.0).color(color));
        }

        let mut layout = TextLayout::new();
        layout.set_text(&text, attrs_list, None);
        layout
    })
    .style(|s| s.padding(8.0));

    scroll(content)
        .style(move |s| {
            let s = s
                .width_full()
                .height(300.0)
                .background(Color::from_rgb8(0x14, 0x16, 0x1b))
                .border_top(1.0)
                .border_color(theme::border());
            if state.terminal_open.get() {
                s
            } else {
                s.hide()
            }
        })
        .on_resize(move |rect| {
            let (cw, lh) = char_size();
            let cols = (((rect.width() - 16.0) / cw).floor() as i64).max(20) as usize;
            let rows = (((rect.height() - 16.0) / lh).floor() as i64).max(5) as usize;
            state.resize_terminal(rows, cols);
        })
        .keyboard_navigable()
        .request_focus(move || {
            state.term_tick.get();
            state.terminal_open.get();
        })
        .on_event(EventListener::KeyDown, move |e| {
            if let Event::KeyDown(ke) = e {
                if let Some(bytes) = key_to_bytes(ke) {
                    state.terminal_input(&bytes);
                    return EventPropagation::Stop;
                }
            }
            EventPropagation::Continue
        })
}
