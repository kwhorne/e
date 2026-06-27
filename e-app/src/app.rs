//! Application entry point and root view.

use std::path::PathBuf;

use floem::event::{Event, EventListener, EventPropagation};
use floem::ext_event::create_signal_from_channel;
use floem::keyboard::{Key, Modifiers, NamedKey};
use floem::reactive::{create_effect, Scope, SignalGet, SignalUpdate, SignalWith};
use floem::kurbo::Size;
use floem::views::{stack, Decorators};
use floem::window::WindowConfig;
use floem::{Application, IntoView};

use crate::breadcrumbs::breadcrumbs;
use crate::cmd_palette::command_palette;
use crate::completion::{completion_popup, hover_popup, signature_popup};
use crate::diff_view::diff_view;
use crate::editor_area::editor_area;
use crate::file_ops::file_op_prompt;
use crate::file_tree::file_tree;
use crate::find::find_bar;
use crate::markdown_view::markdown_preview;
use crate::outline::outline_panel;
use crate::palette::palette;
use crate::picker::picker_overlay;
use crate::rename::rename_bar;
use crate::problems::problems_panel;
use crate::state::AppState;
use crate::status::status_bar;
use crate::tabs::tab_bar;
use crate::terminal_view::{term_rename_prompt, terminal_panel};
use crate::theme;

/// Launch the editor.
pub fn launch() {
    Application::new()
        .window(
            move |_| app_view(),
            Some(
                WindowConfig::default()
                    .size(Size::new(1280.0, 820.0))
                    .title("e"),
            ),
        )
        .run();
}

/// Central keyboard shortcut dispatch. Returns true if the key was handled.
///
/// This is invoked both from the editor's key handler (so shortcuts work while
/// the editor is focused — it otherwise consumes every key) and from a global
/// fallback listener (for when nothing focusable is active).
pub(crate) fn handle_shortcut(state: AppState, key: &Key, mods: Modifiers) -> bool {
    let cmd = mods.meta() || mods.control();
    let shift = mods.shift();

    match key {
        Key::Character(s) => {
            let c = s.to_lowercase();
            match c.as_str() {
                _ if !cmd => false,
                "p" if shift => {
                    state.cmd.open.set(true);
                    true
                }
                "p" => {
                    state.palette_open.update(|o| *o = !*o);
                    true
                }
                "s" => {
                    state.save_active();
                    true
                }
                "w" => {
                    if state.terminal_focused.get() {
                        if let Some(id) = state.focused_term_id() {
                            state.close_terminal(id);
                        }
                    } else if let Some(id) = state.focused_active_id() {
                        state.close(id);
                    }
                    true
                }
                "t" => {
                    state.toggle_terminal();
                    true
                }
                "o" if shift => {
                    state.open_symbol_search();
                    true
                }
                "f" if shift => {
                    state.open_global_search();
                    true
                }
                "f" => {
                    state.open_find();
                    true
                }
                "\\" => {
                    state.toggle_split();
                    true
                }
                "1" => {
                    state.sidebar_open.update(|o| *o = !*o);
                    true
                }
                "d" => {
                    state.select_next_occurrence();
                    true
                }
                "m" if shift => {
                    state.toggle_md_preview();
                    true
                }
                "`" if mods.control() => {
                    state.toggle_terminal();
                    true
                }
                " " => {
                    if let Some(id) = state.focused_active_id() {
                        state.request_completion(id);
                    }
                    true
                }
                _ => false,
            }
        }
        Key::Named(named) => match named {
            NamedKey::F1 if mods.is_empty() => {
                state.request_hover();
                true
            }
            NamedKey::F2 if mods.is_empty() => {
                state.open_rename();
                true
            }
            NamedKey::F8 if mods.is_empty() => {
                theme::toggle();
                true
            }
            NamedKey::F12 if shift => {
                state.request_references();
                true
            }
            NamedKey::F12 if mods.is_empty() => {
                state.goto_definition();
                true
            }
            NamedKey::Space if cmd => {
                if let Some(id) = state.focused_active_id() {
                    state.request_completion(id);
                }
                true
            }
            NamedKey::Escape => {
                state.close_completion();
                state.close_hover();
                state.close_signature();
                state.picker.open.set(false);
                state.palette_open.set(false);
                state.cmd.open.set(false);
                state.md_preview.set(false);
                state.diff_open.set(false);
                state.close_find();
                state.close_rename();
                true
            }
            _ => false,
        },
        _ => false,
    }
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

    // Restore the saved theme, and persist it whenever it changes.
    theme::set_dark(crate::config::load_dark());
    create_effect(|_| {
        crate::config::save_dark(theme::is_dark());
    });

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

    // Idle auto-save: a ticker drives a UI-thread check every 500ms.
    {
        let (auto_tx, auto_rx) = std::sync::mpsc::channel::<()>();
        std::thread::spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_millis(500));
            if auto_tx.send(()).is_err() {
                break;
            }
        });
        let ticks = create_signal_from_channel(auto_rx);
        create_effect(move |_| {
            if ticks.get().is_some() {
                state.maybe_autosave();
            }
        });
    }

    let editor_column = stack((
        tab_bar(state),
        breadcrumbs(state),
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

    // Highlight the matching bracket as the cursor moves.
    create_effect(move |_| {
        if let Some(buf) = state.active_buffer() {
            if let Some(ed) = buf.editor.get() {
                ed.cursor.get(); // track caret movement
                state.update_bracket_marks();
            }
        }
    });

    // Re-run find-in-file whenever the query changes.
    create_effect(move |_| {
        if state.find.open.get() {
            state.find.query.get();
            state.run_find();
        }
    });

    let sidebar = stack((file_tree(state), outline_panel(state))).style(move |s| {
        let s = s
            .flex_col()
            .width(240.0)
            .height_full()
            .border_right(1.0)
            .border_color(theme::border());
        if state.sidebar_open.get() {
            s
        } else {
            s.hide()
        }
    });

    let main_row = stack((sidebar, editor_column)).style(|s| s.flex_row().size_full());

    stack((
        main_row,
        markdown_preview(state),
        diff_view(state),
        find_bar(state),
        rename_bar(state),
        file_op_prompt(state),
        term_rename_prompt(state),
        signature_popup(state),
        completion_popup(state),
        hover_popup(state),
        picker_overlay(state),
        palette(state),
        command_palette(state),
    ))
        .style(|s| s.size_full().background(theme::bg()).color(theme::fg()))
        .window_title(move || {
            let (name, dirty) = state
                .active_buffer()
                .map(|b| (b.file.display_name(), b.dirty.get()))
                .unwrap_or_else(|| ("e".to_string(), false));
            let mark = if dirty { "● " } else { "" };
            format!("{mark}{name} — e")
        })
        .on_event(EventListener::KeyDown, move |e| {
            if let Event::KeyDown(ke) = e {
                if handle_shortcut(state, &ke.key.logical_key, ke.modifiers) {
                    return EventPropagation::Stop;
                }
            }
            EventPropagation::Continue
        })
}
