//! Continuous "Runtime" panel: captures every request against the dev app while
//! you work — from Grove's proxy when it serves the project (queries with
//! `grove sql-capture`, mail, matching error-log entries), else via Clockwork —
//! so you don't need Telescope or Debugbar installed.

use std::collections::HashMap;

use floem::peniko::Color;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use floem::views::{dyn_stack, label, scroll, stack, Decorators};
use floem::IntoView;

use crate::runtime::{DetailLine, RuntimeReq};
use crate::state::AppState;
use crate::theme;

const GREEN: Color = Color::from_rgb8(0x9e, 0xce, 0x6a);
const RED: Color = Color::from_rgb8(0xf7, 0x76, 0x8e);
const AMBER: Color = Color::from_rgb8(0xe5, 0xc0, 0x7b);

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

/// The largest number of identical (normalised) queries — the N+1 signal.
fn max_duplicates(queries: &[(String, String)]) -> usize {
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut max = 0;
    for (sql, _) in queries {
        let n = counts.entry(normalize(sql)).or_insert(0);
        *n += 1;
        max = max.max(*n);
    }
    max
}

fn chip(text: String, color: Color) -> impl IntoView {
    label(move || text.clone()).style(move |s| s.font_size(10.5).color(color).margin_left(8.0))
}

fn request_row(state: AppState, r: RuntimeReq) -> impl IntoView {
    let dup = max_duplicates(&r.queries);
    let id = r.id.clone();
    let id_for_expand = r.id.clone();
    let id_for_explain = r.id.clone();
    let id_for_verify = r.id.clone();

    let method = r.method.clone();
    let uri = r.uri.clone();
    let status = r.status;
    let dur = r.duration_ms;
    let qn = r.queries.len();
    let cache_h = r.cache_hits;
    let cache_m = r.cache_misses;
    let mails = r.mails.len();
    let events = r.events;

    let head = stack((
        label(move || method.clone()).style(|s| {
            s.font_size(11.0)
                .font_family("monospace".to_string())
                .color(theme::fg_dim())
                .min_width(46.0)
        }),
        label(move || uri.clone()).style(|s| {
            s.flex_grow(1.0_f32)
                .font_size(12.0)
                .font_family("monospace".to_string())
                .color(theme::fg())
                .text_ellipsis()
        }),
        chip(
            format!("{status}"),
            if (200..300).contains(&status) {
                GREEN
            } else {
                RED
            },
        ),
        chip(
            format!("{dur:.0}ms"),
            if dur > 200.0 { AMBER } else { theme::fg_dim() },
        ),
        chip(
            if dup > 1 {
                format!("Q:{qn} ⚠×{dup}")
            } else {
                format!("Q:{qn}")
            },
            if dup > 1 { RED } else { theme::fg_dim() },
        ),
        chip(format!("C:{cache_h}/{cache_m}"), theme::fg_dim()),
        chip(
            if mails > 0 {
                format!("✉{mails}")
            } else {
                String::new()
            },
            AMBER,
        ),
        chip(
            if events > 0 {
                format!("⚡{events}")
            } else {
                String::new()
            },
            theme::fg_dim(),
        ),
        label(|| "✨".to_string())
            .style(|s| {
                s.margin_left(8.0)
                    .font_size(12.0)
                    .cursor(floem::style::CursorStyle::Pointer)
                    .hover(|s| s.color(theme::accent()))
            })
            .on_click_stop(move |_| state.runtime_explain(&id_for_explain)),
        label(|| "✓".to_string())
            .style(|s| {
                s.margin_left(6.0)
                    .font_size(12.0)
                    .color(theme::fg_dim())
                    .cursor(floem::style::CursorStyle::Pointer)
                    .hover(|s| s.color(theme::accent()))
            })
            .on_click_stop(move |_| state.verify_begin(&id_for_verify)),
    ))
    .style(|s| {
        s.flex_row()
            .items_center()
            .gap(2.0)
            .width_full()
            .padding_horiz(12.0)
            .padding_vert(6.0)
            .cursor(floem::style::CursorStyle::Pointer)
            .hover(|s| s.background(theme::bg_hover()))
    })
    .on_click_stop(move |_| {
        let opening = state
            .runtime_expanded
            .with_untracked(|e| e.as_deref() != Some(id_for_expand.as_str()));
        state.runtime_expanded.update(|e| {
            *e = if opening {
                Some(id_for_expand.clone())
            } else {
                None
            }
        });
        if opening {
            state.runtime_load_chain(&id_for_expand);
        }
    });

    // Expanded detail: queries (with EXPLAIN), mail sent, matching error log.
    let row = r.clone();
    let detail = dyn_stack(
        move || {
            if state
                .runtime_expanded
                .with(|e| e.as_deref() == Some(id.as_str()))
            {
                row.detail_lines(state.grove_sql_capture.get())
                    .into_iter()
                    .enumerate()
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            }
        },
        |(i, _)| *i,
        move |(_, line)| detail_row(state, line),
    )
    .style(|s| s.flex_col().width_full().padding_bottom(4.0));

    stack((head, detail)).style(|s| {
        s.flex_col()
            .width_full()
            .border_bottom(1.0)
            .border_color(theme::border())
    })
}

