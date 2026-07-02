//! Left-hand file explorer: an expandable, flattened directory tree.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use e_core::language::Language;
use floem::menu::{Menu, MenuItem};
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::views::{dyn_stack, label, scroll, stack, Decorators};
use floem::IntoView;

use crate::file_ops::FileOpKind;
use crate::state::AppState;
use crate::theme;

/// A small glyph representing a file/folder type in the explorer.
fn file_glyph(name: &str, is_dir: bool, expanded: bool) -> &'static str {
    if is_dir {
        return if expanded { "📂" } else { "📁" };
    }
    let lower = name.to_ascii_lowercase();

    // Well-known file names.
    match lower.as_str() {
        "cargo.toml" | "cargo.lock" | "package.json" | "package-lock.json" | "composer.json"
        | "composer.lock" | "yarn.lock" | "pnpm-lock.yaml" => return "📦",
        "dockerfile" | ".dockerignore" => return "🐳",
        ".env" | ".env.example" | ".env.local" => return "🔑",
        ".gitignore" | ".gitattributes" => return "🔧",
        "license" | "license.md" => return "📜",
        "readme.md" | "readme" => return "📖",
        "makefile" => return "🛠",
        _ => {}
    }

    let ext = Path::new(&lower)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    match ext {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "ico" | "bmp" | "svg" => return "🖼",
        "lock" => return "🔒",
        "pdf" => return "📕",
        "zip" | "tar" | "gz" | "tgz" | "rar" | "7z" => return "🗜",
        "sql" | "db" | "sqlite" => return "🗄",
        "sh" | "bash" | "zsh" | "fish" => return "🐚",
        "yml" | "yaml" => return "⚙️",
        _ => {}
    }

    match Language::from_path(Path::new(name)) {
        Language::Rust => "🦀",
        Language::Php => "🐘",
        Language::Blade => "🍃",
        Language::JavaScript => "🟨",
        Language::TypeScript => "🟦",
        Language::Vue => "💚",
        Language::Svelte => "🧡",
        Language::Python => "🐍",
        Language::Go => "🐹",
        Language::C | Language::Cpp => "🔧",
        Language::Json => "🗂",
        Language::Css => "🎨",
        Language::Html => "🌐",
        Language::Markdown => "📝",
        Language::Toml => "⚙️",
        Language::Shell => "🐚",
        _ => "📄",
    }
}

/// Context menu for a tree item.
fn item_menu(state: AppState, path: PathBuf) -> Menu {
    let p = || path.clone();
    let is_root = state
        .roots
        .with_untracked(|r| r.contains(&path) && r.len() > 1);

    let mut menu = Menu::new("").entry(
        MenuItem::new("Add Folder to Workspace…").action(move || state.add_workspace_folder()),
    );
    if is_root {
        let rp = path.clone();
        menu = menu.entry(
            MenuItem::new("Remove Folder from Workspace")
                .action(move || state.remove_workspace_folder(rp.clone())),
        );
    }
    menu.separator()
        .entry(MenuItem::new("New File").action({
            let p = p();
            move || state.start_file_op(FileOpKind::NewFile, p.clone())
        }))
        .entry(MenuItem::new("New Folder").action({
            let p = p();
            move || state.start_file_op(FileOpKind::NewFolder, p.clone())
        }))
        .separator()
        .entry(MenuItem::new("Rename").action({
            let p = p();
            move || state.start_file_op(FileOpKind::Rename, p.clone())
        }))
        .entry(MenuItem::new("Duplicate").action({
            let p = p();
            move || state.start_file_op(FileOpKind::Duplicate, p.clone())
        }))
        .entry(MenuItem::new("Delete").action({
            let p = p();
            move || state.delete_path(p.clone())
        }))
        .separator()
        .entry(MenuItem::new("Copy Path").action({
            let p = p();
            move || state.copy_path_to_clipboard(&p)
        }))
        .entry(MenuItem::new("Reveal in Finder").action({
            let p = p();
            move || state.reveal_in_finder(&p)
        }))
}

#[derive(Clone)]
struct Row {
    path: PathBuf,
    name: String,
    is_dir: bool,
    depth: usize,
    expanded: bool,
}

