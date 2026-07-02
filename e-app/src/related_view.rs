//! Related-files picker: a quick list of the model/migration/factory/controller/
//! test/… that belong to the same resource as the active file.

use floem::peniko::Color;
use floem::reactive::{SignalGet, SignalUpdate};
use floem::views::{dyn_stack, label, scroll, stack, Decorators};
use floem::IntoView;

use crate::state::AppState;
use crate::theme;

pub fn related_panel(state: AppState) -> impl IntoView {
    let title = label(|| "Related files".to_string()).style(|s| {
        s.flex_grow(1.0)
            .font_size(13.0)
            .font_bold()
            .color(theme::fg())
    });
    let close = label(|| "✕".to_string())
        .style(|s| {
            s.padding_horiz(8.0)
                .color(theme::fg_dim())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.color(theme::fg()))
        })
        .on_click_stop(move |_| state.related_open.set(false));
    let header = stack((title, close)).style(|s| {
        s.flex_row()
            .items_center()
            .padding_horiz(12.0)
            .padding_vert(8.0)
            .width_full()
            .border_bottom(1.0)
            .border_color(theme::border())
    });

    let rows = dyn_stack(
        move || {
            state
                .related_items
                .get()
                .into_iter()
                .enumerate()
                .collect::<Vec<_>>()
        },
        |(i, _)| *i,
        move |(_, (kind, path))| {
            let root = state.root.get_untracked();
            let rel = path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .display()
                .to_string();
            let p = path.clone();
            stack((
                label(move || kind.clone()).style(|s| {
                    s.width(90.0)
                        .font_size(11.0)
                        .color(Color::from_rgb8(0x61, 0xaf, 0xef))
                }),
                label(move || rel.clone()).style(|s| {
                    s.flex_grow(1.0)
                        .font_size(12.0)
                        .font_family("monospace".to_string())
                        .color(theme::fg())
                        .text_ellipsis()
                }),
            ))
            .style(|s| {
                s.flex_row()
                    .items_center()
                    .gap(8.0)
                    .width_full()
                    .padding_horiz(12.0)
                    .padding_vert(6.0)
                    .cursor(floem::style::CursorStyle::Pointer)
                    .hover(|s| s.background(theme::bg_hover()))
            })
            .on_click_stop(move |_| state.open_related(p.clone()))
        },
    )
    .style(|s| s.flex_col().width_full());

    let card = stack((
        header,
        scroll(rows).style(|s| s.flex_grow(1.0).width_full()),
    ))
    .style(|s| {
        s.flex_col()
            .width(560.0)
            .max_height(420.0)
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
        if state.related_open.get() {
            s
        } else {
            s.hide()
        }
    })
}
