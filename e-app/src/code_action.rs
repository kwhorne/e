//! The code-action picker: a small centred list of LSP quick fixes / refactors
//! (extract variable/method, etc.) offered at the cursor.

use floem::reactive::{SignalGet, SignalUpdate};
use floem::views::{container, dyn_stack, label, scroll, stack, Decorators};
use floem::IntoView;

use crate::state::AppState;
use crate::theme;

pub fn code_action_picker(state: AppState) -> impl IntoView {
    let rows = dyn_stack(
        move || {
            state
                .code_actions
                .get()
                .into_iter()
                .enumerate()
                .collect::<Vec<_>>()
        },
        |(i, _)| *i,
        move |(i, item)| {
            let title = item.title.clone();
            label(move || title.clone())
                .style(|s| {
                    s.width_full()
                        .padding_horiz(12.0)
                        .padding_vert(6.0)
                        .font_size(13.0)
                        .color(theme::fg())
                        .cursor(floem::style::CursorStyle::Pointer)
                        .hover(|s| s.background(theme::bg_hover()))
                })
                .on_click_stop(move |_| state.apply_code_action(i))
        },
    )
    .style(|s| s.flex_col().width_full());

    let card = stack((
        label(|| "Code actions".to_string()).style(|s| {
            s.font_size(11.0)
                .font_bold()
                .color(theme::fg_dim())
                .padding_horiz(12.0)
                .padding_vert(6.0)
                .border_bottom(1.0)
                .border_color(theme::border())
        }),
        scroll(rows).style(|s| s.max_height(320.0).width_full()),
    ))
    .style(|s| {
        s.flex_col()
            .width(460.0)
            .border(1.0)
            .border_color(theme::border())
            .border_radius(8.0)
            .background(theme::bg_panel())
    })
    .on_click_stop(|_| {});

    container(card)
        .style(move |s| {
            let s = s
                .absolute()
                .inset(0.0)
                .size_full()
                .justify_center()
                .items_start()
                .padding_top(120.0);
            if state.code_actions_open.get() {
                s
            } else {
                s.hide()
            }
        })
        .on_click_stop(move |_| state.code_actions_open.set(false))
}
