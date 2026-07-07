//! Git diff reading-mode: the active file's changes vs `HEAD`.

use floem::peniko::Color;
use floem::reactive::SignalGet;
use floem::views::editor::text::Document;
use floem::views::{dyn_stack, empty, label, scroll, Decorators};
use floem::IntoView;

use e_core::git::{self, DiffKind};

use crate::state::AppState;
use crate::theme;

pub fn diff_view(state: AppState) -> impl IntoView {
    floem::views::dyn_container(
        move || {
            let visible = state.diff_open.get();
            let rev = state
                .active_buffer()
                .map(|b| b.doc.cache_rev().get())
                .unwrap_or(0);
            (visible, rev)
        },
        move |(visible, _rev)| {
            if !visible {
                return empty().into_any();
            }
            let Some(buf) = state.active_buffer() else {
                return empty().into_any();
            };
            let Some(path) = buf.file.path.clone() else {
                return empty().into_any();
            };
            let head = git::head_text(&path).unwrap_or_default();
            let current = buf.doc.text().to_string();
            let lines = git::diff(&head, &current);

            let rows = dyn_stack(
                move || lines.clone().into_iter().enumerate().collect::<Vec<_>>(),
                |(i, _)| *i,
                move |(_, dl)| {
                    let (sign, color, bg) = match dl.kind {
                        DiffKind::Added => (
                            "+",
                            Color::from_rgb8(0x98, 0xc3, 0x79),
                            Color::from_rgba8(0x6a, 0xb0, 0x4a, 0x22),
                        ),
                        DiffKind::Removed => (
                            "-",
                            Color::from_rgb8(0xe0, 0x6c, 0x75),
                            Color::from_rgba8(0xe0, 0x6c, 0x75, 0x22),
                        ),
                        DiffKind::Context => {
                            ("\u{00a0}", theme::fg_dim(), Color::from_rgba8(0, 0, 0, 0))
                        }
                    };
                    let text = format!("{sign} {}", dl.text);
                    label(move || text.clone()).style(move |s| {
                        s.width_full()
                            .padding_horiz(12.0)
                            .font_family("monospace".to_string())
                            .font_size(13.0)
                            .line_height(1.4)
                            .color(color)
                            .background(bg)
                    })
                },
            )
            .style(|s| s.flex_col().width_full().padding_vert(8.0));

            scroll(rows)
                .style(|s| s.size_full().background(theme::bg()))
                .into_any()
        },
    )
    .style(move |s| {
        let s = s.absolute().inset(0.0).size_full();
        if state.diff_open.get() {
            s
        } else {
            s.hide()
        }
    })
}

/// A modal comparing the active file with another file picked from disk.
pub fn file_diff_view(state: AppState) -> impl IntoView {
    use floem::reactive::{SignalGet, SignalWith};
    use floem::views::{container, label, stack};

    let close = label(|| "✕".to_string())
        .style(|s| {
            s.padding_horiz(8.0)
                .color(theme::fg_dim())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.color(theme::fg()))
        })
        .on_click_stop(move |_| state.close_file_diff());
    let title = label(move || match state.file_diff.get() {
        Some((l, r, _)) => format!("{l}  ↔  {r}"),
        None => String::new(),
    })
    .style(|s| {
        s.flex_grow(1.0)
            .font_size(13.0)
            .font_bold()
            .color(theme::fg())
    });
    let header = stack((title, close)).style(|s| {
        s.flex_row()
            .items_center()
            .width_full()
            .padding(10.0)
            .border_bottom(1.0)
            .border_color(theme::border())
    });

    let rows = dyn_stack(
        move || {
            state
                .file_diff
                .get()
                .map(|(_, _, lines)| lines)
                .unwrap_or_default()
                .into_iter()
                .enumerate()
                .collect::<Vec<_>>()
        },
        |(i, _)| *i,
        move |(_, dl)| {
            let (sign, color, bg) = match dl.kind {
                DiffKind::Added => (
                    "+",
                    Color::from_rgb8(0x98, 0xc3, 0x79),
                    Color::from_rgba8(0x6a, 0xb0, 0x4a, 0x22),
                ),
                DiffKind::Removed => (
                    "-",
                    Color::from_rgb8(0xe0, 0x6c, 0x75),
                    Color::from_rgba8(0xe0, 0x6c, 0x75, 0x22),
                ),
                DiffKind::Context => ("\u{00a0}", theme::fg_dim(), Color::from_rgba8(0, 0, 0, 0)),
            };
            let text = format!("{sign} {}", dl.text);
            label(move || text.clone()).style(move |s| {
                s.width_full()
                    .padding_horiz(12.0)
                    .font_family("monospace".to_string())
                    .font_size(12.5)
                    .color(color)
                    .background(bg)
            })
        },
    )
    .style(|s| s.flex_col().width_full().padding_vert(8.0));

    let card = stack((
        header,
        scroll(rows).style(|s| s.flex_grow(1.0).width_full()),
    ))
    .style(|s| {
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
            .background(Color::from_rgba8(0, 0, 0, 130));
        if state.file_diff.with(|d| d.is_some()) {
            s
        } else {
            s.hide()
        }
    })
}
