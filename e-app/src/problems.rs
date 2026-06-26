//! Bottom "Problems" panel — lists LSP diagnostics for the active buffer.

use floem::peniko::Color;
use floem::views::{dyn_stack, label, scroll, stack, Decorators};
use floem::IntoView;
use lsp_types::DiagnosticSeverity;

use crate::state::AppState;
use crate::theme;

fn severity_color(severity: Option<DiagnosticSeverity>) -> Color {
    match severity {
        Some(DiagnosticSeverity::ERROR) => Color::from_rgb8(0xe0, 0x6c, 0x75),
        Some(DiagnosticSeverity::WARNING) => Color::from_rgb8(0xe5, 0xc0, 0x7b),
        _ => Color::from_rgb8(0x61, 0xaf, 0xef),
    }
}

pub fn problems_panel(state: AppState) -> impl IntoView {
    let header = label(move || {
        let n = state.active_diagnostics().len();
        format!("PROBLEMS · {n}")
    })
    .style(|s| {
        s.height(26.0)
            .width_full()
            .items_center()
            .padding_horiz(10.0)
            .font_size(11.0)
            .color(theme::FG_DIM)
            .border_bottom(1.0)
            .border_color(theme::BORDER)
    });

    let rows = dyn_stack(
        move || state.active_diagnostics().into_iter().enumerate().collect::<Vec<_>>(),
        |(i, _)| *i,
        move |(_, d)| {
            let color = severity_color(d.severity);
            let line = d.range.start.line + 1;
            let col = d.range.start.character + 1;
            let msg = d.message.replace('\n', " ");
            label(move || format!("{line}:{col}  {msg}"))
                .style(move |s| {
                    s.height(22.0)
                        .width_full()
                        .items_center()
                        .padding_horiz(12.0)
                        .text_ellipsis()
                        .color(color)
                        .hover(|s| s.background(theme::BG_HOVER))
                })
        },
    )
    .style(|s| s.flex_col().width_full());

    stack((header, scroll(rows).style(|s| s.width_full().flex_grow(1.0))))
        .style(move |s| {
            let s = s
                .flex_col()
                .width_full()
                .height(150.0)
                .background(theme::BG_PANEL)
                .border_top(1.0)
                .border_color(theme::BORDER);
            // Only show the panel when there are diagnostics.
            if state.active_diagnostics().is_empty() {
                s.hide()
            } else {
                s
            }
        })
}
