//! The **session review** panel: a risk-ranked list of everything the agent
//! changed, with the diff, sign-off, revert and "ask why" beside it.

use e_review::flags::Severity;
use e_review::ship::Readiness;
use e_review::{ChangeKind, Risk};
use floem::peniko::Color;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use floem::views::{dyn_container, dyn_stack, label, scroll, stack, Decorators};
use floem::IntoView;

use crate::state::AppState;
use crate::theme;

const GREEN: Color = Color::from_rgb8(0x98, 0xc3, 0x79);
const RED: Color = Color::from_rgb8(0xe0, 0x6c, 0x75);
const AMBER: Color = Color::from_rgb8(0xe5, 0xc0, 0x7b);
const ADD_BG: Color = Color::from_rgba8(0x6a, 0xb0, 0x4a, 0x22);
const DEL_BG: Color = Color::from_rgba8(0xe0, 0x6c, 0x75, 0x22);

fn risk_color(r: Risk) -> Color {
    match r {
        Risk::High => RED,
        Risk::Medium => AMBER,
        Risk::Low => theme::fg_dim(),
    }
}

fn severity_color(s: Severity) -> Color {
    match s {
        Severity::Danger => RED,
        Severity::Warn => AMBER,
        Severity::Info => theme::fg_dim(),
    }
}

fn kind_glyph(k: ChangeKind) -> &'static str {
    match k {
        ChangeKind::Added => "A",
        ChangeKind::Modified => "M",
        ChangeKind::Deleted => "D",
        ChangeKind::Renamed => "R",
    }
}

fn btn(text: &'static str, primary: bool) -> impl IntoView {
    label(move || text.to_string()).style(move |s| {
        let s = s
            .padding_horiz(10.0)
            .padding_vert(4.0)
            .border_radius(6.0)
            .font_size(11.0)
            .cursor(floem::style::CursorStyle::Pointer);
        if primary {
            s.background(theme::accent())
                .color(Color::WHITE)
                .hover(|s| s.background(theme::accent().multiply_alpha(0.85)))
        } else {
            s.background(theme::bg_hover())
                .color(theme::fg())
                .hover(|s| s.background(theme::border()))
        }
    })
}

/// A lightweight projection of a `FileChange` for the list (avoids cloning hunks).
#[derive(Clone, PartialEq)]
struct Row {
    path: String,
    risk: Risk,
    reason: &'static str,
    added: usize,
    removed: usize,
    reviewed: bool,
    kind: ChangeKind,
    /// Worst flag severity on this file, if any.
    flag: Option<Severity>,
}

fn file_row(state: AppState, r: Row) -> impl IntoView {
    let path_for_click = r.path.clone();
    let path_text = r.path.clone();
    let selected_path = r.path.clone();

    let tick = label(move || {
        if r.reviewed {
            "✓".to_string()
        } else {
            String::new()
        }
    })
    .style(move |s| s.width(14.0).font_size(11.0).color(GREEN));
    let kind = label(move || kind_glyph(r.kind).to_string())
        .style(move |s| s.width(12.0).font_size(10.0).color(theme::fg_dim()));
    let name = label(move || path_text.clone()).style(move |s| {
        s.flex_grow(1.0).font_size(12.0).color(if r.reviewed {
            theme::fg_dim()
        } else {
            theme::fg()
        })
    });
    let badge = label(move || r.reason.to_string()).style(move |s| {
        s.font_size(10.0)
            .padding_horiz(5.0)
            .border_radius(4.0)
            .color(risk_color(r.risk))
    });
    let flag_dot = label(move || match r.flag {
        Some(Severity::Danger) => "●".to_string(),
        Some(Severity::Warn) => "●".to_string(),
        Some(Severity::Info) => "○".to_string(),
        None => String::new(),
    })
    .style(move |s| {
        s.width(10.0)
            .font_size(10.0)
            .color(r.flag.map(severity_color).unwrap_or(theme::fg_dim()))
    });
    let counts = label(move || format!("+{} −{}", r.added, r.removed))
        .style(|s| s.font_size(10.0).color(theme::fg_dim()).margin_left(6.0));

    stack((tick, kind, name, flag_dot, badge, counts))
        .style(move |s| {
            let sel = state
                .review_selected
                .with(|p| p.as_deref() == Some(selected_path.as_str()));
            let s = s
                .flex_row()
                .items_center()
                .gap(4.0)
                .width_full()
                .padding_horiz(8.0)
                .padding_vert(4.0)
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::bg_hover()));
            if sel {
                s.background(theme::bg_active())
            } else {
                s
            }
        })
        .on_click_stop(move |_| state.review_select(path_for_click.clone()))
}

