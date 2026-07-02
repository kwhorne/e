//! Event dispatch graph: events as nodes, their listeners as clickable edges.

use floem::peniko::Color;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use floem::views::{dyn_stack, label, scroll, stack, Decorators};
use floem::IntoView;

use crate::state::AppState;
use crate::theme;

pub fn event_graph_panel(state: AppState) -> impl IntoView {
    let title = label(|| "Event Dispatch Graph".to_string()).style(|s| {
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
        .on_click_stop(move |_| state.event_open.set(false));
    let header = stack((title, close)).style(|s| {
        s.flex_row()
            .items_center()
            .padding_horiz(12.0)
            .padding_vert(8.0)
            .width_full()
            .border_bottom(1.0)
            .border_color(theme::border())
    });

    let nodes = dyn_stack(
        move || {
            state
                .event_graph
                .get()
                .into_iter()
                .enumerate()
                .collect::<Vec<_>>()
        },
        |(i, _)| *i,
        move |(_, node)| {
            let event = node.event.clone();
            let event_file = node.event_file.clone();
            let head = label(move || format!("⚡ {event}"))
                .style(move |s| {
                    let s = s
                        .font_size(13.0)
                        .font_bold()
                        .color(Color::from_rgb8(0xe5, 0xc0, 0x7b))
                        .padding_vert(2.0);
                    if event_file.is_some() {
                        s.cursor(floem::style::CursorStyle::Pointer)
                            .hover(|s| s.color(theme::fg()))
                    } else {
                        s
                    }
                })
                .on_click_stop({
                    let f = node.event_file.clone();
                    move |_| {
                        if let Some(f) = &f {
                            state.open_event_file(f.clone());
                        }
                    }
                });

            let listeners = dyn_stack(
                {
                    let ls = node.listeners.clone();
                    move || ls.clone().into_iter().enumerate().collect::<Vec<_>>()
                },
                |(i, _)| *i,
                move |(_, (name, file))| {
                    let text = format!("    → {name}");
                    let f = file.clone();
                    let clickable = f.is_some();
                    label(move || text.clone())
                        .style(move |s| {
                            let s = s
                                .font_size(12.0)
                                .font_family("monospace".to_string())
                                .padding_left(16.0)
                                .padding_vert(1.0)
                                .color(theme::fg());
                            if clickable {
                                s.cursor(floem::style::CursorStyle::Pointer)
                                    .hover(|s| s.background(theme::bg_hover()))
                            } else {
                                s.color(theme::fg_dim())
                            }
                        })
                        .on_click_stop(move |_| {
                            if let Some(f) = &f {
                                state.open_event_file(f.clone());
                            }
                        })
                },
            )
            .style(|s| s.flex_col().width_full());

            stack((head, listeners)).style(|s| {
                s.flex_col()
                    .width_full()
                    .padding_horiz(12.0)
                    .padding_vert(6.0)
                    .border_bottom(1.0)
                    .border_color(theme::border())
            })
        },
    )
    .style(|s| s.flex_col().width_full());

    let empty = label(|| "No events with listeners found.".to_string()).style(move |s| {
        let s = s.padding(16.0).color(theme::fg_dim()).font_size(12.0);
        if state.event_graph.with(|g| g.is_empty()) {
            s
        } else {
            s.hide()
        }
    });

    let card = stack((
        header,
        empty,
        scroll(nodes).style(|s| s.flex_grow(1.0).width_full()),
    ))
    .style(|s| {
        s.flex_col()
            .width(640.0)
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
        if state.event_open.get() {
            s
        } else {
            s.hide()
        }
    })
}
