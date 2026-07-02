//! Eloquent relationship graph panel: models as nodes, their relationships as
//! edges, with mismatches (relation in code but no foreign key in the DB)
//! flagged.

use floem::peniko::Color;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use floem::views::{dyn_stack, label, scroll, stack, Decorators};
use floem::IntoView;

use crate::state::AppState;
use crate::theme;

const WARN: Color = Color::from_rgb8(0xf7, 0x76, 0x8e);

fn tool(text: &'static str, on: impl Fn() + 'static) -> impl IntoView {
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
        .on_click_stop(move |_| on())
}

pub fn relation_graph_panel(state: AppState) -> impl IntoView {
    let title = label(|| "Eloquent Relations".to_string()).style(|s| {
        s.flex_grow(1.0)
            .font_size(13.0)
            .font_bold()
            .color(theme::fg())
    });
    let warn_summary = label(move || {
        let n = state.rel_graph.with(|g| {
            g.iter()
                .flat_map(|m| &m.relations)
                .filter(|r| !r.ok)
                .count()
        });
        if n > 0 {
            format!("⚠ {n} without FK")
        } else {
            String::new()
        }
    })
    .style(move |s| {
        let s = s.margin_right(8.0).font_size(12.0).color(WARN);
        let n = state.rel_graph.with(|g| {
            g.iter()
                .flat_map(|m| &m.relations)
                .filter(|r| !r.ok)
                .count()
        });
        if n > 0 {
            s
        } else {
            s.hide()
        }
    });
    let refresh = tool("↻ Refresh", move || state.compute_relations());
    let close = label(|| "✕".to_string())
        .style(|s| {
            s.padding_horiz(8.0)
                .color(theme::fg_dim())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.color(theme::fg()))
        })
        .on_click_stop(move |_| state.rel_open.set(false));
    let header = stack((title, warn_summary, refresh, close)).style(|s| {
        s.flex_row()
            .items_center()
            .gap(8.0)
            .padding_horiz(12.0)
            .padding_vert(8.0)
            .width_full()
            .border_bottom(1.0)
            .border_color(theme::border())
    });

    let nodes = dyn_stack(
        move || {
            state
                .rel_graph
                .get()
                .into_iter()
                .enumerate()
                .collect::<Vec<_>>()
        },
        |(i, _)| *i,
        move |(_, node)| {
            let file = node.file.clone();
            let head = stack((
                label({
                    let n = node.name.clone();
                    move || n.clone()
                })
                .style(|s| {
                    s.font_size(13.0)
                        .font_bold()
                        .color(Color::from_rgb8(0x61, 0xaf, 0xef))
                        .cursor(floem::style::CursorStyle::Pointer)
                        .hover(|s| s.color(theme::fg()))
                })
                .on_click_stop(move |_| {
                    state.jump_to(&format!("file://{}", file.display()), 0, 0);
                }),
                label({
                    let t = node.table.clone();
                    move || format!("· {t}")
                })
                .style(|s| s.font_size(11.0).color(theme::fg_dim())),
            ))
            .style(|s| s.flex_row().items_center().gap(6.0).margin_bottom(2.0));

            let node_file = node.file.clone();
            let rels = dyn_stack(
                {
                    let relations = node.relations.clone();
                    move || {
                        relations
                            .clone()
                            .into_iter()
                            .enumerate()
                            .collect::<Vec<_>>()
                    }
                },
                |(i, _)| *i,
                move |(_, r)| {
                    let flag = if r.ok { "" } else { "  ⚠ no FK" };
                    let text = format!("{}()  {} → {}{}", r.method, r.kind, r.target, flag);
                    let ok = r.ok;
                    let line = r.line;
                    let mfile = node_file.clone();
                    let tfile = r.target_file.clone();
                    label(move || text.clone())
                        .style(move |s| {
                            let s = s
                                .font_size(12.0)
                                .font_family("monospace".to_string())
                                .padding_left(16.0)
                                .padding_vert(1.0)
                                .cursor(floem::style::CursorStyle::Pointer)
                                .hover(|s| s.background(theme::bg_hover()));
                            if ok {
                                s.color(theme::fg())
                            } else {
                                s.color(WARN)
                            }
                        })
                        .on_click_stop(move |_| {
                            // Jump to the related model if known, else the method.
                            if let Some(tf) = &tfile {
                                state.jump_to(&format!("file://{}", tf.display()), 0, 0);
                            } else {
                                state.jump_to(
                                    &format!("file://{}", mfile.display()),
                                    line.saturating_sub(1),
                                    0,
                                );
                            }
                        })
                },
            )
            .style(|s| s.flex_col().width_full());

            stack((head, rels)).style(|s| {
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

    let empty_hint = label(|| "No model relationships found (open a Laravel project).".to_string())
        .style(move |s| {
            let s = s.padding(16.0).color(theme::fg_dim()).font_size(12.0);
            if state.rel_graph.with(|g| g.is_empty()) {
                s
            } else {
                s.hide()
            }
        });

    let card = stack((
        header,
        empty_hint,
        scroll(nodes).style(|s| s.flex_grow(1.0).width_full()),
    ))
    .style(|s| {
        s.flex_col()
            .width(760.0)
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
            .background(Color::from_rgba8(0, 0, 0, 0xCC));
        if state.rel_open.get() {
            s
        } else {
            s.hide()
        }
    })
}
