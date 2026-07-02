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
        "tinker" => state.toggle_tinker(),
        "tinker-selection" => state.run_tinker_selection(),
        "laravel-map" => state.toggle_laravel_map(),
        "agent-log" => state.toggle_agent_log(),
        "run-tests" => state.toggle_tdd(),
        "laravel-log" => state.toggle_laravel_log(),
        "runtime" => state.toggle_runtime(),
        "schema-diff" => state.compute_schema_diff(),
        "relations" => state.toggle_relations(),
        "props-contract" => state.compute_contract(),
        "related-files" => state.show_related_files(),
        "generate-rules" => state.generate_validation_rules(),
        "livewire-companion" => state.livewire_companion(),
        "undo-tree" => state.toggle_undo_tree(),
        "semantic-search" => state.toggle_semantic_search(),
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
    if state.agent_edit.get().is_some() {
        state.agent_edit_cancel();
        return;
    }
    if state.agent_log_open.get() {
        state.agent_log_open.set(false);
        return;
    }
    if state.tdd_open.get() {
        state.tdd_open.set(false);
        return;
    }
    if state.req_open.get() {
        state.close_request();
        return;
    }
    if state.log_open.get() {
        state.log_open.set(false);
        return;
    }
    if state.runtime_open.get() {
        state.runtime_open.set(false);
        return;
    }
    if state.schema_diff_open.get() {
        state.schema_diff_open.set(false);
        return;
    }
    if state.rel_open.get() {
        state.rel_open.set(false);
        return;
    }
    if state.contract_open.get() {
        state.contract_open.set(false);
        return;
    }
    if state.related_open.get() {
        state.related_open.set(false);
        return;
    }
    if state.undo_open.get() {
        state.undo_open.set(false);
        return;
    }
    if state.sem_open.get() {
        state.sem_open.set(false);
        return;
    }
    if state.db_edit.get().is_some() {
        state.db_cancel_edit();
        return;
    }
    if state.tinker_open.get() {
        state.tinker_open.set(false);
        return;
    }
    if state.map_open.get() {
        state.map_open.set(false);
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
