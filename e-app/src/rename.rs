//! Local rename (F2): rename every whole-word occurrence of the identifier
//! under the cursor within the current file. Works without an LSP.

use floem::keyboard::{Key, NamedKey};
use floem::reactive::{RwSignal, SignalGet};
use floem::views::{container, label, stack, text_input, Decorators};
use floem::IntoView;

use crate::state::AppState;
use crate::theme;

#[derive(Clone, Copy)]
pub struct RenameState {
    pub open: RwSignal<bool>,
    pub word: RwSignal<String>,
    pub new_name: RwSignal<String>,
}

impl RenameState {
    pub fn new() -> Self {
        Self {
            open: RwSignal::new(false),
            word: RwSignal::new(String::new()),
            new_name: RwSignal::new(String::new()),
        }
    }
}

pub fn rename_bar(state: AppState) -> impl IntoView {
    let rename = state.rename;

    let title = label(move || format!("Rename “{}” to:", rename.word.get()))
        .style(|s| s.color(theme::fg_dim()).font_size(12.0));

    let input = text_input(rename.new_name)
        .style(|s| {
            theme::input_colors(s)
                .width(220.0)
                .height(28.0)
                .padding_horiz(8.0)
                .border(1.0)
                .border_radius(4.0)
        })
        .request_focus(move || {
            rename.open.get();
        })
        .on_key_down(Key::Named(NamedKey::Escape), |_| true, move |_| {
            state.close_rename();
        })
        .on_key_down(Key::Named(NamedKey::Enter), |_| true, move |_| {
            state.apply_rename();
        });

    let box_ = stack((title, input)).style(|s| {
        s.items_center()
            .gap(10.0)
            .padding(10.0)
            .background(theme::bg_panel())
            .border(1.0)
            .border_color(theme::border())
            .border_radius(6.0)
    });

    container(box_).style(move |s| {
        let s = s.absolute().inset_top(8.0).width_full().justify_center();
        if rename.open.get() {
            s
        } else {
            s.hide()
        }
    })
}
