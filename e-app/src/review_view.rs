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
/// A style that hides what only makes sense for local changes — open,
/// revert, ask the agent, ship — while a pull request is under review.
fn local_only(state: AppState) -> impl Fn(floem::style::Style) -> floem::style::Style {
    move |s| {
        if state.review.pr.with(Option::is_some) {
            s.hide()
        } else {
            s
        }
    }
}

fn green() -> Color {
    theme::success()
}
fn red() -> Color {
    theme::error()
}
fn amber() -> Color {
    theme::warning()
}
const ADD_BG: Color = Color::from_rgba8(0x6a, 0xb0, 0x4a, 0x22);
const DEL_BG: Color = Color::from_rgba8(0xe0, 0x6c, 0x75, 0x22);

fn risk_color(r: Risk) -> Color {
    match r {
        Risk::High => red(),
        Risk::Medium => amber(),
        Risk::Low => theme::fg_dim(),
    }
}

fn severity_color(s: Severity) -> Color {
    match s {
        Severity::Danger => red(),
        Severity::Warn => amber(),
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
    .style(move |s| s.width(14.0).font_size(11.0).color(green()));
    let kind = label(move || kind_glyph(r.kind).to_string())
        .style(move |s| s.width(12.0).font_size(10.0).color(theme::fg_dim()));
    let name = label(move || path_text.clone()).style(move |s| {
        s.flex_grow(1.0_f32).font_size(12.0).color(if r.reviewed {
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
                .review
                .selected
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
        .review
        .changeset
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
            s.flex_grow(1.0_f32)
                .font_size(12.0)
                .font_bold()
                .color(theme::fg())
        })
    };
    let actions = {
        let (p1, p2, p3) = (path.clone(), path.clone(), path.clone());
        stack((
            btn("Open", false)
                .style(local_only(state))
                .on_click_stop(move |_| state.review_open_file(&p1)),
            btn("Ask why", false)
                .style(local_only(state))
                .on_click_stop(move |_| state.review_ask_agent(&p2)),
            btn("Revert", false)
                .style(local_only(state))
                .on_click_stop(move |_| state.review_revert_file(&p3)),
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
                    .style(move |s| s.flex_grow(1.0_f32).font_size(11.0).color(color)),
                btn("Ask", false)
                    .style(local_only(state))
                    .on_click_stop(move |_| state.review_ask_flag(&ask)),
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
                    Some(b'+') => (green(), ADD_BG),
                    Some(b'-') => (red(), DEL_BG),
                    _ => (theme::fg_dim(), Color::TRANSPARENT),
                }
            };
            label(move || text.clone()).style(move |s| {
                s.width_full()
                    .font_family(theme::mono_family())
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
        scroll(body).style(|s| s.flex_grow(1.0_f32).width_full()),
    ))
    .style(|s| s.flex_col().size_full())
    .into_any()
}

pub fn review_panel(state: AppState) -> impl IntoView {
    let title = label(move || {
        if state.review.picking.get() {
            "Pull requests".to_string()
        } else if let Some(pr) = state.review.pr.get() {
            format!("#{}  {}", pr.number, pr.title)
        } else if state.review.turn.with(Option::is_some) {
            "Review: one turn".to_string()
        } else {
            "Session review".to_string()
        }
    })
    .style(|s| s.font_size(13.0).font_bold().color(theme::fg()));
    // Back from one turn to everything the session changed.
    let whole = btn("Whole session", false)
        .style(move |s| {
            if state.review.turn.with(Option::is_some) {
                s
            } else {
                s.hide()
            }
        })
        .on_click_stop(move |_| {
            state.review.turn.set(None);
            state.refresh_review();
        });
    let summary = label(move || state.review.changeset.with(|cs| cs.summary()))
        .style(|s| s.font_size(11.0).color(theme::fg_dim()).margin_left(10.0));
    let progress = label(move || {
        let (done, total) = state.review.changeset.with(|cs| cs.progress());
        if total == 0 {
            String::new()
        } else {
            format!("{done}/{total} reviewed")
        }
    })
    .style(move |s| {
        let all = state.review.changeset.with(|cs| cs.all_reviewed());
        s.flex_grow(1.0_f32)
            .margin_left(10.0)
            .font_size(11.0)
            .color(if all { green() } else { theme::fg_dim() })
    });
    let ask = btn("Summarize", false)
        .style(move |s| {
            if state.review.pr.with(Option::is_some) || state.review.picking.get() {
                s.hide()
            } else {
                s
            }
        })
        .on_click_stop(move |_| state.review_ask_summary());
    let refresh = btn("Refresh", false).on_click_stop(move |_| {
        if state.review.picking.get_untracked() {
            state.review_load_pulls();
        } else {
            state.refresh_review();
        }
    });
    // To the open pull requests, and back to the session.
    let pulls = btn("Pull requests", false)
        .style(move |s| {
            if state.review.picking.get() {
                s.hide()
            } else {
                s
            }
        })
        .on_click_stop(move |_| state.review_pick_pull());
    let session = btn("Session", false)
        .style(move |s| {
            if state.review.pr.with(Option::is_some) || state.review.picking.get() {
                s
            } else {
                s.hide()
            }
        })
        .on_click_stop(move |_| state.review_back_to_session());
    let close = label(|| "✕".to_string())
        .style(|s| {
            s.padding_horiz(8.0)
                .color(theme::fg_dim())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.color(theme::fg()))
        })
        .on_click_stop(move |_| state.review.open.set(false));
    let header = stack((
        title, summary, progress, whole, session, pulls, ask, refresh, close,
    ))
    .style(|s| {
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
            state.review.changeset.with(|cs| {
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
                                flag: state.review.flags.with(|all| {
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
        let pr = state.review.pr.get();
        match (state.review.busy.get(), pr) {
            (true, Some(pr)) => format!("Reading #{}…", pr.number),
            (true, None) => "Reading changes…".to_string(),
            (false, Some(pr)) => format!("No changes in #{}.", pr.number),
            (false, None) => "No changes since the session started.".to_string(),
        }
    })
    .style(move |s| {
        let s = s.padding(14.0).color(theme::fg_dim()).font_size(12.0);
        if state.review.changeset.with(|cs| cs.is_empty()) {
            s
        } else {
            s.hide()
        }
    });

    let files = stack((
        empty_hint,
        scroll(list).style(|s| s.flex_grow(1.0_f32).width_full()),
    ))
    .style(move |s| {
        let s = s.flex_col().size_full();
        if state.review.picking.get() {
            s.hide()
        } else {
            s
        }
    });
    let left = stack((files, pull_list(state))).style(|s| {
        s.flex_col()
            .width(340.0)
            .height_full()
            .border_right(1.0)
            .border_color(theme::border())
    });

    // Rebuild the diff pane when the selection or the changeset materially changes.
    let right = dyn_container(
        move || {
            let sel = state.review.selected.get();
            let fingerprint = state.review.changeset.with(|cs| {
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
    .style(move |s| {
        if state.review.picking.get() {
            s.hide()
        } else {
            s
        }
    });
    let pick_hint = label(|| {
        "Pick a pull request to review it here: its diff from GitHub, risk-ranked and \
         flagged like a session, without checking it out."
            .to_string()
    })
    .style(move |s| {
        let s = s
            .padding(24.0)
            .max_width(460.0)
            .font_size(12.0)
            .color(theme::fg_dim());
        if state.review.picking.get() {
            s
        } else {
            s.hide()
        }
    });
    let right = stack((right, pick_hint)).style(|s| s.flex_col().flex_grow(1.0_f32).height_full());

    // ---- Ship bar: readiness checklist + run tests + commit/PR ----------
    let verdict_badge = label(move || match state.review_ship_verdict().readiness {
        Readiness::Ready => "Ready".to_string(),
        Readiness::Warn => "Notes".to_string(),
        Readiness::Blocked => "Needs attention".to_string(),
    })
    .style(move |s| {
        let color = match state.review_ship_verdict().readiness {
            Readiness::Ready => green(),
            Readiness::Warn => amber(),
            Readiness::Blocked => red(),
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
            s.flex_grow(1.0_f32)
                .margin_left(8.0)
                .font_size(11.0)
                .color(theme::fg_dim())
        });
    let run_tests = btn("Run tests", false).on_click_stop(move |_| state.run_tests());
    // Sits next to the tests because it answers the neighbouring question: the
    // tests say the change is correct, this says what it did.
    let measure = label(move || {
        if state.review.evidence_busy.get() {
            "Measuring…".to_string()
        } else {
            match state.review.evidence.with(|e| e.as_ref().map(|r| r.len())) {
                Some(0) => "No routes".to_string(),
                Some(n) => format!("Measured {n}"),
                None => "Measure routes".to_string(),
            }
        }
    })
    .style(move |s| {
        let s = s
            .padding_horiz(10.0)
            .height(26.0)
            .items_center()
            .font_size(12.0)
            .border(1.0)
            .border_color(theme::border())
            .border_radius(4.0)
            .color(theme::fg())
            .cursor(floem::style::CursorStyle::Pointer)
            .hover(|s| s.background(theme::bg_hover()));
        // Nothing to measure unless routes were attributed to the changeset.
        if state.review_attribution().affected.is_empty() {
            s.color(theme::fg_dim())
                .cursor(floem::style::CursorStyle::Default)
        } else {
            s
        }
    })
    .on_click_stop(move |_| state.review_measure_evidence());
    let ship = label(move || {
        if state.review.shipping.get() {
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
            .background(if blocked { amber() } else { theme::accent() })
    })
    .on_click_stop(move |_| state.review_commit_and_pr(true));
    let ship_bar =
        stack((verdict_badge, verdict_reasons, measure, run_tests, ship)).style(move |s| {
            let s = s
                .flex_row()
                .items_center()
                .gap(8.0)
                .width_full()
                .padding_horiz(12.0)
                .padding_vert(8.0)
                .border_top(1.0)
                .border_color(theme::border());
            if state.review.changeset.with(|cs| cs.is_empty())
                || state.review.pr.with(Option::is_some)
                || state.review.picking.get()
            {
                s.hide()
            } else {
                s
            }
        });

    let card = stack((
        header,
        stack((left, right)).style(|s| s.flex_row().flex_grow(1.0_f32).width_full()),
        ship_bar,
        pull_bar(state),
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
        if state.review.open.get() {
            s
        } else {
            s.hide()
        }
    })
}

/// The open pull requests, a row each; a click reviews one.
fn pull_list(state: AppState) -> impl IntoView {
    let status = label(move || state.review.pulls_status.get()).style(move |s| {
        let s = s.padding(14.0).font_size(12.0).color(theme::fg_dim());
        if state.review.pulls_status.with(String::is_empty) {
            s.hide()
        } else {
            s
        }
    });
    let rows = dyn_stack(
        move || state.review.pulls.get(),
        |pr| (pr.number, pr.head_oid.clone(), pr.checks.clone()),
        move |pr| {
            let (title, detail) = crate::review_pulls::pull_line(&pr);
            let checks = pr.checks.clone();
            stack((
                label(move || title.clone()).style(|s| {
                    s.font_size(12.0)
                        .color(theme::fg())
                        .text_ellipsis()
                        .min_width(0.0)
                }),
                label(move || detail.clone()).style(move |s| {
                    let color = match checks.as_str() {
                        "fail" => red(),
                        _ => theme::fg_dim(),
                    };
                    s.font_size(11.0)
                        .color(color)
                        .text_ellipsis()
                        .min_width(0.0)
                }),
            ))
            .style(|s| {
                s.flex_col()
                    .gap(2.0)
                    .width_full()
                    .padding_horiz(12.0)
                    .padding_vert(6.0)
                    .border_bottom(1.0)
                    .border_color(theme::border())
                    .cursor(floem::style::CursorStyle::Pointer)
                    .hover(|s| s.background(theme::bg_hover()))
            })
            .on_click_stop(move |_| state.review_open_pull(pr.clone()))
        },
    )
    .style(|s| s.flex_col().width_full());
    stack((
        status,
        scroll(rows).style(|s| s.flex_grow(1.0_f32).width_full()),
    ))
    .style(move |s| {
        let s = s.flex_col().size_full();
        if state.review.picking.get() {
            s
        } else {
            s.hide()
        }
    })
}

/// Under a pull request: who, which branches, how its checks stand, what
/// couldn't be checked — and to GitHub, or to check it out.
fn pull_bar(state: AppState) -> impl IntoView {
    let about = label(move || {
        state
            .review
            .pr
            .get()
            .map(|pr| crate::review_pulls::pull_line(&pr).1)
            .unwrap_or_default()
    })
    .style(|s| s.font_size(11.0).color(theme::fg_dim()));
    let note = label(move || state.review.pr_note.get().unwrap_or_default()).style(move |s| {
        let s = s
            .flex_grow(1.0_f32)
            .min_width(0.0)
            .font_size(11.0)
            .color(amber());
        if state.review.pr_note.with(Option::is_none) {
            s.hide()
        } else {
            s
        }
    });
    let spacer = floem::views::empty().style(|s| s.flex_grow(1.0_f32));
    let github = btn("Open on GitHub", false).on_click_stop(move |_| state.review_pull_on_github());
    let checkout = btn("Check out", false).on_click_stop(move |_| state.review_checkout_pull());
    stack((about, note, spacer, github, checkout)).style(move |s| {
        let s = s
            .flex_row()
            .items_center()
            .gap(8.0)
            .width_full()
            .padding_horiz(12.0)
            .padding_vert(8.0)
            .border_top(1.0)
            .border_color(theme::border());
        if state.review.pr.with(Option::is_some) && !state.review.picking.get() {
            s
        } else {
            s.hide()
        }
    })
}