/// Walk the workspace from `root`, descending only into expanded directories,
/// and produce a flat, ordered list of visible rows.
fn flatten(roots: &[PathBuf], expanded: &HashSet<PathBuf>) -> Vec<Row> {
    let mut out = Vec::new();
    let multi = roots.len() > 1;
    for root in roots {
        if multi {
            // Each root is a collapsible top-level folder (expanded by default).
            let name = root
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| root.to_string_lossy().into_owned());
            let is_expanded = !expanded.contains(root);
            out.push(Row {
                path: root.clone(),
                name,
                is_dir: true,
                depth: 0,
                expanded: is_expanded,
            });
            if is_expanded {
                walk(root, 1, expanded, &mut out);
            }
        } else {
            walk(root, 0, expanded, &mut out);
        }
    }
    out
}

fn walk(dir: &PathBuf, depth: usize, expanded: &HashSet<PathBuf>, out: &mut Vec<Row>) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<(PathBuf, bool, String)> = read
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let path = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            // Show every project file, including dotfiles like `.env` and
            // `.gitignore`. Only heavy VCS/build/dependency directories are
            // skipped to keep the tree usable.
            if name == ".git" || name == "target" || name == "node_modules" {
                return None;
            }
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            Some((path, is_dir, name))
        })
        .collect();

    // Directories first, then alphabetical.
    entries.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| a.2.to_lowercase().cmp(&b.2.to_lowercase()))
    });

    for (path, is_dir, name) in entries {
        let is_expanded = expanded.contains(&path);
        out.push(Row {
            path: path.clone(),
            name,
            is_dir,
            depth,
            expanded: is_expanded,
        });
        if is_dir && is_expanded {
            walk(&path, depth + 1, expanded, out);
        }
    }
}

pub fn file_tree(state: AppState) -> impl IntoView {
    let expanded: RwSignal<HashSet<PathBuf>> = RwSignal::new(HashSet::new());

    let root_name = state
        .root
        .with(|r| r.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "workspace".to_string());

    let header = label(move || root_name.to_uppercase()).style(|s| {
        s.height(30.0)
            .width_full()
            .items_center()
            .padding_horiz(12.0)
            .color(theme::fg_dim())
            .font_size(11.0)
    });

    let rows = dyn_stack(
        move || {
            state.fs_rev.get(); // refresh after filesystem operations
            flatten(&state.roots.get(), &expanded.get())
        },
        |r| r.path.clone(),
        move |r| {
            let path = r.path.clone();
            let menu_path = r.path.clone();
            let is_dir = r.is_dir;
            let indent = 8.0 + r.depth as f64 * 14.0;

            let glyph = file_glyph(&r.name, is_dir, r.expanded);

            stack((
                label(move || glyph.to_string())
                    .style(|s| s.width(20.0).font_size(13.0).justify_center()),
                label(move || r.name.clone()).style(|s| s.text_ellipsis().color(theme::fg())),
            ))
            .style(move |s| {
                s.items_center()
                    .gap(2.0)
                    .height(24.0)
                    .width_full()
                    .padding_left(indent)
                    .padding_right(8.0)
                    .cursor(floem::style::CursorStyle::Pointer)
                    .hover(|s| s.background(theme::bg_hover()))
            })
            .on_click_stop(move |_| {
                if is_dir {
                    expanded.update(|set| {
                        if !set.remove(&path) {
                            set.insert(path.clone());
                        }
                    });
                } else {
                    state.open_path(path.clone());
                }
            })
            .context_menu(move || item_menu(state, menu_path.clone()))
        },
    )
    .style(|s| s.flex_col().width_full());

    stack((
        header,
        scroll(rows).style(|s| s.flex_grow(1.0).width_full()),
    ))
    .style(|s| {
        s.flex_col()
            .width_full()
            .flex_grow(1.0)
            .min_height(0.0)
            .background(theme::bg_panel())
    })
    // Right-click on empty space: create items at the workspace root.
    .context_menu(move || {
        let root = state.root.get_untracked();
        Menu::new("")
            .entry(MenuItem::new("New File").action({
                let root = root.clone();
                move || state.start_file_op(FileOpKind::NewFile, root.clone())
            }))
            .entry(
                MenuItem::new("New Folder")
                    .action(move || state.start_file_op(FileOpKind::NewFolder, root.clone())),
            )
    })
}
