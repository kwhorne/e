//! Command palette / fuzzy file finder (⌘P or Ctrl+P).

use std::path::{Path, PathBuf};

use floem::keyboard::{Key, NamedKey};
use floem::reactive::{create_effect, RwSignal, SignalGet, SignalUpdate};
use floem::views::{container, dyn_stack, label, scroll, stack, text_input, Decorators};
use floem::IntoView;

use crate::state::AppState;
use crate::theme;

const MAX_FILES: usize = 5000;
const MAX_RESULTS: usize = 200;

/// Recursively collect files under `root`, skipping noise.
fn collect_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if out.len() >= MAX_FILES {
            break;
        }
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in read.filter_map(|e| e.ok()) {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || name == "target" || name == "node_modules" {
                continue;
            }
            let path = entry.path();
            match entry.file_type() {
                Ok(t) if t.is_dir() => stack.push(path),
                Ok(_) => out.push(path),
                Err(_) => {}
            }
        }
    }
    out.sort();
    out
}

fn rel(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

pub fn palette(state: AppState) -> impl IntoView {
    let query: RwSignal<String> = RwSignal::new(String::new());
    let files: RwSignal<Vec<PathBuf>> = RwSignal::new(Vec::new());
    let selected: RwSignal<usize> = RwSignal::new(0);

    // (Re)load the file list whenever the palette opens.
    create_effect(move |_| {
        if state.palette_open.get() {
            files.set(collect_files(&state.root.get()));
            query.set(String::new());
            selected.set(0);
        }
    });

    // Filtered results, shared by the list view and the keyboard handlers.
    let filtered = move || -> Vec<PathBuf> {
        let q = query.get().to_lowercase();
        let root = state.root.get();
        files
            .get()
            .into_iter()
            .filter(|p| q.is_empty() || rel(p, &root).to_lowercase().contains(&q))
            .take(MAX_RESULTS)
            .collect()
    };

    let open_selected = move || {
        let results = filtered();
        if results.is_empty() {
            return;
        }
        let idx = selected.get().min(results.len() - 1);
        state.open_path(results[idx].clone());
        state.palette_open.set(false);
    };

    let input = text_input(query)
        .placeholder("Go to file…")
        .style(|s| {
            s.width_full()
                .height(36.0)
                .padding_horiz(10.0)
                .background(theme::bg())
                .color(theme::fg())
                .border_bottom(1.0)
                .border_color(theme::border())
        })
        .request_focus(move || {
            // Re-focus the input each time the palette is toggled open.
            state.palette_open.get();
        })
        .on_key_down(Key::Named(NamedKey::Escape), |_| true, move |_| {
            state.palette_open.set(false)
        })
        .on_key_down(Key::Named(NamedKey::Enter), |_| true, move |_| open_selected())
        .on_key_down(Key::Named(NamedKey::ArrowDown), |_| true, move |_| {
            let len = filtered().len();
            if len > 0 {
                selected.update(|i| *i = (*i + 1).min(len - 1));
            }
        })
        .on_key_down(Key::Named(NamedKey::ArrowUp), |_| true, move |_| {
            selected.update(|i| *i = i.saturating_sub(1));
        });

    let results = dyn_stack(
        move || filtered().into_iter().enumerate().collect::<Vec<_>>(),
        |(i, p)| (*i, p.clone()),
        move |(i, path)| {
            let root = state.root.get();
            let text = rel(&path, &root);
            label(move || text.clone())
                .style(move |s| {
                    let s = s
                        .height(26.0)
                        .width_full()
                        .items_center()
                        .padding_horiz(10.0)
                        .text_ellipsis()
                        .cursor(floem::style::CursorStyle::Pointer)
                        .color(theme::fg());
                    if selected.get() == i {
                        s.background(theme::bg_active()).color(theme::accent())
                    } else {
                        s.hover(|s| s.background(theme::bg_hover()))
                    }
                })
                .on_click_stop(move |_| {
                    selected.set(i);
                    state.open_path(path.clone());
                    state.palette_open.set(false);
                })
        },
    )
    .style(|s| s.flex_col().width_full());

    let results_scroll = scroll(results)
        .scroll_to_percent(move || {
            let n = filtered().len().max(1) as f32;
            selected.get() as f32 / n
        })
        .style(|s| s.max_height(320.0).width_full());

    let box_ = stack((input, results_scroll))
        .style(|s| {
            s.flex_col()
                .width(560.0)
                .background(theme::bg_panel())
                .border(1.0)
                .border_color(theme::border())
                .border_radius(8.0)
        })
        .on_click_stop(|_| {});

    // Backdrop fills the window; clicking it closes the palette.
    container(box_)
        .style(move |s| {
            let s = s
                .absolute()
                .inset(0.0)
                .width_full()
                .height_full()
                .justify_center()
                .padding_top(90.0);
            if state.palette_open.get() {
                s
            } else {
                s.hide()
            }
        })
        .on_click_stop(move |_| state.palette_open.set(false))
}
