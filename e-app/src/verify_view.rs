//! The "Verify fix" panel: shows the baseline measurement, a re-measure button,
//! and — once measured — a before/after verdict with Keep / Discard.

use floem::peniko::Color;
use floem::reactive::{SignalGet, SignalWith};
use floem::views::{dyn_container, empty, label, stack, Decorators};
use floem::IntoView;

use crate::state::AppState;
use crate::theme;
use crate::verify::{summary, verdict_label, VerifyPhase};
use e_verify::{Comparison, RequestMetrics, Verdict};

const GREEN: Color = Color::from_rgb8(0x9e, 0xce, 0x6a);
const RED: Color = Color::from_rgb8(0xf7, 0x76, 0x8e);
const AMBER: Color = Color::from_rgb8(0xe5, 0xc0, 0x7b);

fn verdict_color(v: Verdict) -> Color {
    match v {
        Verdict::Improved => GREEN,
        Verdict::NoChange => theme::fg_dim(),
        Verdict::Regressed => AMBER,
        Verdict::Broke => RED,
    }
}

fn btn(text: &'static str, primary: bool) -> impl IntoView {
    label(move || text.to_string()).style(move |s| {
        let s = s
            .padding_horiz(12.0)
            .padding_vert(5.0)
            .border_radius(6.0)
            .font_size(12.0)
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

/// A `Label · value` metric line.
fn metric(name: String, value: String, color: Color) -> impl IntoView {
    stack((
        label(move || name.clone())
            .style(|s| s.color(theme::fg_dim()).font_size(11.0).width(110.0)),
        label(move || value.clone()).style(move |s| s.color(color).font_size(12.0)),
    ))
    .style(|s| s.flex_row().items_center().gap(6.0))
}

fn metrics_block(title: &'static str, m: &RequestMetrics) -> impl IntoView {
    let n1 = if m.has_n_plus_one() {
        "yes".to_string()
    } else {
        "no".to_string()
    };
    let n1_color = if m.has_n_plus_one() { AMBER } else { GREEN };
    stack((
        label(move || title.to_string()).style(|s| {
            s.color(theme::fg())
                .font_size(12.0)
                .font_bold()
                .margin_bottom(4.0)
        }),
        metric("Time".into(), format!("{:.0} ms", m.ms), theme::fg()),
        metric("Queries".into(), m.query_count.to_string(), theme::fg()),
        metric("N+1".into(), n1, n1_color),
    ))
    .style(|s| s.flex_col().gap(2.0).flex_grow(1.0))
}

fn verdict_view(state: AppState, c: &Comparison) -> impl IntoView {
    let color = verdict_color(c.verdict);
    let vlabel = verdict_label(c.verdict).to_string();
    let sum = summary(c);
    let badge = label(move || vlabel.clone()).style(move |s| {
        s.padding_horiz(10.0)
            .padding_vert(3.0)
            .border_radius(6.0)
            .font_size(12.0)
            .font_bold()
            .color(Color::WHITE)
            .background(color)
    });
    let summary_label = label(move || sum.clone())
        .style(|s| s.color(theme::fg_dim()).font_size(12.0).margin_top(2.0));

    let buttons = stack((
        btn("Measure again", false).on_click_stop(move |_| state.verify_measure()),
        btn("Keep", true).on_click_stop(move |_| state.verify_keep()),
        btn("Discard", false).on_click_stop(move |_| state.verify_discard()),
    ))
    .style(|s| s.flex_row().gap(8.0).margin_top(14.0));

    stack((
        stack((badge, summary_label)).style(|s| s.flex_col().gap(2.0).margin_top(12.0)),
        buttons,
    ))
    .style(|s| s.flex_col())
}

fn body(state: AppState, busy: bool, phase: Option<VerifyPhase>) -> floem::AnyView {
    let session = state.verify_session.get_untracked();
    let Some(session) = session else {
        // No session yet — we're taking the baseline measurement.
        return label(|| "Measuring baseline…".to_string())
            .style(|s| s.color(theme::fg_dim()).font_size(13.0).padding(16.0))
            .into_any();
    };

    let target = format!("{} {}", session.method, session.uri);
    let header = label(move || target.clone()).style(|s| {
        s.color(theme::fg())
            .font_size(13.0)
            .font_bold()
            .margin_bottom(10.0)
    });

    let measuring = label(move || {
        if busy {
            "Measuring…".to_string()
        } else {
            String::new()
        }
    })
    .style(move |s| {
        let s = s.color(theme::accent()).font_size(12.0).margin_top(8.0);
        if busy {
            s
        } else {
            s.hide()
        }
    });

    match phase {
        Some(VerifyPhase::Done) => {
            let cols = stack((
                metrics_block("Before", &session.before),
                metrics_block("After", session.after.as_ref().unwrap_or(&session.before)),
            ))
            .style(|s| s.flex_row().gap(20.0));
            let verdict = session
                .comparison
                .as_ref()
                .map(|c| verdict_view(state, c).into_any())
                .unwrap_or_else(|| empty().into_any());
            stack((header, cols, verdict, measuring))
                .style(|s| s.flex_col().padding(16.0))
                .into_any()
        }
        _ => {
            // AwaitingFix: show the baseline and prompt to apply a fix.
            let base = metrics_block("Baseline", &session.before);
            let hint = label(|| {
                "Apply your fix — edit the code or ask the agent (⌘L) — then re-measure."
                    .to_string()
            })
            .style(|s| s.color(theme::fg_dim()).font_size(12.0).margin_top(12.0));
            let buttons = stack((
                btn("Measure again", true).on_click_stop(move |_| state.verify_measure()),
                btn("Cancel & revert", false).on_click_stop(move |_| state.verify_discard()),
            ))
            .style(|s| s.flex_row().gap(8.0).margin_top(14.0));
            stack((header, base, hint, buttons, measuring))
                .style(|s| s.flex_col().padding(16.0))
                .into_any()
        }
    }
}

pub fn verify_panel(state: AppState) -> impl IntoView {
    let title = label(|| "Verify fix".to_string()).style(|s| {
        s.font_size(13.0)
            .font_bold()
            .color(theme::fg())
            .flex_grow(1.0)
    });
    let close = label(|| "✕".to_string())
        .style(|s| {
            s.padding_horiz(8.0)
                .color(theme::fg_dim())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.color(theme::fg()))
        })
        .on_click_stop(move |_| state.verify_keep());
    let header = stack((title, close)).style(|s| {
        s.flex_row()
            .items_center()
            .gap(8.0)
            .padding_horiz(14.0)
            .padding_vert(8.0)
            .width_full()
            .border_bottom(1.0)
            .border_color(theme::border())
    });

    let content = dyn_container(
        move || {
            let busy = state.verify_busy.get();
            let phase = state.verify_session.with(|s| s.as_ref().map(|s| s.phase));
            (busy, phase)
        },
        move |(busy, phase)| body(state, busy, phase),
    );

    let card = stack((header, content)).style(|s| {
        s.flex_col()
            .width(520.0)
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
        if state.verify_open.get() {
            s
        } else {
            s.hide()
        }
    })
}
