//! `e-app` — the GUI layer of the `e` editor, built on Floem.
//!
//! Milepæl 0: open a single file passed on the command line (or a scratch
//! buffer), edit it, and save with Cmd/Ctrl+S. A status bar shows the file
//! name, language and dirty state. Everything else grows from here.

mod app;
mod editor_area;
mod file_tree;
mod palette;
mod problems;
mod state;
mod status;
mod styling;
mod tabs;
mod theme;

pub use app::launch;
