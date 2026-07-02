//! "Describe what you're looking for" search panel (semantic / local).

use floem::event::EventListener;
use floem::keyboard::{Key, NamedKey};
use floem::peniko::Color;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use floem::views::{dyn_stack, label, scroll, stack, text_input, Decorators};
use floem::IntoView;

use crate::state::AppState;
use crate::theme;

pub fn semantic_panel(state: AppState) -> impl IntoView {
    let input = text_input(state.sem_query)
        .placeholder("Describe what you're looking for… e.g. where is the invoice email sent")
        .style(|s| {
            theme::input_colors(s)
                .flex_grow(1.0)
                .font_size(13.0)
                .padding_horiz(10.0)
                .padding_vert(6.0)
        })
        .on_event_stop(EventListener::KeyDown, move |e| {
            if let floem::event::Event::KeyDown(ke) = e {
                if ke.key.logical_key == Key::Named(NamedKey::Enter) {
                    state.run_semantic_search();
                }
            }
        });

    let go = label(|| "Search".to_string())
        .style(|s| {
            s.padding_horiz(12.0)
                .padding_vert(6.0)
                .border_radius(5.0)
                .font_size(12.0)
                .color(Color::WHITE)
                .background(Color::from_rgb8(0x3b, 0x82, 0xf6))
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(Color::from_rgb8(0x2f, 0x6f, 0xe0)))
        })
        .on_click_stop(move |_| state.run_semantic_search());

    let close = label(|| "✕".to_string())
        .style(|s| {
            s.padding_horiz(8.0)
                .color(theme::fg_dim())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.color(theme::fg()))
        })
        .on_click_stop(move |_| state.sem_open.set(false));

    let header = stack((input, go, close)).style(|s| {
        s.flex_row()
            .items_center()
            .gap(8.0)
            .padding(12.0)
            .width_full()
            .border_bottom(1.0)
            .border_color(theme::border())
    });

    let status = label(move || state.sem_status.get()).style(move |s| {
        let s = s
            .padding_horiz(12.0)
            .padding_vert(4.0)
            .font_size(11.0)
            .color(theme::fg_dim());
        if state.sem_status.with(|t| t.is_empty()) {
            s.hide()
        } else {
            s
        }
    });

    let results = dyn_stack(
        move || {
            state
                .sem_results
                .get()
                .into_iter()
                .enumerate()
                .collect::<Vec<_>>()
        },
        |(i, _)| *i,
        move |(_, hit)| {
            let root = state.root.get_untracked();
            let rel = hit
                .path
                .strip_prefix(&root)
                .unwrap_or(&hit.path)
                .display()
                .to_string();
            let title = format!("{rel}:{}", hit.line);
            let snip = hit.snippet.clone();
            let hit2 = hit.clone();
            stack((
                label(move || title.clone()).style(|s| {
                    s.font_size(12.0)
                        .font_family("monospace".to_string())
                        .color(Color::from_rgb8(0x61, 0xaf, 0xef))
                }),
                label(move || snip.clone()).style(|s| s.font_size(11.0).color(theme::fg_dim())),
            ))
            .style(|s| {
                s.flex_col()
                    .gap(2.0)
                    .width_full()
                    .padding_horiz(12.0)
                    .padding_vert(6.0)
                    .cursor(floem::style::CursorStyle::Pointer)
                    .border_bottom(1.0)
                    .border_color(theme::border())
                    .hover(|s| s.background(theme::bg_hover()))
            })
            .on_click_stop(move |_| state.open_semantic_hit(&hit2))
        },
    )
    .style(|s| s.flex_col().width_full());

    let card = stack((
        header,
        status,
        scroll(results).style(|s| s.flex_grow(1.0).width_full()),
    ))
    .style(|s| {
        s.flex_col()
            .width(720.0)
            .height(560.0)
            .border(1.0)
            .border_color(theme::border())
            .border_radius(10.0)
            .background(theme::bg())
    });

    floem::views::container(card).style(move |s| {
        let s = s
            .absolute()
            .inset(0.0)
            .size_full()
            .items_center()
            .justify_center()
            .background(Color::from_rgba8(0, 0, 0, 0xCC));
        if state.sem_open.get() {
            s
        } else {
            s.hide()
        }
    })
}