/// The diff pane for the selected file.
fn diff_pane(state: AppState, path: Option<String>) -> floem::AnyView {
    let Some(path) = path else {
        return label(|| "No file selected".to_string())
            .style(|s| s.padding(16.0).color(theme::fg_dim()).font_size(12.0))
            .into_any();
    };

    let (binary, blocks) = state
        .review_changeset
        .with_untracked(|cs| match cs.get(&path) {
            Some(f) => (
                f.binary,
                f.hunks
                    .iter()
                    .map(|h| {
                        (
                            format!("@@ -{} +{} @@", h.old_start, h.new_start),
                            h.lines.clone(),
                        )
                    })
                    .collect::<Vec<_>>(),
            ),
            None => (false, Vec::new()),
        });

    let title = {
        let p = path.clone();
        label(move || p.clone()).style(|s| {
            s.flex_grow(1.0)
                .font_size(12.0)
                .font_bold()
                .color(theme::fg())
        })
    };
    let actions = {
        let (p1, p2, p3) = (path.clone(), path.clone(), path.clone());
        stack((
            btn("Open", false).on_click_stop(move |_| state.review_open_file(&p1)),
            btn("Ask why", false).on_click_stop(move |_| state.review_ask_agent(&p2)),
            btn("Revert", false).on_click_stop(move |_| state.review_revert_file(&p3)),
            btn("Reviewed →", true).on_click_stop(move |_| state.review_mark_and_next()),
        ))
        .style(|s| s.flex_row().gap(6.0))
    };
    let header = stack((title, actions)).style(|s| {
        s.flex_row()
            .items_center()
            .gap(8.0)
            .width_full()
            .padding_horiz(12.0)
            .padding_vert(8.0)
            .border_bottom(1.0)
            .border_color(theme::border())
    });

    // Findings for this file, listed above the diff.
    let file_flags = state.review_flags_for(&path);
    let flags_empty = file_flags.is_empty();
    let flag_list = dyn_stack(
        move || {
            file_flags
                .clone()
                .into_iter()
                .enumerate()
                .collect::<Vec<_>>()
        },
        |(i, _)| *i,
        move |(_, f)| {
            let color = severity_color(f.severity);
            let text = if f.line > 0 {
                format!("{}  ({}:{})", f.message, f.code, f.line)
            } else {
                format!("{}  ({})", f.message, f.code)
            };
            let ask = f.clone();
            stack((
                label(move || text.clone())
                    .style(move |s| s.flex_grow(1.0).font_size(11.0).color(color)),
                btn("Ask", false).on_click_stop(move |_| state.review_ask_flag(&ask)),
            ))
            .style(|s| {
                s.flex_row()
                    .items_center()
                    .gap(8.0)
                    .width_full()
                    .padding_horiz(12.0)
                    .padding_vert(3.0)
            })
        },
    )
    .style(move |s| {
        let s = s.flex_col().width_full().padding_vert(4.0);
        if flags_empty {
            s.hide()
        } else {
            s.border_bottom(1.0).border_color(theme::border())
        }
    });

    if binary {
        return stack((
            header,
            label(|| "Binary file — no textual diff.".to_string())
                .style(|s| s.padding(16.0).color(theme::fg_dim()).font_size(12.0)),
        ))
        .style(|s| s.flex_col().size_full())
        .into_any();
    }

    let rows: Vec<(usize, String, bool)> = blocks
        .into_iter()
        .flat_map(|(hdr, lines)| {
            std::iter::once((hdr, true)).chain(lines.into_iter().map(|l| (l, false)))
        })
        .enumerate()
        .map(|(i, (text, is_header))| (i, text, is_header))
        .collect();

    let body = dyn_stack(
        move || rows.clone(),
        |(i, _, _)| *i,
        move |(_, text, is_header)| {
            let (color, bg) = if is_header {
                (theme::accent(), Color::TRANSPARENT)
            } else {
                match text.as_bytes().first() {
                    Some(b'+') => (GREEN, ADD_BG),
                    Some(b'-') => (RED, DEL_BG),
                    _ => (theme::fg_dim(), Color::TRANSPARENT),
                }
            };
            label(move || text.clone()).style(move |s| {
                s.width_full()
                    .font_family("monospace".to_string())
                    .font_size(12.0)
                    .padding_horiz(12.0)
                    .color(color)
                    .background(bg)
            })
        },
    )
    .style(|s| s.flex_col().width_full());

    stack((
        header,
        flag_list,
        scroll(body).style(|s| s.flex_grow(1.0).width_full()),
    ))
    .style(|s| s.flex_col().size_full())
    .into_any()
}

