//! Bottom "Problems" panel — LSP diagnostics across all open files.

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

/// One rendered row: a file header or a diagnostic line.
#[derive(Clone)]
enum Row {
    Header(String, usize, usize),
    Item {
        uri: String,
        line: u32,
        col: u32,
        msg: String,
        severity: Option<DiagnosticSeverity>,
    },
}

fn rows(state: AppState) -> Vec<Row> {
    let mut rows = Vec::new();
    for (uri, diags) in state.all_diagnostics() {
        let errors = diags
            .iter()
            .filter(|d| d.severity == Some(DiagnosticSeverity::ERROR))
            .count();
        let warnings = diags
            .iter()
            .filter(|d| d.severity == Some(DiagnosticSeverity::WARNING))
            .count();
        rows.push(Row::Header(state.rel_path(&uri), errors, warnings));
        for d in diags {
            rows.push(Row::Item {
                uri: uri.clone(),
                line: d.range.start.line,
                col: d.range.start.character,
                msg: d.message.replace('\n', " "),
                severity: d.severity,
            });
        }
    }
    rows
}

pub fn problems_panel(state: AppState) -> impl IntoView {
    let header = label(move || format!("PROBLEMS · {}", state.total_diagnostic_count())).style(|s| {
        s.height(26.0)
            .width_full()
            .items_center()
            .padding_horiz(10.0)
            .font_size(11.0)
            .color(theme::fg_dim())
            .border_bottom(1.0)
            .border_color(theme::border())
    });

    let list = dyn_stack(
        move || rows(state).into_iter().enumerate().collect::<Vec<_>>(),
        |(i, _)| *i,
        move |(_, row)| match row {
            Row::Header(rel, errors, warnings) => label(move || {
                format!("{rel}   ⨯ {errors}  ⚠ {warnings}")
            })
            .style(|s| {
                s.height(24.0)
                    .width_full()
                    .items_center()
                    .padding_horiz(10.0)
                    .font_size(12.0)
                    .color(theme::fg())
                    .background(theme::bg_panel())
            })
            .into_any(),
            Row::Item {
                uri,
                line,
                col,
                msg,
                severity,
            } => {
                let color = severity_color(severity);
                label(move || format!("{}:{}  {msg}", line + 1, col + 1))
                    .style(move |s| {
                        s.height(22.0)
                            .width_full()
                            .items_center()
                            .padding_left(28.0)
                            .padding_right(10.0)
                            .text_ellipsis()
                            .color(color)
                            .cursor(floem::style::CursorStyle::Pointer)
                            .hover(|s| s.background(theme::bg_hover()))
                    })
                    .on_click_stop(move |_| state.jump_to(&uri, line as usize, col as usize))
                    .into_any()
            }
        },
    )
    .style(|s| s.flex_col().width_full());

    stack((header, scroll(list).style(|s| s.flex_grow(1.0).width_full()))).style(move |s| {
        let s = s
            .flex_col()
            .width_full()
            .height(160.0)
            .background(theme::bg())
            .border_top(1.0)
            .border_color(theme::border());
        if state.total_diagnostic_count() == 0 {
            s.hide()
        } else {
            s
        }
    })
}
