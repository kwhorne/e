//! A list-picker overlay used for "go to references" and workspace symbol
//! search (⌘T). Always shows a focused input so keyboard navigation works.

use floem::keyboard::{Key, NamedKey};
use floem::reactive::{create_effect, RwSignal, SignalGet, SignalUpdate};
use floem::views::{dyn_stack, label, scroll, stack, text_input, Decorators};
use floem::IntoView;

use crate::state::AppState;
use crate::theme;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PickerMode {
    References,
    Symbols,
    Search,
}

#[derive(Clone)]
pub struct PickerItem {
    pub label: String,
    pub detail: String,
    pub uri: String,
    pub line: u32,
    pub char: u32,
}

#[derive(Clone, Copy)]
pub struct Picker {
    pub open: RwSignal<bool>,
    pub mode: RwSignal<PickerMode>,
    pub query: RwSignal<String>,
    pub items: RwSignal<Vec<PickerItem>>,
    pub selected: RwSignal<usize>,
    /// Generation counter to drop stale async symbol results.
    pub gen: RwSignal<u64>,
}

impl Picker {
    pub fn new() -> Self {
        Self {
            open: RwSignal::new(false),
            mode: RwSignal::new(PickerMode::Symbols),
            query: RwSignal::new(String::new()),
            items: RwSignal::new(Vec::new()),
            selected: RwSignal::new(0),
            gen: RwSignal::new(0),
        }
    }
}

pub fn picker_overlay(state: AppState) -> impl IntoView {
    let p = state.picker;

    let focus_pulse: RwSignal<u64> = RwSignal::new(0);
    create_effect(move |_| {
        if p.open.get() {
            focus_pulse.update(|x| *x += 1);
        }
    });

    // Symbols and Search re-query asynchronously as the query changes.
    create_effect(move |_| {
        if p.open.get() && matches!(p.mode.get(), PickerMode::Symbols | PickerMode::Search) {
            let q = p.query.get();
            state.run_picker_query(q);
        }
    });

    // Items actually displayed: references are filtered client-side.
    let displayed = move || -> Vec<(usize, PickerItem)> {
        let items = p.items.get();
        let q = p.query.get().to_lowercase();
        let filter = p.mode.get() == PickerMode::References && !q.is_empty();
        items
            .into_iter()
            .filter(|it| {
                !filter
                    || it.label.to_lowercase().contains(&q)
                    || it.detail.to_lowercase().contains(&q)
            })
            .take(300)
            .enumerate()
            .collect()
    };

    let displayed_for_keys = displayed;
    let accept = move || {
        let items = displayed_for_keys();
        if items.is_empty() {
            return;
        }
        let idx = p.selected.get().min(items.len() - 1);
        let it = items[idx].1.clone();
        p.open.set(false);
        state.jump_to(&it.uri, it.line as usize, it.char as usize);
    };

    let input = text_input(p.query)
        .placeholder("Search…")
        .on_enter(accept)
        .style(|s| {
            theme::input_colors(s)
                .width_full()
                .height(36.0)
                .padding_horiz(10.0)
                .border(0.0)
                .border_bottom(1.0)
        })
        .request_focus(move || {
            focus_pulse.get();
        })
        .on_event_stop(floem::event::EventListener::FocusLost, move |_| {
            if p.open.get_untracked() {
                p.open.set(false);
            }
        })
        .on_key_down(
            Key::Named(NamedKey::Escape),
            |_| true,
            move |_| p.open.set(false),
        )
        .on_key_down(
            Key::Named(NamedKey::ArrowDown),
            |_| true,
            move |_| {
                let len = displayed().len();
                if len > 0 {
                    p.selected.update(|i| *i = (*i + 1).min(len - 1));
                }
            },
        )
        .on_key_down(
            Key::Named(NamedKey::ArrowUp),
            |_| true,
            move |_| {
                p.selected.update(|i| *i = i.saturating_sub(1));
            },
        );

    let rows = dyn_stack(
        displayed,
        |(i, _)| *i,
        move |(i, it)| {
            let detail = it.detail.clone();
            stack((
                label(move || it.label.clone())
                    .style(|s| s.color(theme::fg()).flex_grow(1.0).text_ellipsis()),
                label(move || detail.clone()).style(|s| s.color(theme::fg_dim()).text_ellipsis()),
            ))
            .style(move |s| {
                let s = s
                    .items_center()
                    .gap(10.0)
                    .height(24.0)
                    .width_full()
                    .padding_horiz(10.0)
                    .cursor(floem::style::CursorStyle::Pointer);
                if p.selected.get() == i {
                    s.background(theme::bg_active())
                } else {
                    s.hover(|s| s.background(theme::bg_hover()))
                }
            })
            .on_click_stop(move |_| {
                p.selected.set(i);
                p.open.set(false);
                state.jump_to(&it.uri, it.line as usize, it.char as usize);
            })
        },
    )
    .style(|s| s.flex_col().width_full());

    let rows_scroll = scroll(rows)
        .scroll_to_percent(move || {
            let n = displayed().len().max(1) as f32;
            p.selected.get() as f32 / n
        })
        .style(|s| s.max_height(360.0).width_full());

    let box_ = stack((input, rows_scroll))
        .style(|s| {
            s.flex_col()
                .width(620.0)
                .background(theme::bg_panel())
                .border(1.0)
                .border_color(theme::border())
                .border_radius(8.0)
        })
        .on_click_stop(|_| {});

    floem::views::container(box_)
        .style(move |s| {
            let s = s
                .absolute()
                .inset(0.0)
                .size_full()
                .justify_center()
                .padding_top(90.0);
            if p.open.get() {
                s
            } else {
                s.hide()
            }
        })
        .on_click_stop(move |_| p.open.set(false))
}
