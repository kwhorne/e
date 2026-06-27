//! `e-app` — the GUI layer of the `e` editor, built on Floem.
//!
//! Milepæl 0: open a single file passed on the command line (or a scratch
//! buffer), edit it, and save with Cmd/Ctrl+S. A status bar shows the file
//! name, language and dirty state. Everything else grows from here.

mod editing;
mod dialogs;
mod recent;
mod git_view;
mod builtin_completion;
mod framework_completion;
mod hints_doc;
mod about;
mod update_view;
mod updater;
mod agent_view;
mod app;
mod breadcrumbs;
mod cmd_palette;
mod completion;
mod config;
mod diff_view;
mod editor_area;
mod file_ops;
mod file_tree;
mod find;
mod laravel;
mod markdown_view;
mod outline;
mod palette;
mod picker;
mod problems;
mod rename;
mod session;
mod snippets;
mod state;
mod status;
mod styling;
mod tabs;
mod terminal_view;
mod theme;

pub use app::launch;
