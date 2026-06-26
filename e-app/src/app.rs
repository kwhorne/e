//! Application entry point and root view.

use std::path::PathBuf;

use floem::ext_event::create_signal_from_channel;
use floem::keyboard::{Key, NamedKey};
use floem::reactive::{create_effect, Scope, SignalGet, SignalUpdate};
use floem::views::{stack, Decorators};
use floem::IntoView;

use crate::completion::{completion_popup, hover_popup};
use crate::editor_area::editor_area;
use crate::file_tree::file_tree;
use crate::palette::palette;
use crate::problems::problems_panel;
use crate::state::AppState;
use crate::status::status_bar;
use crate::tabs::tab_bar;
use crate::theme;

/// Launch the editor.
pub fn launch() {
    floem::launch(app_view);
}

/// Resolve the CLI argument into `(workspace_root, file_to_open)`.
fn resolve_args() -> (PathBuf, Option<PathBuf>) {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    match std::env::args().nth(1) {
        None => (cwd, None),
        Some(arg) => {
            let path = PathBuf::from(arg);
            let path = path.canonicalize().unwrap_or(path);
            if path.is_dir() {
                (path, None)
            } else {
                let root = path.parent().map(|p| p.to_path_buf()).unwrap_or(cwd);
                (root, Some(path))
            }
        }
    }
}

fn app_view() -> impl IntoView {
    let (root, file) = resolve_args();
    let state = AppState::new(Scope::current(), root);

    // Bridge the LSP reader thread's diagnostics into a UI-thread signal.
    if let Some(rx) = state.diag_rx.try_update(|opt| opt.take()).flatten() {
        let notif = create_signal_from_channel(rx);
        let diagnostics = state.diagnostics;
        create_effect(move |_| {
            if let Some(params) = notif.get() {
                diagnostics.update(|map| {
                    map.insert(params.uri.to_string(), params.diagnostics);
                });
            }
        });
    }

    if let Some(file) = file {
        state.open_path(file);
    }

    let editor_column = stack((
        tab_bar(state),
        editor_area(state).style(|s| s.flex_grow(1.0).width_full()),
        problems_panel(state),
        status_bar(state),
    ))
    .style(|s| s.flex_col().flex_grow(1.0).height_full());

    let main_row = stack((file_tree(state), editor_column))
        .style(|s| s.flex_row().size_full());

    stack((
        main_row,
        completion_popup(state),
        hover_popup(state),
        palette(state),
    ))
        .style(|s| s.size_full().background(theme::BG).color(theme::FG))
        .window_title(move || {
            let (name, dirty) = state
                .active_buffer()
                .map(|b| (b.file.display_name(), b.dirty.get()))
                .unwrap_or_else(|| ("e".to_string(), false));
            let mark = if dirty { "● " } else { "" };
            format!("{mark}{name} — e")
        })
        // ⌘P / Ctrl+P toggles the command palette.
        .on_key_down(
            Key::Character("p".into()),
            |m| m.meta() || m.control(),
            move |_| state.palette_open.update(|o| *o = !*o),
        )
        // ⌘S / Ctrl+S saves the active buffer.
        .on_key_down(
            Key::Character("s".into()),
            |m| m.meta() || m.control(),
            move |_| state.save_active(),
        )
        // ⌘Space / Ctrl+Space requests completion at the cursor.
        .on_key_down(
            Key::Character(" ".into()),
            |m| m.meta() || m.control(),
            move |_| {
                if let Some(id) = state.active.get() {
                    state.request_completion(id);
                }
            },
        )
        // F1 shows hover info for the symbol at the cursor.
        .on_key_down(Key::Named(NamedKey::F1), |m| m.is_empty(), move |_| {
            state.request_hover();
        })
        // Escape dismisses popups.
        .on_key_down(Key::Named(NamedKey::Escape), |m| m.is_empty(), move |_| {
            state.close_completion();
            state.close_hover();
        })
}
