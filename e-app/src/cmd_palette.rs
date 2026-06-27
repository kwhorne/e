//! Command palette (⌘⇧P): run editor commands by name.

use floem::keyboard::{Key, NamedKey};
use floem::reactive::{RwSignal, SignalGet, SignalUpdate};
use floem::views::{container, dyn_stack, label, scroll, stack, text_input, Decorators};
use floem::IntoView;

use crate::state::AppState;
use crate::theme;

/// `(id, label)` for every command. `id` is matched in [`run_command`].
const COMMANDS: &[(&str, &str)] = &[
    ("goto-file", "Go to File…"),
    ("symbols", "Go to Symbol…"),
    ("search", "Search in Files…"),
    ("find", "Find in File"),
    ("save", "Save File"),
    ("format", "Format Document"),
    ("rename", "Rename Symbol"),
    ("definition", "Go to Definition"),
    ("references", "Find References"),
    ("markdown", "Toggle Markdown Preview"),
    ("diff", "Show Git Diff vs HEAD"),
    ("split", "Toggle Split View"),
    ("terminal", "Toggle Terminal"),
    ("theme", "Toggle Light/Dark Theme"),
    ("settings", "Open Settings (config.json)"),
    ("close-tab", "Close Tab"),
];

#[derive(Clone, Copy)]
pub struct CmdPalette {
    pub open: RwSignal<bool>,
    pub query: RwSignal<String>,
    pub selected: RwSignal<usize>,
}

impl CmdPalette {
    pub fn new() -> Self {
        Self {
            open: RwSignal::new(false),
            query: RwSignal::new(String::new()),
            selected: RwSignal::new(0),
        }
    }
}

pub fn run_command(state: AppState, id: &str) {
    match id {
        "goto-file" => state.palette_open.set(true),
        "symbols" => state.open_symbol_search(),
        "search" => state.open_global_search(),
        "find" => state.open_find(),
        "save" => state.save_active(),
        "format" => state.format_active(),
        "rename" => state.open_rename(),
        "definition" => state.goto_definition(),
        "references" => state.request_references(),
        "markdown" => state.toggle_md_preview(),
        "diff" => state.toggle_diff(),
        "split" => state.toggle_split(),
        "terminal" => state.toggle_terminal(),
        "theme" => theme::toggle(),
        "settings" => {
            if let Some(home) = std::env::var_os("HOME") {
                let path = std::path::PathBuf::from(home)
                    .join(".config")
                    .join("e")
                    .join("config.json");
                state.open_path(path);
            }
        }
        "close-tab" => {
            if let Some(id) = state.focused_active_id() {
                state.close(id);
            }
        }
        _ => {}
    }
}

pub fn command_palette(state: AppState) -> impl IntoView {
    let cmd = state.cmd;

    let filtered = move || -> Vec<(&'static str, &'static str)> {
        let q = cmd.query.get().to_lowercase();
        COMMANDS
            .iter()
            .filter(|(_, label)| q.is_empty() || label.to_lowercase().contains(&q))
            .copied()
            .collect()
    };

    let run_selected = move || {
        let results = filtered();
        if results.is_empty() {
            return;
        }
        let idx = cmd.selected.get().min(results.len() - 1);
        cmd.open.set(false);
        run_command(state, results[idx].0);
    };

    let input = text_input(cmd.query)
        .placeholder("Run a command…")
        .style(|s| {
            theme::input_colors(s)
                .width_full()
                .height(36.0)
                .padding_horiz(10.0)
                .border(0.0)
                .border_bottom(1.0)
        })
        .request_focus(move || {
            cmd.open.get();
        })
        .on_key_down(Key::Named(NamedKey::Escape), |_| true, move |_| cmd.open.set(false))
        .on_key_down(Key::Named(NamedKey::Enter), |_| true, move |_| run_selected())
        .on_key_down(Key::Named(NamedKey::ArrowDown), |_| true, move |_| {
            let len = filtered().len();
            if len > 0 {
                cmd.selected.update(|i| *i = (*i + 1).min(len - 1));
            }
        })
        .on_key_down(Key::Named(NamedKey::ArrowUp), |_| true, move |_| {
            cmd.selected.update(|i| *i = i.saturating_sub(1));
        });

    let rows = dyn_stack(
        move || filtered().into_iter().enumerate().collect::<Vec<_>>(),
        |(i, _)| *i,
        move |(i, (id, lbl))| {
            label(move || lbl.to_string())
                .style(move |s| {
                    let s = s
                        .height(28.0)
                        .width_full()
                        .items_center()
                        .padding_horiz(12.0)
                        .color(theme::fg())
                        .cursor(floem::style::CursorStyle::Pointer);
                    if cmd.selected.get() == i {
                        s.background(theme::bg_active())
                    } else {
                        s.hover(|s| s.background(theme::bg_hover()))
                    }
                })
                .on_click_stop(move |_| {
                    cmd.open.set(false);
                    run_command(state, id);
                })
        },
    )
    .style(|s| s.flex_col().width_full());

    let rows_scroll = scroll(rows)
        .scroll_to_percent(move || {
            let n = filtered().len().max(1) as f32;
            cmd.selected.get() as f32 / n
        })
        .style(|s| s.max_height(360.0).width_full());

    let box_ = stack((input, rows_scroll))
        .style(|s| {
            s.flex_col()
                .width(520.0)
                .background(theme::bg_panel())
                .border(1.0)
                .border_color(theme::border())
                .border_radius(8.0)
        })
        .on_click_stop(|_| {});

    container(box_)
        .style(move |s| {
            let s = s.absolute().inset(0.0).size_full().justify_center().padding_top(90.0);
            if cmd.open.get() {
                s
            } else {
                s.hide()
            }
        })
        .on_click_stop(move |_| cmd.open.set(false))
}
