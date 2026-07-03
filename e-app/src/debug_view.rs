//! The Debug panel: session status, execution controls, call stack, variables
//! and breakpoints. A modal overlay in the same style as the Runtime panel.

use std::path::Path;

use floem::peniko::Color;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use floem::views::{dyn_stack, label, scroll, stack, Decorators};
use floem::IntoView;

use e_lsp::path_to_uri;

use crate::state::AppState;
use crate::theme;

const GREEN: Color = Color::from_rgb8(0x9e, 0xce, 0x6a);
const AMBER: Color = Color::from_rgb8(0xe5, 0xc0, 0x7b);
const BLUE: Color = Color::from_rgb8(0x7a, 0xa2, 0xf7);

fn status_color(status: &str) -> Color {
    match status {
        "paused" => AMBER,
        "running" => GREEN,
        s if s.starts_with("error") || s.contains("No ") || s.contains("failed") => {
            Color::from_rgb8(0xf7, 0x76, 0x8e)
        }
        _ => theme::fg_dim(),
    }
}

/// A pill-shaped control button that runs `action` on click.
fn control(text: &'static str, action: impl Fn() + 'static) -> impl IntoView {
    label(move || text.to_string())
        .style(|s| {
            s.padding_horiz(10.0)
                .padding_vert(4.0)
                .border_radius(5.0)
                .font_size(11.5)
                .color(theme::fg())
                .background(theme::bg_hover())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::border()).color(theme::accent()))
        })
        .on_click_stop(move |_| action())
}

fn section_title(text: &'static str) -> impl IntoView {
    label(move || text.to_string()).style(|s| {
        s.font_size(11.0)
            .font_bold()
            .color(theme::fg_dim())
            .padding_horiz(12.0)
            .padding_top(10.0)
            .padding_bottom(4.0)
    })
}

