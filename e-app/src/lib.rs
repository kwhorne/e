//! `e-app` — the GUI layer of the `e` editor, built on Floem.
//!
//! Milepæl 0: open a single file passed on the command line (or a scratch
//! buffer), edit it, and save with Cmd/Ctrl+S. A status bar shows the file
//! name, language and dirty state. Everything else grows from here.

mod about;
mod agent_sync;
mod agent_ui;
mod agent_view;
mod app;
mod breadcrumbs;
mod builtin_completion;
mod cmd_palette;
mod commands;
mod completion;
mod config;
mod db_view;
mod dialogs;
mod diff_view;
mod editing;
mod editor_area;
mod eloquent;
mod emmet;
mod file_ops;
mod file_tree;
mod find;
mod framework_completion;
mod git_view;
mod hints_doc;
mod keymap;
mod laravel;
mod log;
mod map;
mod markdown_view;
mod outline;
mod palette;
mod picker;
mod problems;
mod recent;
mod relations;
mod relations_view;
mod rename;
mod request;
mod schema_diff;
mod semantic;
mod semantic_view;
mod session;
mod settings_view;
mod snippets;
mod state;
mod status;
mod styling;
mod tabs;
mod task_palette;
mod tasks;
mod tdd;
mod terminal_view;
mod theme;
mod tinker;
mod undo_view;
mod update_view;
mod updater;

pub use app::launch;
