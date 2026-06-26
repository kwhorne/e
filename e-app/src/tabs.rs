//! The tab strip above the editor area.

use floem::event::EventPropagation;
use floem::reactive::SignalGet;
use floem::views::{dyn_stack, label, scroll, stack, Decorators};
use floem::IntoView;

use crate::state::AppState;
use crate::theme;

pub fn tab_bar(state: AppState) -> impl IntoView {
    let tabs = dyn_stack(
        move || state.buffers.get(),
        |b| b.id,
        move |b| {
            let id = b.id;
            let dirty = b.dirty;
            let name = b.file.display_name();

            let title = label(move || {
                let mark = if dirty.get() { " ●" } else { "" };
                format!("{name}{mark}")
            })
            .style(|s| s.color(theme::fg()));

            let close = label(|| "×".to_string())
                .style(|s| {
                    s.padding_horiz(4.0)
                        .border_radius(4.0)
                        .color(theme::fg_dim())
                        .hover(|s| s.background(theme::bg_hover()).color(theme::fg()))
                })
                .on_click_stop(move |_| state.close(id));

            stack((title, close))
                .style(move |s| {
                    let s = s
                        .items_center()
                        .gap(6.0)
                        .padding_horiz(12.0)
                        .height(34.0)
                        .border_right(1.0)
                        .border_color(theme::border())
                        .cursor(floem::style::CursorStyle::Pointer);
                    if state.focused_active_id() == Some(id) {
                        s.background(theme::bg_active())
                    } else {
                        s.background(theme::bg_panel())
                            .hover(|s| s.background(theme::bg_hover()))
                    }
                })
                .on_click(move |_| {
                    state.focus_buffer(id);
                    EventPropagation::Stop
                })
        },
    )
    .style(|s| s.items_center());

    scroll(tabs).style(|s| {
        s.width_full()
            .height(34.0)
            .background(theme::bg_panel())
            .border_bottom(1.0)
            .border_color(theme::border())
    })
}
