//! Laravel Tinker scratchpad (⌘⌥T): write PHP, run it against the app's state
//! via `php artisan tinker`, and see the output.

use floem::peniko::Color;
use floem::reactive::{SignalGet, SignalUpdate};
use floem::views::editor::text::Document;
use floem::views::{container, empty, label, scroll, stack, text_editor, Decorators};
use floem::IntoView;

use crate::state::AppState;
use crate::theme;

pub fn tinker_panel(state: AppState) -> impl IntoView {
    let editor = text_editor("// Laravel Tinker — write PHP and press ⌘↵ to run\n");
    let doc = editor.doc();
    let doc_key = doc.clone();
    let doc_run = doc.clone();

    let title = label(|| "Tinker".to_string()).style(|s| {
        s.flex_grow(1.0)
            .font_size(13.0)
            .font_bold()
            .color(theme::fg())
    });
    let run = label(|| "▶ Run  ⌘↵".to_string())
        .style(move |s| {
            let base = s
                .padding_horiz(12.0)
                .height(26.0)
                .items_center()
                .border_radius(5.0)
                .font_size(12.0)
                .cursor(floem::style::CursorStyle::Pointer);
            if state.tinker_running.get() {
                base.background(theme::bg_hover()).color(theme::fg_dim())
            } else {
                base.background(theme::accent())
                    .color(Color::from_rgb8(0x14, 0x16, 0x1b))
            }
        })
        .on_click_stop(move |_| state.run_tinker(doc_run.text().to_string()));
    let close = label(|| "✕".to_string())
        .style(|s| {
            s.padding_horiz(8.0)
                .color(theme::fg_dim())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.color(theme::fg()))
        })
        .on_click_stop(move |_| state.tinker_open.set(false));
    let header = stack((title, run, close)).style(|s| {
        s.flex_row()
            .items_center()
            .gap(10.0)
            .padding_horiz(12.0)
            .padding_vert(8.0)
            .width_full()
            .border_bottom(1.0)
            .border_color(theme::border())
    });

    let code = editor
        .style(|s| {
            s.width_full()
                .height(220.0)
                .font_family("monospace".to_string())
                .font_size(13.0)
                .padding(8.0)
        })
        .on_key_down(
            floem::keyboard::Key::Named(floem::keyboard::NamedKey::Enter),
            |m| m.meta() || m.control(),
            move |_| state.run_tinker(doc_key.text().to_string()),
        );

    let output = scroll(label(move || state.tinker_output.get()).style(|s| {
        s.font_family("monospace".to_string())
            .font_size(12.0)
            .padding(10.0)
            .color(theme::fg())
    }))
    .style(|s| {
        s.flex_grow(1.0)
            .width_full()
            .border_top(1.0)
            .border_color(theme::border())
            .background(theme::bg())
    });

    let card = stack((header, code, output)).style(|s| {
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
            .background(Color::from_rgba8(0, 0, 0, 120));
        if state.tinker_open.get() {
            s
        } else {
            s.hide()
        }
    })
}
