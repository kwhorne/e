//! Laravel architecture map: an interactive flow of route → controller → views.
//! Each card is clickable and jumps to the relevant file.

use floem::peniko::Color;
use floem::reactive::{SignalGet, SignalUpdate};
use floem::views::{container, dyn_stack, label, scroll, stack, text_input, Decorators};
use floem::IntoView;

use crate::laravel::{self, Helper};
use crate::state::AppState;
use crate::theme;

/// A single resolved route row: `(name, methods, uri, action, views)`.
type MapRow = (String, String, String, String, Vec<String>);

fn card(text: String, accent: bool, pointer: bool) -> impl IntoView {
    label(move || text.clone()).style(move |s| {
        let s = s
            .padding_horiz(8.0)
            .padding_vert(3.0)
            .border_radius(5.0)
            .font_size(12.0)
            .border(1.0)
            .border_color(theme::border())
            .text_ellipsis()
            .max_width(280.0);
        let s = if accent {
            s.color(theme::accent())
        } else {
            s.color(theme::fg())
        };
        if pointer {
            s.cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::bg_hover()))
        } else {
            s.color(theme::fg_dim())
        }
    })
}

fn arrow() -> impl IntoView {
    label(|| "→".to_string()).style(|s| s.color(theme::fg_dim()).padding_horiz(2.0))
}

pub fn laravel_map(state: AppState) -> impl IntoView {
    let title = label(|| "Laravel Map".to_string()).style(|s| {
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
        .on_click_stop(move |_| state.map_open.set(false));
    let header = stack((title, close)).style(|s| {
        s.flex_row()
            .items_center()
            .padding_horiz(12.0)
            .padding_vert(8.0)
            .width_full()
            .border_bottom(1.0)
            .border_color(theme::border())
    });

    let filter = text_input(state.map_query)
        .placeholder("Filter routes…")
        .style(|s| {
            theme::input_colors(s)
                .width_full()
                .font_size(12.0)
                .padding_horiz(8.0)
                .padding_vert(5.0)
                .margin(10.0)
        });

    let rows = dyn_stack(
        move || -> Vec<MapRow> {
            let Some(data) = state.laravel.get() else {
                return Vec::new();
            };
            let q = state.map_query.get().to_lowercase();
            data.routes
                .iter()
                .filter(|r| {
                    q.is_empty()
                        || r.name.to_lowercase().contains(&q)
                        || r.uri.to_lowercase().contains(&q)
                        || r.action.to_lowercase().contains(&q)
                })
                .take(60)
                .map(|r| {
                    let views = laravel::route_views(&data, &r.action);
                    (
                        r.name.clone(),
                        r.methods.clone(),
                        r.uri.clone(),
                        r.action.clone(),
                        views,
                    )
                })
                .collect()
        },
        |r| r.0.clone(),
        move |(name, methods, uri, action, views)| {
            // URI card (non-clickable).
            let uri_card = card(format!("{methods}  /{uri}"), false, false);

            // Controller card → jump to the route's controller method.
            let ctrl_label = action.rsplit('\\').next().unwrap_or(&action).to_string();
            let route_name = name.clone();
            let ctrl = card(ctrl_label, true, true).on_click_stop(move |_| {
                if let Some(data) = state.laravel.get_untracked() {
                    if let Some((p, l, c)) = laravel::navigate(&data, Helper::Route, &route_name) {
                        state.jump_to(&format!("file://{}", p.display()), l, c);
                    }
                }
            });

            // View cards → open the blade file.
            let view_cards = dyn_stack(
                {
                    let views = views.clone();
                    move || views.clone().into_iter().enumerate().collect::<Vec<_>>()
                },
                |(i, _)| *i,
                move |(_, v)| {
                    let vname = v.clone();
                    card(v.clone(), false, true).on_click_stop(move |_| {
                        if let Some(data) = state.laravel.get_untracked() {
                            if let Some((p, l, c)) = laravel::navigate(&data, Helper::View, &vname)
                            {
                                state.jump_to(&format!("file://{}", p.display()), l, c);
                            }
                        }
                    })
                },
            )
            .style(|s| s.flex_row().gap(4.0).items_center());

            let name_lbl = label(move || name.clone())
                .style(|s| s.font_size(10.0).color(theme::fg_dim()).min_width(120.0));

            stack((
                name_lbl,
                uri_card.into_any(),
                arrow().into_any(),
                ctrl.into_any(),
                arrow().into_any(),
                view_cards.into_any(),
            ))
            .style(|s| {
                s.flex_row()
                    .items_center()
                    .gap(6.0)
                    .padding_horiz(12.0)
                    .padding_vert(5.0)
                    .width_full()
                    .border_bottom(1.0)
                    .border_color(theme::border())
            })
        },
    )
    .style(|s| s.flex_col().width_full());

    let empty_hint = label(|| "No routes (open a Laravel project).".to_string()).style(move |s| {
        let s = s.padding(16.0).color(theme::fg_dim()).font_size(12.0);
        let has = state
            .laravel
            .get()
            .map(|d| !d.routes.is_empty())
            .unwrap_or(false);
        if has {
            s.hide()
        } else {
            s
        }
    });

    let body = stack((
        filter,
        empty_hint,
        scroll(rows).style(|s| s.flex_grow(1.0).width_full()),
    ))
    .style(|s| s.flex_col().flex_grow(1.0).width_full());

    let card_box = stack((header, body)).style(|s| {
        s.flex_col()
            .width(920.0)
            .height(620.0)
            .border(1.0)
            .border_color(theme::border())
            .border_radius(10.0)
            .background(theme::bg())
    });

    container(card_box).style(move |s| {
        let s = s
            .absolute()
            .inset(0.0)
            .size_full()
            .items_center()
            .justify_center()
            .background(Color::from_rgba8(0, 0, 0, 120));
        if state.map_open.get() {
            s
        } else {
            s.hide()
        }
    })
}
