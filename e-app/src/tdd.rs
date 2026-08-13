//! Autonomous TDD panel: run the test suite, see pass/fail, and let the agent
//! iterate fixes until green.

use floem::peniko::Color;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use floem::views::{container, dyn_stack, empty, label, scroll, stack, Decorators};
use floem::IntoView;

use crate::state::{AppState, TddStatus};
use crate::theme;

fn pill(text: &'static str, primary: bool, on: impl Fn() + 'static) -> impl IntoView {
    label(move || text.to_string())
        .style(move |s| {
            let s = s
                .padding_horiz(14.0)
                .height(28.0)
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

pub fn tdd_panel(state: AppState) -> impl IntoView {
    let title =
        label(|| "Tests".to_string()).style(|s| s.font_size(13.0).font_bold().color(theme::fg()));

    let status = label(move || match state.tdd_status.get() {
        TddStatus::Idle => "—".to_string(),
        TddStatus::Running => "● running…".to_string(),
        TddStatus::Passed => "✓ passing".to_string(),
        TddStatus::Failed => "✗ failing".to_string(),
    })
    .style(move |s| {
        let s = s
            .font_size(12.0)
            .padding_horiz(8.0)
            .padding_vert(2.0)
            .border_radius(4.0);
        match state.tdd_status.get() {
            TddStatus::Passed => s.color(Color::from_rgb8(0x9e, 0xce, 0x6a)),
            TddStatus::Failed => s.color(Color::from_rgb8(0xf7, 0x76, 0x8e)),
            TddStatus::Running => s.color(theme::accent()),
            TddStatus::Idle => s.color(theme::fg_dim()),
        }
    });

    let iter = label(move || {
        let i = state.tdd_iteration.get();
        if i > 0 {
            format!("iteration {i}")
        } else {
            String::new()
        }
    })
    .style(|s| s.font_size(11.0).color(theme::fg_dim()));

    let close = label(|| "✕".to_string())
        .style(|s| {
            s.padding_horiz(8.0)
                .color(theme::fg_dim())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.color(theme::fg()))
        })
        .on_click_stop(move |_| state.tdd_open.set(false));

    let header = stack((
        title,
        status,
        iter,
        empty().style(|s| s.flex_grow(1.0_f32)),
        close,
    ))
    .style(|s| {
        s.flex_row()
            .items_center()
            .gap(10.0)
            .padding_horiz(12.0)
            .padding_vert(8.0)
            .width_full()
            .border_bottom(1.0)
            .border_color(theme::border())
    });

    let run = pill("▶ Run", false, move || state.run_tests());
    // "Fix to green" toggles into / out of the autonomous loop.
    let fix = label(move || {
        if state.tdd_loop.get() {
            "■ Stop".to_string()
        } else {
            "✨ Fix to green".to_string()
        }
    })
    .style(move |s| {
        let base = s
            .padding_horiz(14.0)
            .height(28.0)
            .items_center()
            .border_radius(5.0)
            .font_size(12.0)
            .cursor(floem::style::CursorStyle::Pointer);
        if state.tdd_loop.get() {
            base.border(1.0)
                .border_color(Color::from_rgb8(0xf7, 0x76, 0x8e))
                .color(Color::from_rgb8(0xf7, 0x76, 0x8e))
        } else {
            base.background(theme::accent())
                .color(Color::from_rgb8(0x14, 0x16, 0x1b))
        }
    })
    .on_click_stop(move |_| {
        if state.tdd_loop.get_untracked() {
            state.tdd_stop();
        } else {
            state.tdd_fix_to_green();
        }
    });
    // `12 passed · 2 failed`, from the parsed report rather than the exit code —
    // "the suite failed" and "two of ninety tests failed" are different news.
    let tally = label(move || {
        state.tdd_results.with(|r| {
            if r.is_empty() {
                String::new()
            } else {
                r.summary()
            }
        })
    })
    .style(move |s| {
        let s = s
            .flex_grow(1.0_f32)
            .items_center()
            .font_size(12.0)
            .color(theme::fg_dim());
        if state.tdd_results.with(|r| r.failed() > 0) {
            s.color(Color::from_rgb8(0xe0, 0x6c, 0x75))
        } else {
            s
        }
    });

    let toolbar = stack((run, fix, tally)).style(|s| {
        s.flex_row()
            .gap(8.0)
            .padding_horiz(12.0)
            .padding_vert(8.0)
            .width_full()
            .border_bottom(1.0)
            .border_color(theme::border())
    });

    // The failures, first and clickable. Reading a wall of runner output to find
    // out which test broke is the thing this panel existed to make you do.
    let failures = dyn_stack(
        move || {
            state
                .tdd_results
                .get()
                .problems()
                .cloned()
                .enumerate()
                .collect::<Vec<_>>()
        },
        |(i, c): &(usize, crate::testrun::TestCase)| (*i, c.full_name()),
        move |(_, case): (usize, crate::testrun::TestCase)| {
            let target = case.file.clone().map(|f| (f, case.line.unwrap_or(1)));
            let locatable = target.is_some();
            let name = case.full_name();
            let detail = case
                .message
                .as_deref()
                .unwrap_or("")
                .lines()
                .next()
                .unwrap_or("")
                .to_string();
            let errored = case.outcome == crate::testrun::Outcome::Errored;

            stack((
                label(move || if errored { "!" } else { "\u{00d7}" }.to_string()).style(move |s| {
                    s.width(14.0)
                        .font_size(12.0)
                        .color(Color::from_rgb8(0xe0, 0x6c, 0x75))
                }),
                stack((
                    label(move || name.clone()).style(|s| s.font_size(12.0).color(theme::fg())),
                    label(move || detail.clone()).style(|s| {
                        s.font_size(11.0)
                            .color(theme::fg_dim())
                            .font_family("monospace".to_string())
                    }),
                ))
                .style(|s| s.flex_col().gap(1.0).flex_grow(1.0_f32)),
            ))
            .style(move |s| {
                let s = s
                    .items_start()
                    .gap(6.0)
                    .width_full()
                    .padding_horiz(12.0)
                    .padding_vert(5.0);
                // Only the ones we can locate are worth a pointer.
                if locatable {
                    s.cursor(floem::style::CursorStyle::Pointer)
                        .hover(|s| s.background(theme::bg_hover()))
                } else {
                    s
                }
            })
            .on_click_stop(move |_| {
                if let Some((file, line)) = target.clone() {
                    // JUnit lines are 1-based; the editor counts from zero.
                    let uri = format!("file://{}", file.display());
                    state.jump_to(&uri, (line as usize).saturating_sub(1), 0);
                    state.tdd_open.set(false);
                }
            })
        },
    )
    .style(move |s| {
        let s = s.flex_col().width_full();
        if state.tdd_results.with(|r| r.failed() == 0) {
            s.hide()
        } else {
            s.border_bottom(1.0).border_color(theme::border())
        }
    });

    let output = scroll(label(move || state.tdd_output.get()).style(|s| {
        s.font_family("monospace".to_string())
            .font_size(12.0)
            .padding(10.0)
            .color(theme::fg())
    }))
    .style(|s| s.flex_grow(1.0_f32).width_full());

    let card = stack((header, toolbar, failures, output)).style(|s| {
        s.flex_col()
            .width(820.0)
            .height(560.0)
            .border(1.0)
            .border_color(theme::border())
            .border_radius(10.0)
            .background(theme::bg())
    });

    container(card).style(move |s| {
        let s = s
            .absolute()
            .inset(0.0)
            .size_full()
            .items_center()
            .justify_center()
            .background(Color::from_rgba8(0, 0, 0, 0xCC));
        if state.tdd_open.get() {
            s
        } else {
            s.hide()
        }
    })
}
