//! Modal confirmations: unsaved-changes-on-close and on-disk-change conflicts.

use floem::reactive::{SignalGet, SignalWith};
use floem::views::{container, label, stack, Decorators};
use floem::IntoView;

use crate::state::AppState;
use crate::theme;

fn button(text: &'static str, primary: bool) -> impl IntoView {
    label(move || text.to_string()).style(move |s| {
        let s = s
            .padding_horiz(16.0)
            .padding_vert(7.0)
            .border_radius(7.0)
            .font_size(13.0)
            .cursor(floem::style::CursorStyle::Pointer);
        if primary {
            s.background(theme::accent())
                .color(floem::peniko::Color::from_rgb8(0x14, 0x16, 0x1b))
        } else {
            s.background(theme::bg())
                .color(theme::fg())
                .border(1.0)
                .border_color(theme::border())
                .hover(|s| s.background(theme::bg_hover()))
        }
    })
}

/// "Save changes before closing?" dialog.
pub fn close_confirm_dialog(state: AppState) -> impl IntoView {
    let name = move || {
        let id = state.close_confirm.get();
        id.and_then(|id| {
            state
                .buffers
                .with(|bs| bs.iter().find(|b| b.id == id).map(|b| b.file.display_name()))
        })
        .unwrap_or_default()
    };

    let box_ = stack((
        label(move || format!("Save changes to “{}”?", name()))
            .style(|s| s.color(theme::fg()).font_size(15.0)),
        label(|| "Your changes will be lost if you don't save them.".to_string())
            .style(|s| s.color(theme::fg_dim()).font_size(13.0).margin_top(4.0)),
        stack((
            button("Save", true).on_click_stop(move |_| state.confirm_close_save()),
            button("Don't Save", false).on_click_stop(move |_| state.confirm_close_discard()),
            button("Cancel", false).on_click_stop(move |_| state.cancel_close()),
        ))
        .style(|s| s.gap(8.0).margin_top(18.0)),
    ))
    .style(|s| {
        s.flex_col()
            .width(420.0)
            .padding(24.0)
            .background(theme::bg_panel())
            .border(1.0)
            .border_color(theme::border())
            .border_radius(12.0)
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
            if state.close_confirm.get().is_some() {
                s
            } else {
                s.hide()
            }
        })
        .on_click_stop(move |_| state.cancel_close())
}

/// A bar shown when the caret is inside a git merge-conflict block, offering
/// to accept the current, incoming, or both sides.
pub fn merge_conflict_bar(state: AppState) -> impl IntoView {
    let in_conflict = move || {
        // Re-evaluate as the caret moves / the document changes.
        state.cursor_info();
        state.active_has_conflicts()
    };

    let small_btn = |text: &'static str| {
        label(move || text.to_string()).style(|s| {
            s.padding_horiz(10.0)
                .padding_vert(3.0)
                .border_radius(5.0)
                .font_size(12.0)
                .border(1.0)
                .border_color(theme::border())
                .color(theme::fg())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::bg_hover()))
        })
    };

    stack((
        label(|| "Merge conflict".to_string())
            .style(|s| s.color(theme::fg()).font_size(12.0).flex_grow(1.0)),
        small_btn("Accept Current").on_click_stop(move |_| state.resolve_conflict(0)),
        small_btn("Accept Incoming").on_click_stop(move |_| state.resolve_conflict(1)),
        small_btn("Accept Both").on_click_stop(move |_| state.resolve_conflict(2)),
    ))
    .style(move |s| {
        let s = s
            .items_center()
            .gap(8.0)
            .width_full()
            .padding_horiz(12.0)
            .padding_vert(6.0)
            .background(floem::peniko::Color::from_rgb8(0x3a, 0x2a, 0x40))
            .border_bottom(1.0)
            .border_color(theme::border());
        if in_conflict() {
            s
        } else {
            s.hide()
        }
    })
}

/// A thin bar shown atop the editor when the active file changed on disk while
/// it had unsaved edits.
pub fn disk_conflict_bar(state: AppState) -> impl IntoView {
    let changed = move || {
        state
            .active_buffer()
            .map(|b| b.disk_changed.get())
            .unwrap_or(false)
    };

    let small_btn = |text: &'static str| {
        label(move || text.to_string()).style(|s| {
            s.padding_horiz(10.0)
                .padding_vert(3.0)
                .border_radius(5.0)
                .font_size(12.0)
                .border(1.0)
                .border_color(theme::border())
                .color(theme::fg())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::bg_hover()))
        })
    };

    stack((
        label(|| "This file has changed on disk.".to_string())
            .style(|s| s.color(theme::fg()).font_size(12.0).flex_grow(1.0)),
        small_btn("Reload").on_click_stop(move |_| state.reload_active_from_disk()),
        small_btn("Keep yours").on_click_stop(move |_| state.keep_active_version()),
    ))
    .style(move |s| {
        let s = s
            .items_center()
            .gap(8.0)
            .width_full()
            .padding_horiz(12.0)
            .padding_vert(6.0)
            .background(floem::peniko::Color::from_rgb8(0x4a, 0x3b, 0x1a))
            .border_bottom(1.0)
            .border_color(theme::border());
        if changed() {
            s
        } else {
            s.hide()
        }
    })
}
