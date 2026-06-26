//! Left-hand file explorer: an expandable, flattened directory tree.

use std::collections::HashSet;
use std::path::PathBuf;

use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::views::{dyn_stack, label, scroll, stack, Decorators};
use floem::IntoView;

use crate::state::AppState;
use crate::theme;

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
fn flatten(root: &PathBuf, expanded: &HashSet<PathBuf>) -> Vec<Row> {
    let mut out = Vec::new();
    walk(root, 0, expanded, &mut out);
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
            if name.starts_with('.') || name == "target" || name == "node_modules" {
                return None;
            }
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            Some((path, is_dir, name))
        })
        .collect();

    // Directories first, then alphabetical.
    entries.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.2.to_lowercase().cmp(&b.2.to_lowercase())));

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
            .color(theme::FG_DIM)
            .font_size(11.0)
    });

    let rows = dyn_stack(
        move || flatten(&state.root.get(), &expanded.get()),
        |r| r.path.clone(),
        move |r| {
            let path = r.path.clone();
            let is_dir = r.is_dir;
            let indent = 8.0 + r.depth as f64 * 14.0;

            let icon = if is_dir {
                if r.expanded {
                    "▾"
                } else {
                    "▸"
                }
            } else {
                "·"
            };

            stack((
                label(move || icon.to_string()).style(|s| s.width(14.0).color(theme::FG_DIM)),
                label(move || r.name.clone()).style(|s| s.text_ellipsis().color(theme::FG)),
            ))
            .style(move |s| {
                s.items_center()
                    .gap(2.0)
                    .height(24.0)
                    .width_full()
                    .padding_left(indent)
                    .padding_right(8.0)
                    .cursor(floem::style::CursorStyle::Pointer)
                    .hover(|s| s.background(theme::BG_HOVER))
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
        },
    )
    .style(|s| s.flex_col().width_full());

    stack((header, scroll(rows).style(|s| s.flex_grow(1.0).width_full()))).style(|s| {
        s.flex_col()
            .width_full()
            .flex_grow(1.0)
            .min_height(0.0)
            .background(theme::BG_PANEL)
    })
}
