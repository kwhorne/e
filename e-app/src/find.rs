//! Find-in-file (⌘F): a search bar that highlights matches in the active
//! buffer and lets you jump between them.

use floem::keyboard::{Key, NamedKey};
use floem::reactive::{RwSignal, SignalGet, SignalUpdate};
use floem::views::{label, stack, text_input, Decorators};
use floem::IntoView;

use crate::state::AppState;
use crate::theme;

/// A small square toggle button (Aa / \b / .*) for a search option.
fn opt_toggle(glyph: &'static str, sig: RwSignal<bool>, tip: &'static str) -> impl IntoView {
    let _ = tip;
    label(move || glyph.to_string())
        .style(move |s| {
            let s = s
                .width(24.0)
                .height(22.0)
                .items_center()
                .justify_center()
                .font_size(12.0)
                .border_radius(4.0)
                .cursor(floem::style::CursorStyle::Pointer);
            if sig.get() {
                s.background(theme::accent())
                    .color(floem::peniko::Color::from_rgb8(0x14, 0x16, 0x1b))
            } else {
                s.color(theme::fg_dim()).hover(|s| s.background(theme::bg_hover()))
            }
        })
        .on_click_stop(move |_| sig.update(|v| *v = !*v))
}

#[derive(Clone, Copy)]
pub struct FindState {
    pub open: RwSignal<bool>,
    pub query: RwSignal<String>,
    /// Byte offsets `(start, end)` of every match in the active buffer.
    pub matches: RwSignal<Vec<(usize, usize)>>,
    pub current: RwSignal<usize>,
    /// Replacement text and whether the replace row is shown.
    pub replace: RwSignal<String>,
    pub replace_open: RwSignal<bool>,
    /// Search options.
    pub case_sensitive: RwSignal<bool>,
    pub whole_word: RwSignal<bool>,
    pub use_regex: RwSignal<bool>,
}

impl FindState {
    pub fn new() -> Self {
        Self {
            open: RwSignal::new(false),
            query: RwSignal::new(String::new()),
            matches: RwSignal::new(Vec::new()),
            current: RwSignal::new(0),
            replace: RwSignal::new(String::new()),
            replace_open: RwSignal::new(false),
            case_sensitive: RwSignal::new(false),
            whole_word: RwSignal::new(false),
            use_regex: RwSignal::new(false),
        }
    }
}

pub fn find_bar(state: AppState) -> impl IntoView {
    let find = state.find;

    let input = text_input(find.query)
        .placeholder("Find")
        .on_enter(move || state.find_next())
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
        .on_key_down(
            Key::Named(NamedKey::Escape),
            |_| true,
            move |_| {
                state.close_find();
            },
        )
        .on_key_down(
            Key::Named(NamedKey::ArrowDown),
            |_| true,
            move |_| state.find_next(),
        )
        .on_key_down(
            Key::Named(NamedKey::ArrowUp),
            |_| true,
            move |_| state.find_prev(),
        );

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

    let toggles = stack((
        opt_toggle("Aa", find.case_sensitive, "Match case"),
        opt_toggle("W", find.whole_word, "Whole word"),
        opt_toggle(".*", find.use_regex, "Regex"),
    ))
    .style(|s| s.items_center().gap(4.0));

    // Row 1: find input, toggles, count, next/prev, close.
    let find_row = stack((input, toggles, count, close)).style(|s| s.items_center().gap(8.0));

    // Row 2: replace input + Replace / Replace All (shown when expanded).
    let replace_input = text_input(find.replace)
        .placeholder("Replace")
        .on_enter(move || state.replace_current())
        .style(|s| {
            theme::input_colors(s)
                .width(220.0)
                .height(28.0)
                .padding_horiz(8.0)
                .border(1.0)
                .border_radius(4.0)
        });

    let btn = |text: &'static str| {
        label(move || text.to_string()).style(|s| {
            s.padding_horiz(10.0)
                .height(26.0)
                .items_center()
                .font_size(12.0)
                .border(1.0)
                .border_color(theme::border())
                .border_radius(4.0)
                .color(theme::fg())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::bg_hover()))
        })
    };

    let replace_row = stack((
        replace_input,
        btn("Replace").on_click_stop(move |_| state.replace_current()),
        btn("All").on_click_stop(move |_| state.replace_all()),
    ))
    .style(move |s| {
        let s = s.items_center().gap(8.0).margin_top(8.0);
        if find.replace_open.get() {
            s
        } else {
            s.hide()
        }
    });

    stack((find_row, replace_row)).style(move |s| {
        let s = s
            .flex_col()
            .absolute()
            .inset_top(8.0)
            .inset_right(20.0)
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
