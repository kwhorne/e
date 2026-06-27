//! Recent-files quick switcher (⌘E): a most-recently-used list of the files
//! opened this session, newest first.

use std::path::PathBuf;

use floem::keyboard::{Key, NamedKey};
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::views::{container, dyn_stack, label, scroll, stack, text_input, Decorators};
use floem::IntoView;

use crate::state::AppState;
use crate::theme;

const MAX_RECENT: usize = 50;
const MAX_SHOWN: usize = 10;

#[derive(Clone, Copy)]
pub struct RecentState {
    pub open: RwSignal<bool>,
    pub query: RwSignal<String>,
    pub selected: RwSignal<usize>,
    pub focus_pulse: RwSignal<u64>,
}

impl RecentState {
    pub fn new() -> Self {
        Self {
            open: RwSignal::new(false),
            query: RwSignal::new(String::new()),
            selected: RwSignal::new(0),
            focus_pulse: RwSignal::new(0),
        }
    }
}

impl AppState {
    /// Move `path` to the front of the recent-files list (deduplicated, capped).
    pub fn push_recent(&self, path: PathBuf) {
        self.recent_files.update(|list| {
            list.retain(|p| p != &path);
            list.insert(0, path);
            list.truncate(MAX_RECENT);
        });
    }

    /// The recent files to show, newest first, filtered by the query.
    pub fn recent_paths(&self) -> Vec<PathBuf> {
        let q = self.recent.query.get().to_lowercase();
        self.recent_files
            .get()
            .into_iter()
            .filter(|p| q.is_empty() || p.to_string_lossy().to_lowercase().contains(&q))
            .take(MAX_SHOWN)
            .collect()
    }

    pub fn open_recent(&self) {
        if self.recent_files.with_untracked(|l| l.is_empty()) {
            return;
        }
        self.recent.query.set(String::new());
        // Preselect the previous file (index 1) when there is one, like a
        // most-recently-used switcher; otherwise the only/most-recent file.
        let len = self.recent_files.with_untracked(|l| l.len());
        self.recent.selected.set(if len > 1 { 1 } else { 0 });
        self.recent.focus_pulse.update(|x| *x += 1);
        self.recent.open.set(true);
    }

    pub fn close_recent(&self) {
        self.recent.open.set(false);
    }
}

fn display_for(path: &PathBuf, root: &std::path::Path) -> (String, String) {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let dir = path
        .strip_prefix(root)
        .unwrap_or(path)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    (name, dir)
}

pub fn recent_palette(state: AppState) -> impl IntoView {
    let recent = state.recent;

    let open_selected = move || {
        let results = state.recent_paths();
        if results.is_empty() {
            return;
        }
        let idx = recent.selected.get_untracked().min(results.len() - 1);
        state.open_path(results[idx].clone());
        recent.open.set(false);
    };

    let input = text_input(recent.query)
        .placeholder("Recent files…")
        .on_enter(open_selected)
        .style(|s| {
            theme::input_colors(s)
                .width_full()
                .height(36.0)
                .padding_horiz(10.0)
                .border(0.0)
                .border_bottom(1.0)
        })
        .request_focus(move || {
            recent.focus_pulse.get();
        })
        .on_event_stop(floem::event::EventListener::FocusLost, move |_| {
            if recent.open.get_untracked() {
                recent.open.set(false);
            }
        })
        .on_key_down(Key::Named(NamedKey::Escape), |_| true, move |_| recent.open.set(false))
        .on_key_down(Key::Named(NamedKey::ArrowDown), |_| true, move |_| {
            let len = state.recent_paths().len();
            if len > 0 {
                recent.selected.update(|i| *i = (*i + 1).min(len - 1));
            }
        })
        .on_key_down(Key::Named(NamedKey::ArrowUp), |_| true, move |_| {
            recent.selected.update(|i| *i = i.saturating_sub(1));
        });

    let results = dyn_stack(
        move || state.recent_paths().into_iter().enumerate().collect::<Vec<_>>(),
        |(i, p)| (*i, p.clone()),
        move |(i, path)| {
            let root = state.root.get();
            let (name, dir) = display_for(&path, &root);
            stack((
                label(move || name.clone()).style(|s| s.color(theme::fg())),
                label(move || dir.clone())
                    .style(|s| s.color(theme::fg_dim()).font_size(12.0).text_ellipsis()),
            ))
            .style(move |s| {
                let s = s
                    .height(26.0)
                    .width_full()
                    .items_center()
                    .gap(8.0)
                    .padding_horiz(10.0)
                    .cursor(floem::style::CursorStyle::Pointer);
                if recent.selected.get() == i {
                    s.background(theme::bg_active())
                } else {
                    s.hover(|s| s.background(theme::bg_hover()))
                }
            })
            .on_click_stop(move |_| {
                recent.selected.set(i);
                state.open_path(path.clone());
                recent.open.set(false);
            })
        },
    )
    .style(|s| s.flex_col().width_full());

    let results_scroll = scroll(results)
        .scroll_to_percent(move || {
            let n = state.recent_paths().len().max(1) as f32;
            recent.selected.get() as f32 / n
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

    container(box_)
        .style(move |s| {
            let s = s.absolute().inset(0.0).size_full().justify_center().padding_top(90.0);
            if recent.open.get() {
                s
            } else {
                s.hide()
            }
        })
        .on_click_stop(move |_| recent.open.set(false))
}
