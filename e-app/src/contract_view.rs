//! Props contract panel: the reconciled view of what a controller sends and
//! what the page component expects, with a "Generate TypeScript" action.

use floem::peniko::Color;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use floem::views::{dyn_stack, label, scroll, stack, Decorators};
use floem::IntoView;

use crate::state::AppState;
use crate::theme;

const AMBER: Color = Color::from_rgb8(0xe5, 0xc0, 0x7b);
const RED: Color = Color::from_rgb8(0xf7, 0x76, 0x8e);
const GREEN: Color = Color::from_rgb8(0x9e, 0xce, 0x6a);

pub fn contract_panel(state: AppState) -> impl IntoView {
    let title = label(move || {
        state
            .contract
            .with(|c| c.as_ref().map(|c| format!("Props Contract — {}", c.page)))
            .unwrap_or_else(|| "Props Contract".to_string())
    })
    .style(|s| {
        s.flex_grow(1.0)
            .font_size(13.0)
            .font_bold()
            .color(theme::fg())
    });

    let gen = label(|| "Generate TypeScript".to_string())
        .style(move |s| {
            let s = s
                .padding_horiz(10.0)
                .padding_vert(3.0)
                .border_radius(4.0)
                .font_size(11.0)
                .color(theme::accent())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::bg_hover()));
            if state.contract.with(|c| c.is_some()) {
                s
            } else {
                s.hide()
            }
        })
        .on_click_stop(move |_| state.generate_contract_ts());

    let close = label(|| "✕".to_string())
        .style(|s| {
            s.padding_horiz(8.0)
                .color(theme::fg_dim())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.color(theme::fg()))
        })
        .on_click_stop(move |_| state.contract_open.set(false));

    let header = stack((title, gen, close)).style(|s| {
        s.flex_row()
            .items_center()
            .gap(8.0)
            .padding_horiz(12.0)
            .padding_vert(8.0)
            .width_full()
            .border_bottom(1.0)
            .border_color(theme::border())
    });

    let controller = label(move || {
        state
            .contract
            .with(|c| {
                c.as_ref().map(|c| {
                    let root = state.root.get_untracked();
                    let rel = c.controller.strip_prefix(&root).unwrap_or(&c.controller);
                    format!("controller: {}", rel.display())
                })
            })
            .unwrap_or_else(|| "Searching for the controller that renders this page…".to_string())
    })
    .style(|s| {
        s.padding_horiz(12.0)
            .padding_vert(6.0)
            .font_size(11.0)
            .font_family("monospace".to_string())
            .color(theme::fg_dim())
    });

    // Props sent by the controller (with inferred types).
    let sent = dyn_stack(
        move || {
            state
                .contract
                .with(|c| c.as_ref().map(|c| c.props.clone()).unwrap_or_default())
                .into_iter()
                .enumerate()
                .collect::<Vec<_>>()
        },
        |(i, _)| *i,
        move |(_, p)| {
            let flag = if p.unused {
                "  ⚠ sent but unused"
            } else {
                ""
            };
            let text = format!("{}: {}{}", p.key, p.ty, flag);
            let unused = p.unused;
            label(move || text.clone()).style(move |s| {
                let s = s
                    .font_size(12.0)
                    .font_family("monospace".to_string())
                    .padding_horiz(16.0)
                    .padding_vert(2.0)
                    .width_full();
                if unused {
                    s.color(AMBER)
                } else {
                    s.color(GREEN)
                }
            })
        },
    )
    .style(|s| s.flex_col().width_full());

    // Props the component expects but the controller never sends.
    let missing = dyn_stack(
        move || {
            state
                .contract
                .with(|c| c.as_ref().map(|c| c.missing.clone()).unwrap_or_default())
                .into_iter()
                .enumerate()
                .collect::<Vec<_>>()
        },
        |(i, _)| *i,
        move |(_, key)| {
            let text = format!("{key}  ⚠ used but never sent");
            label(move || text.clone()).style(move |s| {
                s.font_size(12.0)
                    .font_family("monospace".to_string())
                    .padding_horiz(16.0)
                    .padding_vert(2.0)
                    .width_full()
                    .color(RED)
            })
        },
    )
    .style(|s| s.flex_col().width_full());

    let not_found = label(|| "No controller renders this page (or props are dynamic).".to_string())
        .style(move |s| {
            let s = s.padding(16.0).font_size(12.0).color(theme::fg_dim());
            // Shown only once the search finished with no result.
            if state.contract_open.get() && state.contract.with(|c| c.is_none()) {
                s
            } else {
                s.hide()
            }
        });

    let body = stack((controller, sent, missing, not_found)).style(|s| s.flex_col().width_full());

    let card = stack((
        header,
        scroll(body).style(|s| s.flex_grow(1.0).width_full()),
    ))
    .style(|s| {
        s.flex_col()
            .width(640.0)
            .height(520.0)
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
        if state.contract_open.get() {
            s
        } else {
            s.hide()
        }
    })
}
