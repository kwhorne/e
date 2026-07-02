//! Request-replay response overlay: status, body, and captured SQL queries
//! (with N+1 detection), launched from the architecture map.

use std::collections::HashMap;

use floem::peniko::Color;
use floem::reactive::{SignalGet, SignalWith};
use floem::views::{dyn_stack, label, scroll, stack, Decorators};
use floem::IntoView;

use crate::state::AppState;
use crate::theme;

/// Normalise a query so repeated queries with different literals group together.
fn normalize(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len());
    let mut chars = sql.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_ascii_digit() {
            while chars.peek().map(|c| c.is_ascii_digit()).unwrap_or(false) {
                chars.next();
            }
            out.push('?');
        } else if c == '\'' {
            for x in chars.by_ref() {
                if x == '\'' {
                    break;
                }
            }
            out.push('?');
        } else {
            out.push(c);
        }
    }
    out
}

pub fn request_view(state: AppState) -> impl IntoView {
    let title = label(move || state.req_url.get()).style(|s| {
        s.flex_grow(1.0)
            .font_size(12.0)
            .font_family("monospace".to_string())
            .color(theme::fg())
            .text_ellipsis()
    });

    let status = label(move || match state.req_status.get() {
        Some(code) => format!("{code}"),
        None if state.req_running.get() => "…".to_string(),
        None => String::new(),
    })
    .style(move |s| {
        let s = s
            .font_size(12.0)
            .font_bold()
            .padding_horiz(8.0)
            .padding_vert(2.0)
            .border_radius(4.0);
        match state.req_status.get() {
            Some(c) if (200..300).contains(&c) => s.color(Color::from_rgb8(0x9e, 0xce, 0x6a)),
            Some(_) => s.color(Color::from_rgb8(0xf7, 0x76, 0x8e)),
            None => s.color(theme::fg_dim()),
        }
    });

    let time = label(move || {
        let t = state.req_time.get();
        if t.is_empty() {
            String::new()
        } else {
            format!("{t}s")
        }
    })
    .style(|s| s.font_size(11.0).color(theme::fg_dim()));

    let explain = label(|| "✨ Explain".to_string())
        .style(|s| {
            s.font_size(11.0)
                .padding_horiz(8.0)
                .padding_vert(2.0)
                .border_radius(4.0)
                .color(theme::accent())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::bg_hover()))
        })
        .on_click_stop(move |_| {
            let url = state.req_url.get_untracked();
            let status = state
                .req_status
                .get_untracked()
                .map(|c| c.to_string())
                .unwrap_or_default();
            let qn = state.req_queries.with_untracked(|q| q.len());
            state.send_to_agent(&format!(
                "Analyze this request. URL: {url} (status {status}). It ran {qn} SQL queries. \
                 Point out any N+1 problems and how to fix them."
            ));
        });

    let close = label(|| "✕".to_string())
        .style(|s| {
            s.padding_horiz(8.0)
                .color(theme::fg_dim())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.color(theme::fg()))
        })
        .on_click_stop(move |_| state.close_request());

    let header = stack((title, status, time, explain, close)).style(|s| {
        s.flex_row()
            .items_center()
            .gap(10.0)
            .padding_horiz(12.0)
            .padding_vert(8.0)
            .width_full()
            .border_bottom(1.0)
            .border_color(theme::border())
    });

    // Query summary bar with N+1 warning.
    let qsummary = label(move || {
        let qs = state.req_queries.get();
        if qs.is_empty() {
            if state.req_running.get() {
                String::new()
            } else {
                "No queries captured (install laravel/clockwork to see SQL).".to_string()
            }
        } else {
            let mut counts: HashMap<String, usize> = HashMap::new();
            for (sql, _) in &qs {
                *counts.entry(normalize(sql)).or_default() += 1;
            }
            let dupes = counts.values().filter(|&&c| c > 1).count();
            let worst = counts.values().copied().max().unwrap_or(1);
            if dupes > 0 {
                format!(
                    "{} queries · ⚠ possible N+1: a query ran {worst}×",
                    qs.len()
                )
            } else {
                format!("{} queries", qs.len())
            }
        }
    })
    .style(move |s| {
        let s = s.font_size(11.0).padding_horiz(12.0).padding_vert(4.0);
        let has_dupe = state.req_queries.with(|qs| {
            let mut counts: HashMap<String, usize> = HashMap::new();
            for (sql, _) in qs {
                *counts.entry(normalize(sql)).or_default() += 1;
            }
            counts.values().any(|&c| c > 1)
        });
        if has_dupe {
            s.color(Color::from_rgb8(0xe5, 0xc0, 0x7b))
        } else {
            s.color(theme::fg_dim())
        }
    });

    let queries = dyn_stack(
        move || {
            state
                .req_queries
                .get()
                .into_iter()
                .enumerate()
                .collect::<Vec<_>>()
        },
        |(i, _)| *i,
        move |(_, (sql, dur))| {
            stack((
                label(move || sql.clone()).style(|s| {
                    s.flex_grow(1.0)
                        .font_family("monospace".to_string())
                        .font_size(11.0)
                        .color(theme::fg())
                        .text_ellipsis()
                }),
                label(move || dur.clone())
                    .style(|s| s.font_size(10.0).color(theme::fg_dim()).min_width(60.0)),
            ))
            .style(|s| {
                s.flex_row()
                    .items_center()
                    .gap(8.0)
                    .width_full()
                    .padding_horiz(12.0)
                    .padding_vert(2.0)
            })
        },
    )
    .style(|s| s.flex_col().width_full().max_height(180.0));

    let body = scroll(label(move || state.req_body.get()).style(|s| {
        s.font_family("monospace".to_string())
            .font_size(12.0)
            .padding(10.0)
            .color(theme::fg())
    }))
    .style(|s| {
        s.flex_grow(1.0)
            .width_full()
            .border_top(1.0)
            .border_color(theme::border())
    });

    let err = label(move || state.req_error.get().unwrap_or_default()).style(move |s| {
        let s = s
            .padding_horiz(12.0)
            .padding_vert(4.0)
            .font_size(11.0)
            .color(Color::from_rgb8(0xf7, 0x76, 0x8e));
        if state.req_error.get().is_some() {
            s
        } else {
            s.hide()
        }
    });

    let card = stack((
        header,
        err,
        qsummary,
        scroll(queries).style(|s| s.max_height(180.0).width_full()),
        body,
    ))
    .style(|s| {
        s.flex_col()
            .width(860.0)
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
        if state.req_open.get() {
            s
        } else {
            s.hide()
        }
    })
}
