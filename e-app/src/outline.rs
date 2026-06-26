//! Document outline panel — the active buffer's symbols (LSP documentSymbol).

use floem::peniko::Color;
use floem::reactive::{RwSignal, SignalGet};
use floem::views::{dyn_stack, label, scroll, stack, Decorators};
use floem::IntoView;

use crate::state::AppState;
use crate::theme;

#[derive(Clone)]
pub struct OutlineItem {
    pub name: String,
    pub kind: i64,
    pub line: u32,
    pub char: u32,
    pub depth: usize,
}

fn kind_icon(kind: i64) -> (&'static str, Color) {
    match kind {
        // Class, Struct, Interface, Enum
        5 | 23 | 11 | 10 => ("C", Color::from_rgb8(0xe5, 0xc0, 0x7b)),
        // Method, Function, Constructor
        6 | 12 | 9 => ("ƒ", Color::from_rgb8(0x61, 0xaf, 0xef)),
        // Property, Field, Variable
        7 | 8 | 13 => ("$", Color::from_rgb8(0xe0, 0x6c, 0x75)),
        // Constant, EnumMember
        14 | 22 => ("π", Color::from_rgb8(0xd1, 0x9a, 0x66)),
        // Namespace, Module, Package
        2 | 3 | 4 => ("{}", theme::FG_DIM),
        _ => ("•", theme::FG_DIM),
    }
}

pub fn outline_panel(state: AppState) -> impl IntoView {
    let items: RwSignal<Vec<OutlineItem>> = state.outline;

    let header = label(|| "OUTLINE".to_string()).style(|s| {
        s.height(28.0)
            .width_full()
            .items_center()
            .padding_horiz(12.0)
            .font_size(11.0)
            .color(theme::FG_DIM)
            .border_bottom(1.0)
            .border_color(theme::BORDER)
    });

    let rows = dyn_stack(
        move || items.get().into_iter().enumerate().collect::<Vec<_>>(),
        |(i, _)| *i,
        move |(_, it)| {
            let (icon, color) = kind_icon(it.kind);
            let indent = 8.0 + it.depth as f64 * 14.0;
            let (line, ch) = (it.line, it.char);
            stack((
                label(move || icon.to_string()).style(move |s| s.width(16.0).color(color)),
                label(move || it.name.clone())
                    .style(|s| s.color(theme::FG).text_ellipsis().flex_grow(1.0)),
            ))
            .style(move |s| {
                s.items_center()
                    .gap(4.0)
                    .height(22.0)
                    .width_full()
                    .padding_left(indent)
                    .padding_right(8.0)
                    .cursor(floem::style::CursorStyle::Pointer)
                    .hover(|s| s.background(theme::BG_HOVER))
            })
            .on_click_stop(move |_| {
                if let Some(buf) = state.active_buffer() {
                    if let Some(uri) = buf.uri {
                        state.jump_to(&uri, line as usize, ch as usize);
                    }
                }
            })
        },
    )
    .style(|s| s.flex_col().width_full());

    stack((header, scroll(rows).style(|s| s.flex_grow(1.0).width_full()))).style(move |s| {
        let s = s
            .flex_col()
            .width_full()
            .height(260.0)
            .background(theme::BG_PANEL)
            .border_top(1.0)
            .border_color(theme::BORDER);
        if items.get().is_empty() {
            s.hide()
        } else {
            s
        }
    })
}
