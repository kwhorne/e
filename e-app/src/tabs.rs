//! The tab strip above the editor area.

use floem::event::{EventListener, EventPropagation};
use floem::menu::{Menu, MenuItem};
use floem::reactive::{RwSignal, SignalGet, SignalUpdate};
use floem::views::{dyn_stack, label, scroll, stack, Decorators};
use floem::IntoView;

use crate::state::AppState;
use crate::theme;

pub fn tab_bar(state: AppState) -> impl IntoView {
    // The tab id currently being dragged (for reordering).
    let drag_tab: RwSignal<Option<u64>> = RwSignal::new(None);

    let tabs = dyn_stack(
        move || state.buffers.get(),
        |b| b.id,
        move |b| {
            let id = b.id;
            let dirty = b.dirty;
            let name = b.file.display_name();

            let title = label(move || {
                let pin = if state.is_pinned(id) { "📌 " } else { "" };
                let mark = if dirty.get() { " ●" } else { "" };
                format!("{pin}{name}{mark}")
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
                .draggable()
                .dragging_style(|s| {
                    s.border(1.0)
                        .border_color(theme::accent())
                        .background(theme::bg_hover())
                })
                .on_event_stop(EventListener::DragStart, move |_| drag_tab.set(Some(id)))
                .on_event_stop(EventListener::DragEnd, move |_| drag_tab.set(None))
                .on_event_stop(EventListener::Drop, move |_| {
                    if let Some(src) = drag_tab.get_untracked() {
                        state.reorder_tab(src, id);
                    }
                    drag_tab.set(None);
                })
                .context_menu(move || {
                    let pin_label = if state.is_pinned(id) { "Unpin Tab" } else { "Pin Tab" };
                    Menu::new("")
                        .entry(MenuItem::new(pin_label).action(move || state.toggle_pin(id)))
                        .separator()
                        .entry(MenuItem::new("Close").action(move || state.close(id)))
                        .entry(MenuItem::new("Close Others").action(move || state.close_others(id)))
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
