//! Application entry point and root view.

use std::path::PathBuf;

use floem::ext_event::create_signal_from_channel;
use floem::keyboard::{Key, NamedKey};
use floem::reactive::{create_effect, Scope, SignalGet, SignalUpdate, SignalWith};
use floem::views::{stack, Decorators};
use floem::IntoView;

use crate::completion::{completion_popup, hover_popup, signature_popup};
use crate::editor_area::editor_area;
use crate::file_tree::file_tree;
use crate::find::find_bar;
use crate::outline::outline_panel;
use crate::palette::palette;
use crate::picker::picker_overlay;
use crate::problems::problems_panel;
use crate::state::AppState;
use crate::status::status_bar;
use crate::tabs::tab_bar;
use crate::terminal_view::terminal_panel;
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
                let uri = params.uri.to_string();
                let diags = params.diagnostics;
                diagnostics.update(|map| {
                    map.insert(uri.clone(), diags.clone());
                });
                // Feed inline squiggles into the matching buffer.
                state.apply_diagnostics_to_buffer(&uri, &diags);
            }
        });
    }

    // Bridge terminal output ticks into a repaint signal.
    if let Some(rx) = state.term_rx.try_update(|opt| opt.take()).flatten() {
        let ticks = create_signal_from_channel(rx);
        let term_tick = state.term_tick;
        create_effect(move |_| {
            if ticks.get().is_some() {
                term_tick.update(|t| *t += 1);
            }
        });
    }

    // Scrape Laravel project data (routes/views/config/env) in the background.
    state.load_laravel();

    // Restore the previous session, then open any file passed on the CLI.
    state.restore_session();
    if let Some(file) = file {
        state.open_path(file);
    }

    // Persist the session whenever the open files / panes change.
    create_effect(move |_| {
        state.buffers.with(|_| ());
        state.active.get();
        state.active2.get();
        state.split.get();
        state.save_session();
    });

    let editor_column = stack((
        tab_bar(state),
        editor_area(state).style(|s| s.flex_grow(1.0).width_full()),
        terminal_panel(state),
        problems_panel(state),
        status_bar(state),
    ))
    .style(|s| s.flex_col().flex_grow(1.0).height_full());

    // Keep the document outline in sync with the active buffer.
    create_effect(move |_| {
        state.active.get();
        state.request_outline();
    });

    // Re-run find-in-file whenever the query changes.
    create_effect(move |_| {
        if state.find.open.get() {
            state.find.query.get();
            state.run_find();
        }
    });

    let sidebar = stack((file_tree(state), outline_panel(state))).style(|s| {
        s.flex_col()
            .width(240.0)
            .height_full()
            .border_right(1.0)
            .border_color(theme::BORDER)
    });

    let main_row = stack((sidebar, editor_column)).style(|s| s.flex_row().size_full());

    stack((
        main_row,
        find_bar(state),
        signature_popup(state),
        completion_popup(state),
        hover_popup(state),
        picker_overlay(state),
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
        // F12 jumps to the definition of the symbol at the cursor.
        .on_key_down(Key::Named(NamedKey::F12), |m| m.is_empty(), move |_| {
            state.goto_definition();
        })
        // Shift+F12 finds references to the symbol at the cursor.
        .on_key_down(Key::Named(NamedKey::F12), |m| m.shift(), move |_| {
            state.request_references();
        })
        // ⌘T / Ctrl+T opens workspace symbol search.
        .on_key_down(
            Key::Character("t".into()),
            |m| m.meta() || m.control(),
            move |_| state.open_symbol_search(),
        )
        // ⌘⇧F / Ctrl+⇧F opens workspace-wide text search.
        .on_key_down(
            Key::Character("F".into()),
            |m| (m.meta() || m.control()) && m.shift(),
            move |_| state.open_global_search(),
        )
        // Ctrl+` toggles the integrated terminal.
        .on_key_down(
            Key::Character("`".into()),
            |m| m.control(),
            move |_| state.toggle_terminal(),
        )
        // ⌘F / Ctrl+F opens find-in-file.
        .on_key_down(
            Key::Character("f".into()),
            |m| m.meta() || m.control(),
            move |_| state.open_find(),
        )
        // ⌘\ / Ctrl+\ toggles the split view.
        .on_key_down(
            Key::Character("\\".into()),
            |m| m.meta() || m.control(),
            move |_| state.toggle_split(),
        )
        // Escape dismisses popups.
        .on_key_down(Key::Named(NamedKey::Escape), |m| m.is_empty(), move |_| {
            state.close_completion();
            state.close_hover();
            state.close_signature();
            state.picker.open.set(false);
        })
}
