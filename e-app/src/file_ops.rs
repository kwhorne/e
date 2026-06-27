//! File-explorer operations: a name prompt (new file/folder, rename,
//! duplicate) and the supporting filesystem helpers.

use std::path::{Path, PathBuf};

use floem::keyboard::{Key, NamedKey};
use floem::reactive::{RwSignal, SignalGet, SignalUpdate};
use floem::views::{container, label, stack, text_input, Decorators};
use floem::IntoView;

use crate::state::AppState;
use crate::theme;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FileOpKind {
    NewFile,
    NewFolder,
    Rename,
    Duplicate,
}

impl FileOpKind {
    pub fn title(self) -> &'static str {
        match self {
            FileOpKind::NewFile => "New file name:",
            FileOpKind::NewFolder => "New folder name:",
            FileOpKind::Rename => "Rename to:",
            FileOpKind::Duplicate => "Duplicate as:",
        }
    }
}

/// Reactive state for the name-prompt modal.
#[derive(Clone, Copy)]
pub struct FileOp {
    pub open: RwSignal<bool>,
    pub kind: RwSignal<FileOpKind>,
    /// Base directory (for new) or the target path (rename/duplicate).
    pub base: RwSignal<PathBuf>,
    pub input: RwSignal<String>,
}

impl FileOp {
    pub fn new() -> Self {
        Self {
            open: RwSignal::new(false),
            kind: RwSignal::new(FileOpKind::NewFile),
            base: RwSignal::new(PathBuf::new()),
            input: RwSignal::new(String::new()),
        }
    }
}

/// A default "duplicate" name: `foo.rs` -> `foo copy.rs`.
pub fn duplicate_name(path: &Path) -> String {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("copy");
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => format!("{stem} copy.{ext}"),
        None => format!("{stem} copy"),
    }
}

/// Recursively copy a file or directory.
pub fn copy_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    if src.is_dir() {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &dst.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(src, dst).map(|_| ())
    }
}

/// The name-prompt modal overlay.
pub fn file_op_prompt(state: AppState) -> impl IntoView {
    let op = state.file_op;

    let title =
        label(move || op.kind.get().title()).style(|s| s.color(theme::fg_dim()).font_size(12.0));

    let input = text_input(op.input)
        .on_enter(move || state.confirm_file_op())
        .style(|s| {
            theme::input_colors(s)
                .width(320.0)
                .height(30.0)
                .padding_horiz(8.0)
                .border(1.0)
                .border_radius(4.0)
        })
        .request_focus(move || {
            op.open.get();
        })
        .on_key_down(
            Key::Named(NamedKey::Escape),
            |_| true,
            move |_| op.open.set(false),
        );

    let box_ = stack((title, input)).style(|s| {
        s.flex_col()
            .gap(8.0)
            .padding(14.0)
            .background(theme::bg_panel())
            .border(1.0)
            .border_color(theme::border())
            .border_radius(8.0)
    });

    container(box_)
        .style(move |s| {
            let s = s
                .absolute()
                .inset(0.0)
                .size_full()
                .justify_center()
                .padding_top(120.0);
            if op.open.get() {
                s
            } else {
                s.hide()
            }
        })
        .on_click_stop(move |_| op.open.set(false))
}

#[cfg(test)]
mod tests {
    use super::duplicate_name;
    use std::path::Path;
    #[test]
    fn duplicate_naming() {
        assert_eq!(duplicate_name(Path::new("/a/foo.rs")), "foo copy.rs");
        assert_eq!(duplicate_name(Path::new("/a/Makefile")), "Makefile copy");
    }
}
