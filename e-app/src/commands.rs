//! Central command dispatch. Every editor action is a named command, invoked
//! both from keybindings ([`crate::keymap`]) and the command palette. This is
//! the single place that maps a command id to its effect.

use floem::reactive::{SignalGet, SignalUpdate};

use crate::state::AppState;
use crate::theme;

/// Run the command with id `id`. Returns `true` if it was a known command.
pub fn dispatch(state: AppState, id: &str) -> bool {
    match id {
        // Navigation & palettes.
        "goto-file" => state.palette_open.update(|o| *o = !*o),
        "command-palette" => state.cmd.open.set(true),
        "recent" => state.open_recent(),
        "open-folder" => state.open_project_dialog(),
        "open-file" => state.open_file_dialog(),
        "add-folder" => state.add_workspace_folder(),
        "laravel-refresh" => state.load_laravel(),
        "symbols" => state.open_symbol_search(),
        "search" => state.open_global_search(),
        "find" => state.open_find(),
        "replace" => state.open_replace(),
        "goto-line" => state.open_goto_line(),
        "nav-back" => state.nav_back(),
        "nav-forward" => state.nav_forward(),

        // Editing.
        "new-file" => state.new_untitled(),
        "save" => state.save_active(),
        "save-as" => state.save_active_as(),
        "format" => state.format_active(),
        "rename" => state.open_rename(),
        "comment" => state.toggle_comment(),
        "move-line-up" => state.move_line_up(),
        "move-line-down" => state.move_line_down(),
        "duplicate-line" => state.duplicate_line(),
        "delete-line" => state.delete_line(),
        "indent" => state.indent_lines(),
        "outdent" => state.outdent_lines(),
        "select-next-occurrence" => state.select_next_occurrence(),
        "select-all-occurrences" => state.select_all_occurrences(),
        "completion" => {
            if let Some(id) = state.focused_active_id() {
                state.request_completion(id);
            }
        }

        // Language features.
        "hover" => state.request_hover(),
        "definition" => state.goto_definition(),
        "references" => state.request_references(),

        // Tasks & tests.
        "run-task" => state.open_task_palette(),
        "run-test" => state.run_test(),

        // Git.
        "source-control" => state.toggle_git_panel(),
        "diff" => state.toggle_diff(),

        // Panels & view.
        "toggle-sidebar" => state.sidebar_open.update(|o| *o = !*o),
        "split" => state.toggle_split(),
        "terminal" | "toggle-terminal" => state.toggle_terminal(),
        "new-terminal" => state.new_terminal(),
        "split-terminal" => state.split_terminal(),
        "agent" | "toggle-agent" => state.toggle_agent(),
        "toggle-database" => state.toggle_db_panel(),
        "emmet-expand" => {
            state.try_emmet_expand();
        }
        "restart-agent" => state.restart_agent(),
        "markdown" => state.toggle_md_preview(),
        "theme" => theme::toggle(),
        "zoom-in" => state.zoom(1),
        "zoom-out" => state.zoom(-1),
        "zoom-reset" => state.zoom_reset(),
        "word-wrap" => state.toggle_word_wrap(),

        // App.
        "settings" => state.settings_open.set(true),
        "settings-json" => state.open_settings(),
        "about" => state.about_open.set(true),
        "check-updates" => state.check_for_updates(true),
        "install-cli" => state.install_cli(),

        // Closing.
        "close" => close_focused(state),
        "close-tab" => {
            if let Some(id) = state.focused_active_id() {
                state.close(id);
            }
        }
        "close-overlays" => close_overlays(state),

        _ => return false,
    }
    true
}

/// Context-aware close (⌘W): agent panel, then terminal, then the active tab.
fn close_focused(state: AppState) {
    // The database results overlay (and its edit popup) take priority.
    if state.db_edit.get().is_some() {
        state.db_cancel_edit();
    } else if state.db_result_open.get() {
        state.close_db_result();
    } else if state.agent_focused.get() {
        state.agent_open.set(false);
    } else if state.terminal_focused.get() {
        if let Some(id) = state.focused_term_id() {
            state.close_terminal(id);
        }
    } else if let Some(id) = state.focused_active_id() {
        state.close(id);
    }
}

/// Dismiss every open overlay (Escape).
fn close_overlays(state: AppState) {
    if state.db_edit.get().is_some() {
        state.db_cancel_edit();
        return;
    }
    state.close_db_result();
    state.close_completion();
    state.close_hover();
    state.close_signature();
    state.picker.open.set(false);
    state.palette_open.set(false);
    state.cmd.open.set(false);
    state.md_preview.set(false);
    state.diff_open.set(false);
    state.about_open.set(false);
    state.close_find();
    state.close_rename();
    state.close_goto_line();
    state.cancel_close();
    state.close_recent();
    state.task.open.set(false);
    state.settings_open.set(false);
}