pub fn debug_panel(state: AppState) -> impl IntoView {
    let title =
        label(|| "Debug".to_string()).style(|s| s.font_size(13.0).font_bold().color(theme::fg()));

    let status = label(move || state.debug_status.get()).style(move |s| {
        s.flex_grow(1.0)
            .margin_left(10.0)
            .font_size(11.0)
            .font_family("monospace".to_string())
            .color(status_color(&state.debug_status.get()))
    });

    let close = label(|| "✕".to_string())
        .style(|s| {
            s.padding_horiz(8.0)
                .color(theme::fg_dim())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.color(theme::fg()))
        })
        .on_click_stop(move |_| state.debug_open.set(false));

    let header = stack((title, status, close)).style(|s| {
        s.flex_row()
            .items_center()
            .gap(8.0)
            .padding_horiz(12.0)
            .padding_vert(8.0)
            .width_full()
            .border_bottom(1.0)
            .border_color(theme::border())
    });

    let controls = stack((
        control("▶ Start / Continue", move || state.debug_start()),
        control("⤼ Step Over", move || state.debug_step_over()),
        control("⤓ Step Into", move || state.debug_step_into()),
        control("⤒ Step Out", move || state.debug_step_out()),
        control("■ Stop", move || state.debug_stop()),
    ))
    .style(|s| {
        s.flex_row()
            .flex_wrap(floem::style::FlexWrap::Wrap)
            .gap(6.0)
            .padding_horiz(12.0)
            .padding_vert(8.0)
            .width_full()
            .border_bottom(1.0)
            .border_color(theme::border())
    });

    // Call stack — click a frame to jump to its source line.
    let frames = dyn_stack(
        move || {
            state
                .debug_frames
                .get()
                .into_iter()
                .enumerate()
                .collect::<Vec<_>>()
        },
        |(i, _)| *i,
        move |(_, f)| {
            let path = f.path.clone();
            let line = f.line;
            let text = if f.path.is_empty() {
                format!("{}  :{}", f.name, f.line)
            } else {
                let base = Path::new(&f.path)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                format!("{}  {}:{}", f.name, base, f.line)
            };
            label(move || text.clone())
                .style(|s| {
                    s.font_size(11.5)
                        .font_family("monospace".to_string())
                        .color(theme::fg())
                        .padding_horiz(12.0)
                        .padding_vert(3.0)
                        .width_full()
                        .text_ellipsis()
                        .cursor(floem::style::CursorStyle::Pointer)
                        .hover(|s| s.background(theme::bg_hover()))
                })
                .on_click_stop(move |_| {
                    if !path.is_empty() {
                        let uri = path_to_uri(Path::new(&path));
                        state.jump_to(&uri, (line.max(1) - 1) as usize, 0);
                    }
                })
        },
    )
    .style(|s| s.flex_col().width_full());

    // Variables in the current frame.
    let vars = dyn_stack(
        move || {
            state
                .debug_vars
                .get()
                .into_iter()
                .enumerate()
                .collect::<Vec<_>>()
        },
        |(i, _)| *i,
        move |(_, v)| {
            let name = v.name.clone();
            let value = if v.ty.is_empty() {
                v.value.clone()
            } else {
                format!("{}  ({})", v.value, v.ty)
            };
            stack((
                label(move || name.clone()).style(|s| {
                    s.font_size(11.5)
                        .font_family("monospace".to_string())
                        .color(BLUE)
                        .min_width(120.0)
                }),
                label(move || value.clone()).style(|s| {
                    s.font_size(11.5)
                        .font_family("monospace".to_string())
                        .color(theme::fg())
                        .flex_grow(1.0)
                        .text_ellipsis()
                }),
            ))
            .style(|s| {
                s.flex_row()
                    .gap(8.0)
                    .padding_horiz(12.0)
                    .padding_vert(2.0)
                    .width_full()
            })
        },
    )
    .style(|s| s.flex_col().width_full());

    // Breakpoints across all files.
    let breakpoints = dyn_stack(
        move || {
            let mut out: Vec<(String, u32)> = Vec::new();
            state.debug_breakpoints.with(|m| {
                for (path, lines) in m {
                    for l in lines {
                        out.push((path.clone(), *l));
                    }
                }
            });
            out.sort();
            out.into_iter().enumerate().collect::<Vec<_>>()
        },
        |(i, _)| *i,
        move |(_, (path, line))| {
            let base = Path::new(&path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.clone());
            let text = format!("● {base}:{line}");
            let jump_path = path.clone();
            label(move || text.clone())
                .style(|s| {
                    s.font_size(11.5)
                        .font_family("monospace".to_string())
                        .color(Color::from_rgb8(0xf7, 0x76, 0x8e))
                        .padding_horiz(12.0)
                        .padding_vert(2.0)
                        .width_full()
                        .cursor(floem::style::CursorStyle::Pointer)
                        .hover(|s| s.background(theme::bg_hover()))
                })
                .on_click_stop(move |_| {
                    let uri = path_to_uri(Path::new(&jump_path));
                    state.jump_to(&uri, (line.max(1) - 1) as usize, 0);
                })
        },
    )
    .style(|s| s.flex_col().width_full());

    let hint = label(|| {
        "No session. Press ▶ (F5). Enable Xdebug in Grove with `grove debug on`, \
         then trigger a request (XDEBUG_TRIGGER)."
            .to_string()
    })
    .style(move |s| {
        let s = s.padding(14.0).color(theme::fg_dim()).font_size(11.5);
        if state.debug_client.get().is_none() && state.debug_frames.with(|f| f.is_empty()) {
            s
        } else {
            s.hide()
        }
    });

    let body = scroll(
        stack((
            section_title("Call Stack"),
            frames,
            section_title("Variables"),
            vars,
            section_title("Breakpoints"),
            breakpoints,
            hint,
        ))
        .style(|s| s.flex_col().width_full()),
    )
    .style(|s| s.flex_grow(1.0).width_full());

    let card = stack((header, controls, body)).style(|s| {
        s.flex_col()
            .width(560.0)
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
        if state.debug_open.get() {
            s
        } else {
            s.hide()
        }
    })
}
