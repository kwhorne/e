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
    ("run-sql", "Database: Run SQL Under Cursor (⌘⏎)"),
    ("tinker", "Tinker: Toggle Scratchpad"),
    ("tinker-selection", "Tinker: Run Selection"),
    ("laravel-map", "Laravel: Architecture Map"),
    ("agent-log", "Agent: Timeline / Audit Log"),
    ("run-tests", "Tests: Runner / Autonomous TDD"),
    ("laravel-log", "Laravel: Log Tail"),
    ("runtime", "Laravel: Runtime Insight (Telescope)"),
    ("schema-diff", "Laravel: Schema Diff (migrations vs DB)"),
    ("relations", "Laravel: Eloquent Relationship Graph"),
    ("event-graph", "Laravel: Event Dispatch Graph"),
    ("props-contract", "Inertia: Props Contract / Generate TS"),
    (
        "related-files",
        "Laravel: Related Files (model / migration / …)",
    ),
    ("livewire-companion", "Livewire: Switch View / Class"),
    ("debug-panel", "Debug: Toggle Panel"),
    ("debug", "Debug: Start / Continue (F5)"),
    ("debug-toggle-breakpoint", "Debug: Toggle Breakpoint (F9)"),
    ("debug-step-over", "Debug: Step Over (F10)"),
    ("debug-step-into", "Debug: Step Into (F11)"),
    ("debug-step-out", "Debug: Step Out (⇧F11)"),
    ("debug-stop", "Debug: Stop"),
    ("undo-tree", "Undo Tree: Show / Time Travel"),
    ("semantic-search", "Search: Semantic (describe it)"),
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
}

impl CmdPalette {
    pub fn new() -> Self {
        Self {
            open: RwSignal::new(false),
        }
    }
}

pub fn run_command(state: AppState, id: &str) {
    crate::commands::dispatch(state, id);
}

/// Fuzzy-rank the command list for a query (empty query → all, in order).
fn rank_commands(query: &str) -> Vec<(&'static str, &'static str)> {
    let q = query.to_lowercase();
    if q.trim().is_empty() {
        return COMMANDS.to_vec();
    }
    let mut scored: Vec<(i64, usize, (&'static str, &'static str))> = COMMANDS
        .iter()
        .enumerate()
        .filter_map(|(i, (id, label))| {
            crate::palette::fuzzy_score(&q, &label.to_lowercase()).map(|sc| (sc, i, (*id, *label)))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, _, c)| c).collect()
}

pub fn command_palette(state: AppState) -> impl IntoView {
    let cmd = state.cmd;
    // Local signals (like the goto-file palette). The `text_input`, filter and
    // list all share these within this view's reactive scope, so typing filters
    // live. (Using AppState-owned signals from another scope didn't propagate
    // reliably, leaving the list stuck on the unfiltered set.)
    let query: RwSignal<String> = RwSignal::new(String::new());
    let selected: RwSignal<usize> = RwSignal::new(0);

    let focus_pulse: RwSignal<u64> = RwSignal::new(0);
    create_effect(move |_| {
        if cmd.open.get() {
            // Start fresh every time: a stale query from a previous invocation
            // would make new keystrokes append (e.g. "che" → "<old>che") and match
            // nothing — which read as an unresponsive palette.
            query.set(String::new());
            selected.set(0);
            focus_pulse.update(|x| *x += 1);
        }
    });

    // Reset the highlight to the top result whenever the query changes.
    create_effect(move |_| {
        query.get();
        selected.set(0);
    });

    // Fuzzy subsequence match on the label, ranked by score (word-boundary and
    // consecutive-character bonuses), ties broken by original order.
    let filtered = move || -> Vec<(&'static str, &'static str)> { rank_commands(&query.get()) };

    let run_selected = move || {
        let results = filtered();
        if results.is_empty() {
            return;
        }
        let idx = selected.get().min(results.len() - 1);
        cmd.open.set(false);
        run_command(state, results[idx].0);
    };

    let input = text_input(query)
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
                    selected.update(|i| *i = (*i + 1).min(len - 1));
                }
            },
        )
        .on_key_down(
            Key::Named(NamedKey::ArrowUp),
            |_| true,
            move |_| {
                selected.update(|i| *i = i.saturating_sub(1));
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
                    if selected.get() == i {
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
            selected.get() as f32 / n
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn up_ranks_relevant_commands() {
        let ids: Vec<&str> = rank_commands("up").iter().map(|(id, _)| *id).collect();
        let pos = |id: &str| ids.iter().position(|x| *x == id);
        // Strong word-boundary matches must be found and rank at the very top.
        assert!(
            ids.contains(&"check-updates"),
            "check-updates missing: {ids:?}"
        );
        assert!(
            ids.contains(&"move-line-up"),
            "move-line-up missing: {ids:?}"
        );
        let top2 = &ids[..2.min(ids.len())];
        assert!(
            top2.contains(&"check-updates") && top2.contains(&"move-line-up"),
            "top: {top2:?}"
        );
        // A weak subsequence like "Architecture Map" (u…p) may match but must
        // rank *below* the strong ones (this was the reported bug).
        if let Some(weak) = pos("laravel-map") {
            assert!(weak > pos("check-updates").unwrap());
            assert!(weak > pos("move-line-up").unwrap());
        }
        // Labels with no "u…p" subsequence are excluded entirely.
        assert!(!ids.contains(&"tinker-selection"), "{ids:?}");
        assert!(!ids.contains(&"agent-log"), "{ids:?}");
    }

    #[test]
    fn empty_query_returns_all() {
        assert_eq!(rank_commands("").len(), COMMANDS.len());
    }

    #[test]
    fn prefix_query_matches_check_updates() {
        // "che" must surface "Check for Updates" (the palette felt unresponsive
        // only because a stale query wasn't cleared on open, not because of the
        // ranking).
        let ids: Vec<&str> = rank_commands("che").iter().map(|(id, _)| *id).collect();
        assert!(ids.contains(&"check-updates"), "che -> {ids:?}");
        assert_eq!(ids.first(), Some(&"check-updates"), "che top: {ids:?}");
    }

    #[test]
    fn abbreviations_match() {
        // "tcs" → "Toggle Source Control"? subsequence t-c-s? no. Use "tsc".
        let ids: Vec<&str> = rank_commands("sched").iter().map(|(id, _)| *id).collect();
        assert!(ids.contains(&"schema-diff"), "{ids:?}");
    }
}
