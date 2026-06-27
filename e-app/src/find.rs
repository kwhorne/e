//! Find-in-file (⌘F): a search bar that highlights matches in the active
//! buffer and lets you jump between them.

use floem::keyboard::{Key, NamedKey};
use floem::reactive::{RwSignal, SignalGet};
use floem::views::{label, stack, text_input, Decorators};
use floem::IntoView;

use crate::state::AppState;
use crate::theme;

#[derive(Clone, Copy)]
pub struct FindState {
    pub open: RwSignal<bool>,
    pub query: RwSignal<String>,
    /// Byte offsets `(start, end)` of every match in the active buffer.
    pub matches: RwSignal<Vec<(usize, usize)>>,
    pub current: RwSignal<usize>,
}

impl FindState {
    pub fn new() -> Self {
        Self {
            open: RwSignal::new(false),
            query: RwSignal::new(String::new()),
            matches: RwSignal::new(Vec::new()),
            current: RwSignal::new(0),
        }
    }
}

pub fn find_bar(state: AppState) -> impl IntoView {
    let find = state.find;

    let input = text_input(find.query)
        .placeholder("Find")
        .style(|s| {
            theme::input_colors(s)
                .width(220.0)
                .height(28.0)
                .padding_horiz(8.0)
                .border(1.0)
                .border_radius(4.0)
        })
        .request_focus(move || {
            find.open.get();
        })
        .on_key_down(Key::Named(NamedKey::Escape), |_| true, move |_| {
            state.close_find();
        })
        .on_key_down(Key::Named(NamedKey::Enter), |m| !m.shift(), move |_| {
            state.find_next();
        })
        .on_key_down(Key::Named(NamedKey::Enter), |m| m.shift(), move |_| {
            state.find_prev();
        });

    let count = label(move || {
        let n = find.matches.get().len();
        if n == 0 {
            if find.query.get().is_empty() {
                String::new()
            } else {
                "No results".to_string()
            }
        } else {
            format!("{}/{}", find.current.get() + 1, n)
        }
    })
    .style(|s| s.color(theme::fg_dim()).font_size(12.0).min_width(60.0));

    let close = label(|| "×".to_string())
        .style(|s| {
            s.padding_horiz(6.0)
                .color(theme::fg_dim())
                .border_radius(4.0)
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::bg_hover()).color(theme::fg()))
        })
        .on_click_stop(move |_| state.close_find());

    stack((input, count, close)).style(move |s| {
        let s = s
            .absolute()
            .inset_top(8.0)
            .inset_right(20.0)
            .items_center()
            .gap(8.0)
            .padding(8.0)
            .background(theme::bg_panel())
            .border(1.0)
            .border_color(theme::border())
            .border_radius(6.0);
        if find.open.get() {
            s
        } else {
            s.hide()
        }
    })
}
