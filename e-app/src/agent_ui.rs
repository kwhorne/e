//! Agent-socket UI: the diff-review overlay for agent-proposed edits, and the
//! agent audit timeline.

use floem::peniko::Color;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::views::{dyn_stack, empty, label, scroll, stack, Decorators};
use floem::IntoView;

use crate::state::{AppState, EditSeg};
use crate::theme;

const RED: Color = Color::from_rgb8(0xf7, 0x76, 0x8e);
const GREEN: Color = Color::from_rgb8(0x9e, 0xce, 0x6a);

fn code_block(text: String, color: Color, bg: Color) -> impl IntoView {
    label(move || text.trim_end_matches('\n').to_string()).style(move |s| {
        s.font_family("monospace".to_string())
            .font_size(12.0)
            .padding_horiz(8.0)
            .padding_vert(3.0)
            .width_full()
            .color(color)
            .background(bg)
    })
}

/// The hunk-by-hunk review overlay for an agent-proposed edit.
pub fn agent_edit_review(state: AppState) -> impl IntoView {
    let title = label(move || match state.agent_edit.get() {
        Some(e) => {
            let n = e
                .segs
                .iter()
                .filter(|s| matches!(s, EditSeg::Change { .. }))
                .count();
            format!(
                "Agent proposes {n} change(s) — {}",
                e.path
                    .file_name()
                    .map(|f| f.to_string_lossy().into_owned())
                    .unwrap_or_default()
            )
        }
        None => String::new(),
    })
    .style(|s| {
        s.flex_grow(1.0)
            .font_size(13.0)
            .font_bold()
            .color(theme::fg())
    });

    let hunks = dyn_stack(
        move || -> Vec<(usize, String, String, RwSignal<bool>)> {
            match state.agent_edit.get() {
                Some(e) => e
                    .segs
                    .iter()
                    .filter_map(|s| match s {
                        EditSeg::Change { old, new, accepted } => {
                            Some((old.clone(), new.clone(), *accepted))
                        }
                        _ => None,
                    })
                    .enumerate()
                    .map(|(i, (o, n, a))| (i, o, n, a))
                    .collect(),
                None => Vec::new(),
            }
        },
        |(i, _, _, _)| *i,
        move |(_, old, new, accepted)| {
            let toggle = label(move || {
                if accepted.get() {
                    "✓ Accepted".to_string()
                } else {
                    "✗ Rejected".to_string()
                }
            })
            .style(move |s| {
                let s = s
                    .padding_horiz(8.0)
                    .padding_vert(2.0)
                    .border_radius(4.0)
                    .font_size(11.0)
                    .cursor(floem::style::CursorStyle::Pointer);
                if accepted.get() {
                    s.color(GREEN)
                } else {
                    s.color(theme::fg_dim())
                }
            })
            .on_click_stop(move |_| accepted.update(|a| *a = !*a));

            let mut blocks: Vec<floem::AnyView> = Vec::new();
            if !old.trim().is_empty() {
                blocks.push(
                    code_block(old, RED, Color::from_rgba8(0xf7, 0x76, 0x8e, 0x22)).into_any(),
                );
            }
            if !new.trim().is_empty() {
                blocks.push(
                    code_block(new, GREEN, Color::from_rgba8(0x9e, 0xce, 0x6a, 0x22)).into_any(),
                );
            }
            let diff = floem::views::stack_from_iter(blocks).style(|s| s.flex_col().width_full());

            stack((
                stack((
                    label(|| "Hunk".to_string())
                        .style(|s| s.flex_grow(1.0).font_size(11.0).color(theme::fg_dim())),
                    toggle,
                ))
                .style(|s| s.flex_row().items_center().width_full().padding_vert(2.0)),
                diff,
            ))
            .style(|s| {
                s.flex_col()
                    .width_full()
                    .margin_bottom(8.0)
                    .border(1.0)
                    .border_color(theme::border())
                    .border_radius(6.0)
                    .padding(6.0)
            })
        },
    )
    .style(|s| s.flex_col().width_full());

    let cancel = pill("Cancel", false, move || state.agent_edit_cancel());
    let apply = pill("Apply accepted", true, move || state.agent_edit_apply());
    let buttons = stack((empty().style(|s| s.flex_grow(1.0)), cancel, apply)).style(|s| {
        s.flex_row()
            .gap(8.0)
            .items_center()
            .width_full()
            .margin_top(8.0)
    });

    let card = stack((
        title,
        scroll(hunks).style(|s| s.flex_grow(1.0).width_full().margin_vert(8.0)),
        buttons,
    ))
    .style(|s| {
        s.flex_col()
            .width(760.0)
            .height(560.0)
            .padding(18.0)
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
        if state.agent_edit.get().is_some() {
            s
        } else {
            s.hide()
        }
    })
}

fn pill(text: &'static str, primary: bool, on: impl Fn() + 'static) -> impl IntoView {
    label(move || text.to_string())
        .style(move |s| {
            let s = s
                .padding_horiz(16.0)
                .height(30.0)
                .items_center()
                .border_radius(5.0)
                .font_size(12.0)
                .cursor(floem::style::CursorStyle::Pointer);
            if primary {
                s.background(theme::accent())
                    .color(Color::from_rgb8(0x14, 0x16, 0x1b))
            } else {
                s.border(1.0)
                    .border_color(theme::border())
                    .color(theme::fg())
                    .hover(|s| s.background(theme::bg_hover()))
            }
        })
        .on_click_stop(move |_| on())
}

/// The agent audit timeline overlay.
pub fn agent_log_panel(state: AppState) -> impl IntoView {
    let title = label(|| "Agent Timeline".to_string()).style(|s| {
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
        .on_click_stop(move |_| state.agent_log_open.set(false));
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
                .agent_log
                .get()
                .into_iter()
                .rev()
                .enumerate()
                .collect::<Vec<_>>()
        },
        |(i, _)| *i,
        move |(_, (time, method, summary))| {
            stack((
                label(move || time.clone())
                    .style(|s| s.width(70.0).color(theme::fg_dim()).font_size(11.0)),
                label(move || method.clone())
                    .style(|s| s.width(120.0).color(theme::accent()).font_size(12.0)),
                label(move || summary.clone()).style(|s| {
                    s.flex_grow(1.0)
                        .color(theme::fg())
                        .font_size(12.0)
                        .text_ellipsis()
                }),
            ))
            .style(|s| {
                s.flex_row()
                    .items_center()
                    .gap(8.0)
                    .width_full()
                    .padding_horiz(12.0)
                    .padding_vert(4.0)
            })
        },
    )
    .style(|s| s.flex_col().width_full());

    let empty_hint = label(|| "No agent activity yet.".to_string()).style(move |s| {
        let s = s.padding(16.0).color(theme::fg_dim()).font_size(12.0);
        if state.agent_log.with(|l| l.is_empty()) {
            s
        } else {
            s.hide()
        }
    });

    let card = stack((
        header,
        empty_hint,
        scroll(rows).style(|s| s.flex_grow(1.0).width_full()),
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
        if state.agent_log_open.get() {
            s
        } else {
            s.hide()
        }
    })
}
