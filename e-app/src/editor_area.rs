//! The central editor area: one live editor per open buffer, only the active
//! one visible. Hidden editors stay alive in the view tree, so each tab keeps
//! its own cursor and scroll position.

use std::rc::Rc;

use floem::reactive::{SignalGet, SignalWith};
use floem::views::editor::text::{default_dark_color, Document, SimpleStyling};
use floem::views::{container, dyn_stack, label, stack, text_editor, Decorators};
use floem::IntoView;

use crate::state::AppState;
use crate::theme;

pub fn editor_area(state: AppState) -> impl IntoView {
    let editors = dyn_stack(
        move || state.buffers.get(),
        |b| b.id,
        move |b| {
            let id = b.id;
            let active = state.active;

            let editor = text_editor("")
                .use_doc(b.doc.clone() as Rc<dyn Document>)
                .styling(SimpleStyling::new())
                .editor_style(default_dark_color)
                .style(|s| s.size_full());

            container(editor).style(move |s| {
                if active.get() == Some(id) {
                    s.size_full()
                } else {
                    s.hide()
                }
            })
        },
    )
    .style(|s| s.size_full());

    // Empty-state shown when no buffer is open.
    let placeholder = container(label(|| "No file open — press ⌘P to find a file".to_string()))
        .style(|s| {
            s.size_full()
                .items_center()
                .justify_center()
                .color(theme::FG_DIM)
        });

    stack((
        placeholder.style(move |s| {
            if state.buffers.with(|b| b.is_empty()) {
                s.size_full()
            } else {
                s.hide()
            }
        }),
        editors,
    ))
    .style(|s| s.size_full().background(theme::BG))
}
