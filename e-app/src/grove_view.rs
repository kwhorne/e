//! The Grove panel: the app's outgoing mail (Grove's mail-catcher) and the
//! webhooks delivered to it (Grove's webhook hub), with re-delivery.

use floem::peniko::Color;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use floem::views::{dyn_stack, label, scroll, stack, Decorators};
use floem::IntoView;

use crate::grove_state::GroveTab;
use crate::state::AppState;
use crate::theme;

const AMBER: Color = Color::from_rgb8(0xe5, 0xc0, 0x7b);

fn tab(state: AppState, which: GroveTab, text: &'static str) -> impl IntoView {
    label(move || text.to_string())
        .style(move |s| {
            let s = s
                .padding_horiz(10.0)
                .padding_vert(3.0)
                .border_radius(4.0)
                .font_size(12.0)
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::bg_hover()).color(theme::fg()));
            if state.grove_tab.get() == which {
                s.background(theme::bg_active()).color(theme::fg())
            } else {
                s.color(theme::fg_dim())
            }
        })
        .on_click_stop(move |_| {
            state.grove_tab.set(which);
            state.grove_selected.set(None);
            state.grove_detail.set(String::new());
        })
}

fn pill(text: &'static str, on_click: impl Fn() + 'static) -> impl IntoView {
    label(move || text.to_string())
        .style(|s| {
            s.padding_horiz(10.0)
                .padding_vert(3.0)
                .border_radius(4.0)
                .font_size(11.0)
                .color(theme::fg_dim())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::bg_hover()).color(theme::fg()))
        })
        .on_click_stop(move |_| on_click())
}

/// One row: `(id, primary, secondary, is_selected)`.
fn row(
    state: AppState,
    tab: GroveTab,
    id: u64,
    primary: String,
    secondary: String,
) -> impl IntoView {
    stack((
        label(move || primary.clone()).style(|s| {
            s.font_size(12.0)
                .color(theme::fg())
                .flex_grow(1.0_f32)
                .min_width(0.0)
                .text_ellipsis()
        }),
        label(move || secondary.clone()).style(|s| {
            s.font_size(11.0)
                .font_family("monospace".to_string())
                .color(theme::fg_dim())
                .flex_shrink(0.0_f32)
        }),
    ))
    .style(move |s| {
        let s = s
            .flex_row()
            .items_center()
            .gap(10.0)
            .width_full()
            .padding_horiz(12.0)
            .padding_vert(5.0)
            .cursor(floem::style::CursorStyle::Pointer)
            .hover(|s| s.background(theme::bg_hover()));
        if state.grove_selected.get() == Some((tab, id)) {
            s.background(theme::bg_active())
        } else {
            s
        }
    })
    .on_click_stop(move |_| state.grove_select(tab, id))
}

pub fn grove_panel(state: AppState) -> impl IntoView {
    let title =
        label(|| "Grove".to_string()).style(|s| s.font_size(13.0).font_bold().color(theme::fg()));
    let tabs = stack((
        tab(state, GroveTab::Mail, "Mail"),
        tab(state, GroveTab::Hooks, "Webhooks"),
    ))
    .style(|s| s.flex_row().gap(4.0).margin_left(12.0).flex_grow(1.0_f32));
    let agent = pill("✨ Agent", move || state.grove_to_agent());
    let replay = label(|| "↻ Re-deliver".to_string())
        .style(move |s| {
            let s = s
                .padding_horiz(10.0)
                .padding_vert(3.0)
                .border_radius(4.0)
                .font_size(11.0)
                .color(AMBER)
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::bg_hover()));
            match state.grove_selected.get() {
                Some((GroveTab::Hooks, _)) => s,
                _ => s.hide(),
            }
        })
        .on_click_stop(move |_| {
            if let Some((GroveTab::Hooks, id)) = state.grove_selected.get_untracked() {
                state.grove_replay_hook(id);
            }
        });
    let close = label(|| "✕".to_string())
        .style(|s| {
            s.padding_horiz(8.0)
                .color(theme::fg_dim())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.color(theme::fg()))
        })
        .on_click_stop(move |_| state.grove_panel_open.set(false));
    let header = stack((title, tabs, replay, agent, close)).style(|s| {
        s.flex_row()
            .items_center()
            .gap(8.0)
            .padding_horiz(12.0)
            .padding_vert(8.0)
            .width_full()
            .border_bottom(1.0)
            .border_color(theme::border())
    });

    // The list: mail or webhooks, whichever tab is active.
    let rows = dyn_stack(
        move || match state.grove_tab.get() {
            GroveTab::Mail => state
                .grove_mail
                .get()
                .into_iter()
                .map(|m| {
                    (
                        (GroveTab::Mail, m.id),
                        format!("{} → {}", m.subject, m.to.join(", ")),
                        m.received_at
                            .get(11..19)
                            .unwrap_or(&m.received_at)
                            .to_string(),
                    )
                })
                .collect::<Vec<_>>(),
            GroveTab::Hooks => state
                .grove_hooks
                .get()
                .into_iter()
                .map(|h| {
                    (
                        (GroveTab::Hooks, h.id),
                        format!("{} {}", h.method, h.path),
                        format!("{} · {}", h.time, h.status),
                    )
                })
                .collect::<Vec<_>>(),
        },
        |(key, _, _)| *key,
        move |((tab, id), primary, secondary)| row(state, tab, id, primary, secondary),
    )
    .style(|s| s.flex_col().width_full());

    let empty = label(move || match state.grove_tab.get() {
        GroveTab::Mail => {
            "No mail captured yet. Point the app's MAIL_* at Grove (`grove env`) and send one."
                .to_string()
        }
        GroveTab::Hooks => {
            "No webhooks captured yet. Send one to /__grove/hooks/<bucket> on the site (or via `grove share`)."
                .to_string()
        }
    })
    .style(move |s| {
        let s = s.padding(14.0).color(theme::fg_dim()).font_size(12.0);
        let empty = match state.grove_tab.get() {
            GroveTab::Mail => state.grove_mail.with(|m| m.is_empty()),
            GroveTab::Hooks => state.grove_hooks.with(|h| h.is_empty()),
        };
        if empty {
            s
        } else {
            s.hide()
        }
    });

    let detail = scroll(label(move || state.grove_detail.get()).style(|s| {
        s.font_size(12.0)
            .font_family("monospace".to_string())
            .color(theme::fg())
            .padding(12.0)
            .width_full()
    }))
    .style(move |s| {
        let s = s
            .width_full()
            .height(220.0)
            .border_top(1.0)
            .border_color(theme::border());
        if state.grove_selected.get().is_some() {
            s
        } else {
            s.hide()
        }
    });

    let card = stack((
        header,
        empty,
        scroll(rows).style(|s| s.flex_grow(1.0_f32).width_full()),
        detail,
    ))
    .style(|s| {
        s.flex_col()
            .width(880.0)
            .height(600.0)
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
            .background(Color::from_rgba8(0, 0, 0, 96));
        if state.grove_panel_open.get() {
            s
        } else {
            s.hide()
        }
    })
}