pub fn review_panel(state: AppState) -> impl IntoView {
    let title = label(|| "Session review".to_string())
        .style(|s| s.font_size(13.0).font_bold().color(theme::fg()));
    let summary = label(move || state.review_changeset.with(|cs| cs.summary()))
        .style(|s| s.font_size(11.0).color(theme::fg_dim()).margin_left(10.0));
    let progress = label(move || {
        let (done, total) = state.review_changeset.with(|cs| cs.progress());
        if total == 0 {
            String::new()
        } else {
            format!("{done}/{total} reviewed")
        }
    })
    .style(move |s| {
        let all = state.review_changeset.with(|cs| cs.all_reviewed());
        s.flex_grow(1.0)
            .margin_left(10.0)
            .font_size(11.0)
            .color(if all { GREEN } else { theme::fg_dim() })
    });
    let ask = btn("Summarize", false).on_click_stop(move |_| state.review_ask_summary());
    let refresh = btn("Refresh", false).on_click_stop(move |_| state.refresh_review());
    let close = label(|| "✕".to_string())
        .style(|s| {
            s.padding_horiz(8.0)
                .color(theme::fg_dim())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.color(theme::fg()))
        })
        .on_click_stop(move |_| state.review_open.set(false));
    let header = stack((title, summary, progress, ask, refresh, close)).style(|s| {
        s.flex_row()
            .items_center()
            .gap(6.0)
            .width_full()
            .padding_horiz(12.0)
            .padding_vert(8.0)
            .border_bottom(1.0)
            .border_color(theme::border())
    });

    let list = dyn_stack(
        move || {
            state.review_changeset.with(|cs| {
                cs.files
                    .iter()
                    .enumerate()
                    .map(|(i, f)| {
                        (
                            i,
                            Row {
                                path: f.path.clone(),
                                risk: f.risk,
                                reason: f.risk_reason,
                                added: f.added,
                                removed: f.removed,
                                reviewed: f.reviewed,
                                kind: f.kind,
                                flag: state.review_flags.with(|all| {
                                    all.iter()
                                        .filter(|x| x.path == f.path)
                                        .map(|x| x.severity)
                                        .max()
                                }),
                            },
                        )
                    })
                    .collect::<Vec<_>>()
            })
        },
        |(i, r)| (*i, r.path.clone(), r.reviewed, r.flag),
        move |(_, r)| file_row(state, r),
    )
    .style(|s| s.flex_col().width_full());

    let empty_hint = label(move || {
        if state.review_busy.get() {
            "Reading changes…".to_string()
        } else {
            "No changes since the session started.".to_string()
        }
    })
    .style(move |s| {
        let s = s.padding(14.0).color(theme::fg_dim()).font_size(12.0);
        if state.review_changeset.with(|cs| cs.is_empty()) {
            s
        } else {
            s.hide()
        }
    });

    let left = stack((
        empty_hint,
        scroll(list).style(|s| s.flex_grow(1.0).width_full()),
    ))
    .style(|s| {
        s.flex_col()
            .width(340.0)
            .height_full()
            .border_right(1.0)
            .border_color(theme::border())
    });

    // Rebuild the diff pane when the selection or the changeset materially changes.
    let right = dyn_container(
        move || {
            let sel = state.review_selected.get();
            let fingerprint = state.review_changeset.with(|cs| {
                (
                    cs.len(),
                    cs.total_added(),
                    cs.total_removed(),
                    cs.progress().0,
                )
            });
            (sel, fingerprint)
        },
        move |(sel, _)| diff_pane(state, sel),
    )
    .style(|s| s.flex_col().flex_grow(1.0).height_full());

    // ---- Ship bar: readiness checklist + run tests + commit/PR ----------
    let verdict_badge = label(move || match state.review_ship_verdict().readiness {
        Readiness::Ready => "Ready".to_string(),
        Readiness::Warn => "Notes".to_string(),
        Readiness::Blocked => "Needs attention".to_string(),
    })
    .style(move |s| {
        let color = match state.review_ship_verdict().readiness {
            Readiness::Ready => GREEN,
            Readiness::Warn => AMBER,
            Readiness::Blocked => RED,
        };
        s.padding_horiz(8.0)
            .padding_vert(2.0)
            .border_radius(5.0)
            .font_size(11.0)
            .font_bold()
            .color(Color::WHITE)
            .background(color)
    });
    let verdict_reasons =
        label(move || state.review_ship_verdict().reasons.join(" · ")).style(|s| {
            s.flex_grow(1.0)
                .margin_left(8.0)
                .font_size(11.0)
                .color(theme::fg_dim())
        });
    let run_tests = btn("Run tests", false).on_click_stop(move |_| state.run_tests());
    let ship = label(move || {
        if state.review_shipping.get() {
            "Shipping…".to_string()
        } else {
            "Commit & PR".to_string()
        }
    })
    .style(move |s| {
        let blocked = state.review_ship_verdict().readiness == Readiness::Blocked;
        s.padding_horiz(12.0)
            .padding_vert(5.0)
            .border_radius(6.0)
            .font_size(11.0)
            .font_bold()
            .color(Color::WHITE)
            .cursor(floem::style::CursorStyle::Pointer)
            .background(if blocked { AMBER } else { theme::accent() })
    })
    .on_click_stop(move |_| state.review_commit_and_pr(true));
    let ship_bar = stack((verdict_badge, verdict_reasons, run_tests, ship)).style(move |s| {
        let s = s
            .flex_row()
            .items_center()
            .gap(8.0)
            .width_full()
            .padding_horiz(12.0)
            .padding_vert(8.0)
            .border_top(1.0)
            .border_color(theme::border());
        if state.review_changeset.with(|cs| cs.is_empty()) {
            s.hide()
        } else {
            s
        }
    });

    let card = stack((
        header,
        stack((left, right)).style(|s| s.flex_row().flex_grow(1.0).width_full()),
        ship_bar,
    ))
    .style(|s| {
        s.flex_col()
            .width(1080.0)
            .height(700.0)
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
        if state.review_open.get() {
            s
        } else {
            s.hide()
        }
    })
}
