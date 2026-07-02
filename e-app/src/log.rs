//! Live `laravel.log` panel: tails the log, colours levels, and makes stack
//! frames clickable to jump to the file:line.

use floem::peniko::Color;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use floem::views::{dyn_stack, label, scroll, stack, Decorators};
use floem::IntoView;

use crate::state::AppState;
use crate::theme;

const RED: Color = Color::from_rgb8(0xf7, 0x76, 0x8e);
const YELLOW: Color = Color::from_rgb8(0xe5, 0xc0, 0x7b);

/// Parse a PHP stack-trace frame `#N /abs/File.php(123): …` → `(path, line)`.
fn parse_frame(line: &str) -> Option<(String, usize)> {
    let t = line.trim_start();
    if !t.starts_with('#') {
        return None;
    }
    let popen = line.find(".php(")? + 4; // index of '('
    let pathstart = line[..popen].rfind(' ').map(|i| i + 1).unwrap_or(0);
    let path = line[pathstart..popen].to_string();
    let after = &line[popen + 1..];
    let close = after.find(')')?;
    let ln: usize = after[..close].parse().ok()?;
    Some((path, ln))
}

fn line_color(line: &str) -> Color {
    if line.contains(".ERROR") || line.contains(".CRITICAL") || line.contains(".EMERGENCY") {
        RED
    } else if line.contains(".WARNING") {
        YELLOW
    } else if line.trim_start().starts_with('#') {
        theme::fg_dim()
    } else {
        theme::fg()
    }
}

pub fn laravel_log_panel(state: AppState) -> impl IntoView {
    let title = label(|| "Laravel Log".to_string()).style(|s| {
        s.flex_grow(1.0)
            .font_size(13.0)
            .font_bold()
            .color(theme::fg())
    });
    let refresh = pill("↻ Refresh", move || state.refresh_laravel_log());
    let fix = pill("✨ Fix with AI", move || state.log_fix_with_agent());
    let close = label(|| "✕".to_string())
        .style(|s| {
            s.padding_horiz(8.0)
                .color(theme::fg_dim())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.color(theme::fg()))
        })
        .on_click_stop(move |_| state.log_open.set(false));
    let header = stack((title, refresh, fix, close)).style(|s| {
        s.flex_row()
            .items_center()
            .gap(8.0)
            .padding_horiz(12.0)
            .padding_vert(8.0)
            .width_full()
            .border_bottom(1.0)
            .border_color(theme::border())
    });

    let lines = dyn_stack(
        move || {
            state
                .log_lines
                .get()
                .into_iter()
                .enumerate()
                .collect::<Vec<_>>()
        },
        |(i, _)| *i,
        move |(_, line)| {
            let frame = parse_frame(&line);
            let color = line_color(&line);
            let clickable = frame.is_some();
            label(move || line.clone())
                .style(move |s| {
                    let s = s
                        .font_family("monospace".to_string())
                        .font_size(11.5)
                        .padding_horiz(12.0)
                        .padding_vert(1.0)
                        .width_full()
                        .color(color);
                    if clickable {
                        s.cursor(floem::style::CursorStyle::Pointer)
                            .hover(|s| s.background(theme::bg_hover()))
                    } else {
                        s
                    }
                })
                .on_click_stop(move |_| {
                    if let Some((path, ln)) = &frame {
                        state.jump_to(&format!("file://{path}"), ln.saturating_sub(1), 0);
                    }
                })
        },
    )
    .style(|s| s.flex_col().width_full());

    let empty_hint =
        label(|| "No log entries (storage/logs/laravel.log).".to_string()).style(move |s| {
            let s = s.padding(16.0).color(theme::fg_dim()).font_size(12.0);
            if state.log_lines.with(|l| l.is_empty()) {
                s
            } else {
                s.hide()
            }
        });

    let card = stack((
        header,
        empty_hint,
        scroll(lines).style(|s| s.flex_grow(1.0).width_full()),
    ))
    .style(|s| {
        s.flex_col()
            .width(900.0)
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
        if state.log_open.get() {
            s
        } else {
            s.hide()
        }
    })
}

fn pill(text: &'static str, on: impl Fn() + 'static) -> impl IntoView {
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
