//! The bottom status bar — reflects the active buffer.

use floem::reactive::SignalGet;
use floem::views::editor::text::Document;
use floem::views::{label, stack, Decorators};
use floem::IntoView;

use crate::state::AppState;
use crate::theme;

pub fn status_bar(state: AppState) -> impl IntoView {
    let left = label(move || match state.active_buffer() {
        Some(b) => {
            let mark = if b.dirty.get() { " ●" } else { "" };
            format!("{}{mark}", b.file.display_name())
        }
        None => String::new(),
    });

    let position = label(move || match state.cursor_info() {
        Some((line, col, sel)) if sel > 0 => format!("Ln {line}, Col {col}  ({sel} sel)"),
        Some((line, col, _)) => format!("Ln {line}, Col {col}"),
        None => String::new(),
    });

    let diags = label(move || {
        let (errors, warnings) = state.active_diagnostic_counts();
        if errors == 0 && warnings == 0 {
            String::new()
        } else {
            format!("⨯ {errors}   ⚠ {warnings}")
        }
    });

    let language = label(move || match state.active_buffer() {
        Some(b) => b.file.language.name().to_string(),
        None => String::new(),
    });

    let branch = label(move || {
        state
            .git_branch
            .get()
            .map(|b| format!("⎇ {b}"))
            .unwrap_or_default()
    });

    let indent = label(move || format!("Spaces: {}", state.settings.get().tab_width));

    let line_ending = label(move || match state.active_buffer() {
        Some(b) if b.doc.text().to_string().contains("\r\n") => "CRLF".to_string(),
        Some(_) => "LF".to_string(),
        None => String::new(),
    })
    .style(|s| s.cursor(floem::style::CursorStyle::Pointer).hover(|s| s.color(theme::fg())))
    .popout_menu(move || {
        floem::menu::Menu::new("Line endings")
            .entry(floem::menu::MenuItem::new("LF").action(move || state.set_line_ending(false)))
            .entry(floem::menu::MenuItem::new("CRLF").action(move || state.set_line_ending(true)))
    });

    let encoding = label(move || match state.active_buffer() {
        Some(_) => "UTF-8".to_string(),
        None => String::new(),
    });

    let blame = label(move || {
        state.blame_rev.get();
        state.cursor_info(); // re-render on caret movement
        state.active_line_blame().unwrap_or_default()
    })
    .style(|s| s.color(theme::fg_dim()).text_ellipsis().max_width(360.0));

    let right = stack((diags, branch, position, indent, line_ending, encoding, language))
        .style(|s| s.items_center().gap(14.0));

    let left = stack((left, blame)).style(|s| s.items_center().gap(14.0).min_width(0.0));

    stack((left, right)).style(|s| {
        s.height(24.0)
            .width_full()
            .items_center()
            .justify_between()
            .padding_horiz(10.0)
            .background(theme::bg_panel())
            .border_top(1.0)
            .border_color(theme::border())
            .color(theme::fg_dim())
    })
}
