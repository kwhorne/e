//! Command palette (⌘⇧P): run editor commands by name.

use floem::keyboard::{Key, NamedKey};
use floem::reactive::{create_effect, RwSignal, SignalGet, SignalUpdate};
use floem::views::{container, dyn_stack, label, scroll, stack, text_input, Decorators};
use floem::IntoView;

use crate::state::AppState;
use crate::theme;

/// `(id, label)` for every command. `id` is matched in [`run_command`].
const COMMANDS: &[(&str, &str)] = &[
    ("goto-file", "Go to File…"),
    ("open-folder", "Open Folder…"),
    ("new-file", "New File"),
    ("open-file", "Open File…"),
    ("add-folder", "Add Folder to Workspace…"),
    ("laravel-refresh", "Laravel: Refresh Project Data"),
    ("toggle-database", "Toggle Database Panel"),
    ("tinker", "Tinker: Toggle Scratchpad"),
    ("tinker-selection", "Tinker: Run Selection"),
    ("laravel-map", "Laravel: Architecture Map"),
    ("emmet-expand", "Emmet: Expand Abbreviation"),
    ("save-as", "Save As…"),
    ("recent", "Recent Files"),
    ("symbols", "Go to Symbol…"),
    ("search", "Search in Files…"),
    ("find", "Find in File"),
    ("replace", "Replace in File"),
    ("goto-line", "Go to Line…"),
    ("comment", "Toggle Line Comment"),
    ("move-line-up", "Move Line Up"),
    ("move-line-down", "Move Line Down"),
    ("duplicate-line", "Duplicate Line (⌘D)"),
    ("delete-line", "Delete Line"),
    ("save", "Save File"),
    ("format", "Format Document"),
    ("rename", "Rename Symbol"),
    ("definition", "Go to Definition"),
    ("references", "Find References"),
    ("nav-back", "Go Back"),
    ("nav-forward", "Go Forward"),
    ("markdown", "Toggle Markdown Preview"),
    ("diff", "Show Git Diff vs HEAD"),
    ("select-all-occurrences", "Select All Occurrences"),
    ("run-task", "Run Task…"),
    ("run-test", "Run Tests"),
    ("source-control", "Toggle Source Control"),
    ("split", "Toggle Split View"),
    ("terminal", "Toggle Terminal"),
    ("new-terminal", "New Terminal"),
    ("split-terminal", "Split Terminal"),
    ("agent", "Toggle Agent Panel"),
    ("restart-agent", "Restart Agent"),
    ("theme", "Toggle Light/Dark Theme"),
    ("zoom-in", "Zoom In"),
    ("zoom-out", "Zoom Out"),
    ("zoom-reset", "Reset Zoom"),
    ("word-wrap", "Toggle Word Wrap"),
    ("check-updates", "Check for Updates"),
    ("settings", "Settings"),
    ("settings-json", "Open Settings (config.json)"),
    ("install-cli", "Install 'e' Command in PATH"),
    ("about", "About e"),
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
    crate::commands::dispatch(state, id);
}

pub fn command_palette(state: AppState) -> impl IntoView {
    let cmd = state.cmd;

    let focus_pulse: RwSignal<u64> = RwSignal::new(0);
    create_effect(move |_| {
        if cmd.open.get() {
            focus_pulse.update(|x| *x += 1);
        }
    });

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
        .on_enter(run_selected)
        .style(|s| {
            theme::input_colors(s)
                .width_full()
                .height(36.0)
                .padding_horiz(10.0)
                .border(0.0)
                .border_bottom(1.0)
        })
        .request_focus(move || {
            focus_pulse.get();
        })
        .on_event_stop(floem::event::EventListener::FocusLost, move |_| {
            // Defer so a click on a list item runs before the palette closes.
            floem::action::exec_after(std::time::Duration::from_millis(150), move |_| {
                if cmd.open.get_untracked() {
                    cmd.open.set(false);
                }
            });
        })
        .on_key_down(
            Key::Named(NamedKey::Escape),
            |_| true,
            move |_| cmd.open.set(false),
        )
        .on_key_down(
            Key::Named(NamedKey::ArrowDown),
            |_| true,
            move |_| {
                let len = filtered().len();
                if len > 0 {
                    cmd.selected.update(|i| *i = (*i + 1).min(len - 1));
                }
            },
        )
        .on_key_down(
            Key::Named(NamedKey::ArrowUp),
            |_| true,
            move |_| {
                cmd.selected.update(|i| *i = i.saturating_sub(1));
            },
        );

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
            let s = s
                .absolute()
                .inset(0.0)
                .size_full()
                .justify_center()
                .items_start()
                .padding_top(90.0);
            if cmd.open.get() {
                s
            } else {
                s.hide()
            }
        })
        .on_click_stop(move |_| cmd.open.set(false))
}
