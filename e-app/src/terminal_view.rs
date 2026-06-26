//! The integrated terminal panel.

use floem::event::{Event, EventListener, EventPropagation};
use floem::keyboard::{Key, NamedKey};
use floem::peniko::Color;
use floem::reactive::SignalGet;
use floem::views::{label, scroll, Decorators};
use floem::IntoView;

use crate::state::AppState;
use crate::theme;

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
    let content = label(move || {
        // Track output ticks so the screen repaints.
        state.term_tick.get();
        state.terminal_snapshot().join("\n")
    })
    .style(|s| {
        s.width_full()
            .padding(8.0)
            .font_family("monospace".to_string())
            .font_size(13.0)
            .line_height(1.3)
            .color(theme::FG)
    });

    scroll(content)
        .style(move |s| {
            let s = s
                .width_full()
                .height(300.0)
                .background(Color::from_rgb8(0x14, 0x16, 0x1b))
                .border_top(1.0)
                .border_color(theme::BORDER);
            if state.terminal_open.get() {
                s
            } else {
                s.hide()
            }
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
