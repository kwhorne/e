//! The About dialog.

use floem::reactive::{SignalGet, SignalUpdate};
use floem::views::{container, label, stack, Decorators};
use floem::IntoView;

use crate::state::AppState;
use crate::theme;

fn line(text: &'static str, accent: bool, url: Option<&'static str>) -> impl IntoView {
    label(move || text.to_string())
        .style(move |s| {
            let s = s.font_size(14.0);
            let s = if accent {
                s.color(theme::accent())
                    .font_family("monospace".to_string())
            } else {
                s.color(theme::fg_dim())
            };
            if url.is_some() {
                s.cursor(floem::style::CursorStyle::Pointer)
            } else {
                s
            }
        })
        .on_click_stop(move |_| {
            if let Some(u) = url {
                let _ = std::process::Command::new("open").arg(u).spawn();
            }
        })
}

pub fn about_dialog(state: AppState) -> impl IntoView {
    let content = stack((
        label(|| "e".to_string())
            .style(|s| s.font_size(40.0).color(theme::fg()).margin_bottom(2.0)),
        label(|| format!("Version {}", env!("CARGO_PKG_VERSION"))).style(|s| {
            s.font_family("monospace".to_string())
                .font_size(13.0)
                .color(theme::fg_dim())
                .margin_bottom(16.0)
        }),
        line("A fast, native code editor in Rust.", false, None),
        line("kwhorne.com", true, Some("https://kwhorne.com")),
        line(
            "github.com/kwhorne/e",
            true,
            Some("https://github.com/kwhorne/e"),
        ),
        line("Knut W. Horne", false, None),
        label(|| "Close".to_string())
            .style(|s| {
                s.margin_top(18.0)
                    .padding_horiz(24.0)
                    .padding_vert(8.0)
                    .background(theme::bg())
                    .color(theme::fg())
                    .border(1.0)
                    .border_color(theme::border())
                    .border_radius(8.0)
                    .cursor(floem::style::CursorStyle::Pointer)
                    .hover(|s| s.background(theme::bg_hover()))
            })
            .on_click_stop(move |_| state.about_open.set(false)),
    ))
    .style(|s| {
        s.flex_col()
            .items_center()
            .gap(10.0)
            .width(420.0)
            .padding(32.0)
            .background(theme::bg_panel())
            .border(1.0)
            .border_color(theme::border())
            .border_radius(14.0)
    })
    .on_click_stop(|_| {});

    container(content)
        .style(move |s| {
            let s = s
                .absolute()
                .inset(0.0)
                .size_full()
                .items_center()
                .justify_center()
                .background(floem::peniko::Color::from_rgba8(0, 0, 0, 0x99));
            if state.about_open.get() {
                s
            } else {
                s.hide()
            }
        })
        .on_click_stop(move |_| state.about_open.set(false))
}
