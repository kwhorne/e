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
            .width(348.0)
            .padding_vert(12.0)
            .background(theme::bg())
            .border(1.0)
            .border_color(theme::border())
            .border_radius(8.0)
    })
}

pub fn about_dialog(state: AppState) -> impl IntoView {
    let icon = container(svg(|| ICON.to_string()).style(|s| s.size_full()))
        .style(|s| s.width(68.0).height(68.0).margin_bottom(12.0));

    let head = stack((
        icon,
        label(|| "e".to_string()).style(|s| s.font_size(30.0).color(theme::fg())),
        label(|| format!("Version {}", env!("CARGO_PKG_VERSION"))).style(|s| {
            s.font_family("monospace".to_string())
                .font_size(12.0)
                .color(theme::fg_dim())
                .margin_bottom(12.0)
        }),
        label(|| "A fast, native code editor in Rust.".to_string())
            .style(|s| s.color(theme::fg_dim()).font_size(13.0)),
    ))
    .style(|s| s.flex_col().items_center());

    let cards = stack((
        card("WEBSITE", "kwhorne.com", Some("https://kwhorne.com")),
        card("GITHUB", "github.com/kwhorne/e", Some("https://github.com/kwhorne/e")),
        card("DEVELOPED BY", "Knut W. Horne", None),
    ))
    .style(|s| s.flex_col().items_center().gap(8.0).margin_vert(18.0));

    let close = label(|| "Close".to_string())
        .style(|s| {
            s.padding_horiz(24.0)
                .padding_vert(7.0)
                .background(theme::bg())
                .color(theme::fg())
                .border(1.0)
                .border_color(theme::border())
                .border_radius(8.0)
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::bg_hover()))
        })
        .on_click_stop(move |_| state.about_open.set(false));

    let box_ = stack((head, cards, close))
        .style(|s| {
            s.flex_col()
                .items_center()
                .width(400.0)
                .padding(26.0)
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
