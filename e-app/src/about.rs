//! The About dialog.

use floem::reactive::{SignalGet, SignalUpdate};
use floem::views::{container, label, stack, svg, Decorators};
use floem::IntoView;

use crate::state::AppState;
use crate::theme;

const ICON: &str = include_str!("../../icons/e.svg");

fn open_url(url: &str) {
    let _ = std::process::Command::new("open").arg(url).spawn();
}

/// One info card: an uppercase label over a (clickable) value.
fn card(heading: &'static str, value: &'static str, url: Option<&'static str>) -> impl IntoView {
    let val = label(move || value.to_string()).style(move |s| {
        let s = s.font_family("monospace".to_string()).font_size(14.0).color(theme::accent());
        if url.is_some() {
            s.cursor(floem::style::CursorStyle::Pointer)
        } else {
            s
        }
    });

    stack((
        label(move || heading.to_string())
            .style(|s| s.font_size(11.0).color(theme::fg_dim()).margin_bottom(4.0)),
        val.on_click_stop(move |_| {
            if let Some(u) = url {
                open_url(u);
            }
        }),
    ))
    .style(|s| {
        s.flex_col()
            .items_center()
            .gap(2.0)
            .width_full()
            .padding_vert(12.0)
            .background(theme::bg())
            .border(1.0)
            .border_color(theme::border())
            .border_radius(8.0)
    })
}

pub fn about_dialog(state: AppState) -> impl IntoView {
    let box_ = stack((
        svg(|| ICON.to_string()).style(|s| s.width(84.0).height(84.0).margin_bottom(16.0)),
        label(|| "e".to_string()).style(|s| s.font_size(34.0).color(theme::fg())),
        label(|| format!("Version {}", env!("CARGO_PKG_VERSION")))
            .style(|s| s.font_family("monospace".to_string()).font_size(13.0).color(theme::fg_dim()).margin_bottom(18.0)),
        label(|| "The editor for the rest of us — a fast, native code editor written in Rust.".to_string())
            .style(|s| s.color(theme::fg_dim()).font_size(14.0).line_height(1.4).margin_bottom(22.0)),
        card("WEBSITE", "kwhorne.com", Some("https://kwhorne.com")),
        card("GITHUB", "github.com/kwhorne/e", Some("https://github.com/kwhorne/e")),
        card("DEVELOPED BY", "Knut W. Horne", None),
        label(|| "Close".to_string())
            .style(|s| {
                s.margin_top(22.0)
                    .padding_horiz(24.0)
                    .padding_vert(8.0)
                    .background(theme::bg_panel())
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
            .width(440.0)
            .padding(28.0)
            .gap(8.0)
            .background(theme::bg_panel())
            .border(1.0)
            .border_color(theme::border())
            .border_radius(14.0)
    })
    .on_click_stop(|_| {});

    container(box_)
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
