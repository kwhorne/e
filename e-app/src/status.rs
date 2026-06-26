//! The bottom status bar — reflects the active buffer.

use floem::reactive::SignalGet;
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

    let right = stack((diags, language)).style(|s| s.items_center().gap(14.0));

    stack((left, right)).style(|s| {
        s.height(24.0)
            .width_full()
            .items_center()
            .justify_between()
            .padding_horiz(10.0)
            .background(theme::BG_PANEL)
            .border_top(1.0)
            .border_color(theme::BORDER)
            .color(theme::FG_DIM)
    })
}
