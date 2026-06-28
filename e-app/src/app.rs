//! Application entry point and root view.

use std::path::PathBuf;

use floem::event::{Event, EventListener, EventPropagation};
use floem::ext_event::create_signal_from_channel;
use floem::keyboard::{Key, Modifiers};
use floem::kurbo::Size;
use floem::reactive::{create_effect, Scope, SignalGet, SignalUpdate, SignalWith};
use floem::views::{dyn_container, stack, Decorators};
use floem::window::WindowConfig;
use floem::{Application, IntoView};

use crate::about::about_dialog;
use crate::recent::recent_palette;
use crate::task_palette::task_palette;
use crate::settings_view::settings_view;
use crate::dialogs::{close_confirm_dialog, disk_conflict_bar, merge_conflict_bar};
use crate::editing::goto_bar;
use crate::agent_view::agent_panel;
use crate::update_view::update_notice;
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
use crate::problems::problems_panel;
use crate::rename::rename_bar;
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
/// Keys are resolved through the (default + user-overridable) keymap and routed
/// to the matching command. Invoked from the editor key handler and a global
/// fallback listener.
pub(crate) fn handle_shortcut(state: AppState, key: &Key, mods: Modifiers) -> bool {
    match crate::keymap::command_for(key, mods) {
        Some(id) => crate::commands::dispatch(state, &id),
        None => false,
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

#[derive(Clone, Copy, PartialEq)]
enum ResizeSide {
    Left,
    Right,
}

/// A thin, draggable vertical handle that resizes a panel by updating `width`.
/// Uses pointer capture (`request_active`) so the drag keeps tracking even when
/// the cursor leaves the 6px hit area; the `width.get() ± delta` formula is
/// self-correcting as the handle re-positions during the drag.
fn resize_handle(
    state: AppState,
    side: ResizeSide,
    width: floem::reactive::RwSignal<f64>,
    open: floem::reactive::RwSignal<bool>,
    min: f64,
    max: f64,
) -> impl IntoView {
    let _ = state;
    let drag_start: floem::reactive::RwSignal<Option<f64>> = floem::reactive::RwSignal::new(None);
    let view = floem::views::empty();
    let id = floem::View::id(&view);
    view.on_event_stop(EventListener::PointerDown, move |e| {
        id.request_active();
        if let Event::PointerDown(pe) = e {
            drag_start.set(Some(pe.pos.x));
        }
    })
    .on_event_stop(EventListener::PointerMove, move |e| {
        if let Event::PointerMove(pe) = e {
            if let Some(start) = drag_start.get_untracked() {
                let delta = pe.pos.x - start;
                let cur = width.get_untracked();
                let new = match side {
                    ResizeSide::Left => cur + delta,
                    ResizeSide::Right => cur - delta,
                };
                width.set(new.clamp(min, max));
            }
        }
    })
    .on_event_stop(EventListener::PointerUp, move |_| drag_start.set(None))
    .style(move |s| {
        let s = s
            .absolute()
            .width(6.0)
            .height_full()
            .z_index(10)
            .cursor(floem::style::CursorStyle::ColResize);
        let s = match side {
            ResizeSide::Left => s.inset_left(width.get() - 3.0),
            ResizeSide::Right => s.inset_right(width.get() - 3.0),
        };
        let s = s.hover(|s| s.background(theme::accent()));
        if open.get() {
            s
        } else {
            s.hide()
        }
    })
}

fn app_view() -> impl IntoView {
    let (root, file) = resolve_args();
    let state = AppState::new(Scope::current(), root);
    crate::snippets::set_user(crate::config::load_user_snippets());
    crate::keymap::load(crate::config::load_user_keybindings());

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

    // Quietly check GitHub for a newer release on startup.
    state.check_for_updates(false);
    // Populate the branch/status once so the status bar shows it.
    state.refresh_git_status();

    // Track recently-used files (newest first) for the ⌘E switcher.
    create_effect(move |_| {
        if let Some(id) = state.focused_active_id() {
            if let Some(path) = state
                .buffers
                .with(|bs| bs.iter().find(|b| b.id == id).and_then(|b| b.file.path.clone()))
            {
                state.push_recent(path);
            }
        }
    });

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
                state.check_external_changes();
            }
        });
    }

    let editor_column = stack((
        tab_bar(state),
        breadcrumbs(state),
        disk_conflict_bar(state),
        merge_conflict_bar(state),
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
        state.request_inlay_hints_active();
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
            state.find.case_sensitive.get();
            state.find.whole_word.get();
            state.find.use_regex.get();
            state.run_find();
        }
    });

    let sidebar_content = dyn_container(
        move || state.git_panel_open.get(),
        move |git| {
            if git {
                crate::git_view::git_panel(state).into_any()
            } else {
                stack((file_tree(state), outline_panel(state)))
                    .style(|s| s.flex_col().size_full())
                    .into_any()
            }
        },
    )
    .style(|s| s.size_full());

    let sidebar = sidebar_content.style(move |s| {
        let s = s
            .flex_col()
            .width(state.sidebar_width.get())
            .height_full()
            .border_right(1.0)
            .border_color(theme::border());
        if state.sidebar_open.get() {
            s
        } else {
            s.hide()
        }
    });

    // Keep the Source Control panel in sync with filesystem changes.
    create_effect(move |_| {
        state.fs_rev.get();
        if state.git_panel_open.get_untracked() {
            state.refresh_git_status();
        }
    });

    // Panel sides are configurable (config keys `sidebar_side` / `agent_side`):
    // the explorer/Git sidebar and the agent panel can each sit left or right.
    let sidebar_right = state.settings.get_untracked().sidebar_right;
    let agent_left = state.settings.get_untracked().agent_left;
    let sidebar_handle_side = if sidebar_right { ResizeSide::Right } else { ResizeSide::Left };
    let agent_handle_side = if agent_left { ResizeSide::Left } else { ResizeSide::Right };

    let sidebar = sidebar.into_any();
    let agent = agent_panel(state).into_any();
    let editor = editor_column.into_any();

    let mut left: Vec<floem::AnyView> = Vec::new();
    let mut right: Vec<floem::AnyView> = Vec::new();
    if sidebar_right {
        right.push(sidebar);
    } else {
        left.push(sidebar);
    }
    if agent_left {
        left.push(agent);
    } else {
        right.push(agent);
    }

    let mut cols: Vec<floem::AnyView> = Vec::new();
    cols.extend(left);
    cols.push(editor);
    cols.extend(right);
    cols.push(
        resize_handle(state, sidebar_handle_side, state.sidebar_width, state.sidebar_open, 150.0, 600.0)
            .into_any(),
    );
    cols.push(
        resize_handle(state, agent_handle_side, state.agent_width, state.agent_open, 300.0, 900.0)
            .into_any(),
    );

    let main_row = floem::views::stack_from_iter(cols).style(|s| s.flex_row().size_full());

    stack((
        main_row,
        markdown_preview(state),
        diff_view(state),
        find_bar(state),
        rename_bar(state),
        file_op_prompt(state),
        term_rename_prompt(state),
        about_dialog(state),
        signature_popup(state),
        completion_popup(state),
        hover_popup(state),
        picker_overlay(state),
        palette(state),
        command_palette(state),
        update_notice(state),
        stack((
            goto_bar(state),
            close_confirm_dialog(state),
            recent_palette(state),
            task_palette(state),
            settings_view(state),
        ))
        .style(move |s| {
            let s = s.absolute().inset(0.0).size_full();
            if state.goto.open.get()
                || state.close_confirm.get().is_some()
                || state.recent.open.get()
                || state.task.open.get()
                || state.settings_open.get()
            {
                s
            } else {
                s.hide()
            }
        }),
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
    .on_event_stop(EventListener::DroppedFile, move |e| {
        if let Event::DroppedFile(ev) = e {
            if ev.path.is_dir() {
                state.open_project(ev.path.clone());
            } else {
                state.open_path(ev.path.clone());
            }
        }
    })
}
