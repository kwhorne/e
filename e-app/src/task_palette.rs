//! The task-runner palette (⌘⇧B): pick a detected project task to run in the
//! terminal.

use floem::keyboard::{Key, NamedKey};
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::views::{container, dyn_stack, label, scroll, stack, text_input, Decorators};
use floem::IntoView;

use crate::state::AppState;
use crate::tasks;
use crate::theme;

#[derive(Clone, Copy)]
pub struct TaskState {
    pub open: RwSignal<bool>,
    pub query: RwSignal<String>,
    pub selected: RwSignal<usize>,
    pub focus_pulse: RwSignal<u64>,
}

impl TaskState {
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
    /// Detect tasks and open the task palette.
    pub fn open_task_palette(&self) {
        let detected = tasks::detect(&self.root.get_untracked());
        self.task_list.set(detected);
        self.task.query.set(String::new());
        self.task.selected.set(0);
        self.task.focus_pulse.update(|x| *x += 1);
        self.task.open.set(true);
    }

    fn filtered_tasks(&self) -> Vec<tasks::Task> {
        let q = self.task.query.get().to_lowercase();
        self.task_list.with(|list| {
            list.iter()
                .filter(|t| {
                    q.is_empty()
                        || t.label.to_lowercase().contains(&q)
                        || t.command.to_lowercase().contains(&q)
                })
                .cloned()
                .collect()
        })
    }
}

pub fn task_palette(state: AppState) -> impl IntoView {
    let task = state.task;

    let run_selected = move || {
        let results = state.filtered_tasks();
        if results.is_empty() {
            return;
        }
        let idx = task.selected.get_untracked().min(results.len() - 1);
        let t = results[idx].clone();
        task.open.set(false);
        state.run_task(&t.label, &t.command);
    };

    let input = text_input(task.query)
        .placeholder("Run task…")
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
            task.focus_pulse.get();
        })
        .on_event_stop(floem::event::EventListener::FocusLost, move |_| {
            floem::action::exec_after(std::time::Duration::from_millis(150), move |_| {
                if task.open.get_untracked() {
                    task.open.set(false);
                }
            });
        })
        .on_key_down(
            Key::Named(NamedKey::Escape),
            |_| true,
            move |_| task.open.set(false),
        )
        .on_key_down(
            Key::Named(NamedKey::ArrowDown),
            |_| true,
            move |_| {
                let len = state.filtered_tasks().len();
                if len > 0 {
                    task.selected.update(|i| *i = (*i + 1).min(len - 1));
                }
            },
        )
        .on_key_down(
            Key::Named(NamedKey::ArrowUp),
            |_| true,
            move |_| {
                task.selected.update(|i| *i = i.saturating_sub(1));
            },
        );

    let rows = dyn_stack(
        move || {
            state
                .filtered_tasks()
                .into_iter()
                .enumerate()
                .collect::<Vec<_>>()
        },
        |(i, t)| (*i, t.label.clone()),
        move |(i, t)| {
            let lbl = t.label.clone();
            let cmd = t.command.clone();
            let run_lbl = t.label.clone();
            let run_cmd = t.command.clone();
            stack((
                label(move || lbl.clone()).style(|s| s.color(theme::fg())),
                label(move || cmd.clone()).style(|s| {
                    s.color(theme::fg_dim())
                        .font_size(11.0)
                        .font_family("monospace".to_string())
                }),
            ))
            .style(move |s| {
                let s = s
                    .flex_col()
                    .gap(1.0)
                    .width_full()
                    .padding_horiz(12.0)
                    .padding_vert(4.0)
                    .cursor(floem::style::CursorStyle::Pointer);
                if task.selected.get() == i {
                    s.background(theme::bg_active())
                } else {
                    s.hover(|s| s.background(theme::bg_hover()))
                }
            })
            .on_click_stop(move |_| {
                task.selected.set(i);
                task.open.set(false);
                state.run_task(&run_lbl, &run_cmd);
            })
        },
    )
    .style(|s| s.flex_col().width_full());

    let rows_scroll = scroll(rows)
        .scroll_to_percent(move || {
            let n = state.filtered_tasks().len().max(1) as f32;
            task.selected.get() as f32 / n
        })
        .style(|s| s.max_height(360.0).width_full());

    let empty = label(|| "No tasks found in this project.".to_string()).style(move |s| {
        let s = s.color(theme::fg_dim()).padding(14.0).font_size(12.0);
        if state.filtered_tasks().is_empty() {
            s
        } else {
            s.hide()
        }
    });

    let box_ = stack((input, rows_scroll, empty))
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
            let s = s
                .absolute()
                .inset(0.0)
                .size_full()
                .justify_center()
                .items_start()
                .padding_top(90.0);
            if task.open.get() {
                s
            } else {
                s.hide()
            }
        })
        .on_click_stop(move |_| task.open.set(false))
}