/// One line of a request's expanded detail.
fn detail_row(state: AppState, line: DetailLine) -> impl IntoView {
    let (text, color, explain_sql) = match line {
        DetailLine::Query { sql, duration } => {
            let text = if duration.is_empty() {
                sql.trim().to_string()
            } else {
                format!("{}  ({duration}ms)", sql.trim())
            };
            (text, theme::fg_dim(), Some(sql))
        }
        DetailLine::Mail(m) => (format!("✉ {m}"), AMBER, None),
        DetailLine::Log(l) => (format!("⚠ {l}"), RED, None),
        DetailLine::Note(n) => (n, theme::fg_dim(), None),
    };
    let has_sql = explain_sql.is_some();
    let sql = explain_sql.unwrap_or_default();
    let explain = label(|| "EXPLAIN".to_string())
        .style(move |s| {
            let s = s
                .font_size(10.0)
                .flex_shrink(0.0_f32)
                .padding_horiz(6.0)
                .border_radius(3.0)
                .color(theme::accent())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::bg_hover()));
            if has_sql {
                s
            } else {
                s.hide()
            }
        })
        .on_click_stop(move |_| state.db_explain_from_runtime(sql.clone()));
    stack((
        label(move || text.clone()).style(move |s| {
            s.font_size(11.0)
                .font_family("monospace".to_string())
                .color(color)
                .flex_grow(1.0_f32)
                .min_width(0.0)
                .text_ellipsis()
        }),
        explain,
    ))
    .style(|s| {
        s.flex_row()
            .items_center()
            .gap(6.0)
            .padding_left(24.0)
            .padding_right(12.0)
            .padding_vert(1.0)
            .width_full()
    })
}

pub fn runtime_panel(state: AppState) -> impl IntoView {
    let title =
        label(|| "Runtime".to_string()).style(|s| s.font_size(13.0).font_bold().color(theme::fg()));
    let count = label(move || {
        let n = state.runtime_reqs.with(|r| r.len());
        format!("{n} captured")
    })
    .style(|s| {
        s.flex_grow(1.0_f32)
            .margin_left(10.0)
            .font_size(11.0)
            .color(theme::fg_dim())
    });
    // Grove correlates SQL with requests only while capture is on; the toggle
    // lives here because an empty query list is the moment you want it.
    let sql = label(move || {
        if state.grove_site.get().flatten().is_none() {
            return String::new();
        }
        match state.grove_sql_capture.get() {
            Some(true) => "SQL capture: on".to_string(),
            Some(false) => "SQL capture: off".to_string(),
            None => "SQL capture: ?".to_string(),
        }
    })
    .style(move |s| {
        let s = s
            .padding_horiz(10.0)
            .padding_vert(3.0)
            .border_radius(4.0)
            .font_size(11.0)
            .cursor(floem::style::CursorStyle::Pointer)
            .hover(|s| s.background(theme::bg_hover()).color(theme::fg()));
        match state.grove_sql_capture.get() {
            Some(true) => s.color(GREEN),
            _ => s.color(theme::fg_dim()),
        }
    })
    .on_click_stop(move |_| state.toggle_grove_sql_capture());
    let clear = label(|| "Clear".to_string())
        .style(|s| {
            s.padding_horiz(10.0)
                .padding_vert(3.0)
                .border_radius(4.0)
                .font_size(11.0)
                .color(theme::fg_dim())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::bg_hover()).color(theme::fg()))
        })
        .on_click_stop(move |_| state.clear_runtime());
    let close = label(|| "✕".to_string())
        .style(|s| {
            s.padding_horiz(8.0)
                .color(theme::fg_dim())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.color(theme::fg()))
        })
        .on_click_stop(move |_| state.runtime_open.set(false));
    let header = stack((title, count, sql, clear, close)).style(|s| {
        s.flex_row()
            .items_center()
            .gap(8.0)
            .padding_horiz(12.0)
            .padding_vert(8.0)
            .width_full()
            .border_bottom(1.0)
            .border_color(theme::border())
    });

    let hint = label(move || {
        if state.grove_site.get().flatten().is_some() {
            "Browse your app — Grove captures every request live, nothing to install.".to_string()
        } else {
            "Browse your app — requests are captured live via Clockwork.".to_string()
        }
    })
    .style(move |s| {
        let s = s.padding(14.0).color(theme::fg_dim()).font_size(12.0);
        if state.runtime_reqs.with(|r| r.is_empty()) {
            s
        } else {
            s.hide()
        }
    });

    let rows = dyn_stack(
        move || state.runtime_reqs.get(),
        |r| format!("{}:{}", r.id, r.chain_loaded),
        move |r| request_row(state, r),
    )
    .style(|s| s.flex_col().width_full());

    let card = stack((
        header,
        hint,
        scroll(rows).style(|s| s.flex_grow(1.0_f32).width_full()),
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
            .background(Color::from_rgba8(0, 0, 0, 0xCC));
        if state.runtime_open.get() {
            s
        } else {
            s.hide()
        }
    })
}
