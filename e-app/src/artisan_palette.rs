//! The Artisan palette (⌘⇧A): pick a command the app declares, give it its
//! arguments and flags with the usage as a hint, and run it in the terminal.

use std::sync::Arc;

use floem::keyboard::{Key, NamedKey};
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::views::{container, dyn_stack, label, scroll, stack, text_input, Decorators};
use floem::IntoView;

use crate::artisan::{self, ArtisanCmd};
use crate::state::AppState;
use crate::theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// Choosing a command.
    Pick,
    /// Typing its arguments.
    Args,
}

#[derive(Clone, Copy)]
pub struct ArtisanState {
    pub open: RwSignal<bool>,
    pub stage: RwSignal<Stage>,
    pub query: RwSignal<String>,
    pub selected: RwSignal<usize>,
    pub chosen: RwSignal<Option<ArtisanCmd>>,
    pub args: RwSignal<String>,
    pub focus_pulse: RwSignal<u64>,
    pub args_pulse: RwSignal<u64>,
}

impl ArtisanState {
    pub fn new() -> Self {
        Self {
            open: RwSignal::new(false),
            stage: RwSignal::new(Stage::Pick),
            query: RwSignal::new(String::new()),
            selected: RwSignal::new(0),
            chosen: RwSignal::new(None),
            args: RwSignal::new(String::new()),
            focus_pulse: RwSignal::new(0),
            args_pulse: RwSignal::new(0),
        }
    }
}

impl AppState {
    /// Open the palette; list the app's commands the first time (boots the
    /// app once, off the UI thread).
    pub fn open_artisan_palette(&self) {
        self.open_artisan_palette_with("");
    }

    /// Open the palette with `query` already typed — `make:` is "New Class",
    /// every generator the app and its packages declare.
    pub fn open_artisan_palette_with(&self, query: &str) {
        let a = self.artisan;
        a.stage.set(Stage::Pick);
        a.query.set(query.to_string());
        a.selected.set(0);
        a.chosen.set(None);
        a.args.set(String::new());
        a.open.set(true);
        a.focus_pulse.update(|x| *x += 1);
        if self.artisan_cmds.with_untracked(|c| c.is_empty())
            && !self.artisan_loading.get_untracked()
        {
            self.artisan_loading.set(true);
            let root = self.root.get_untracked();
            let cmds = self.artisan_cmds;
            let loading = self.artisan_loading;
            let error = self.artisan_error;
            self.spawn_bg(
                move || artisan::list(&root),
                move |list: Result<Vec<ArtisanCmd>, String>| {
                    loading.set(false);
                    match list {
                        Ok(list) => {
                            error.set(String::new());
                            cmds.set(Arc::new(list));
                        }
                        Err(e) => {
                            eprintln!("e: artisan: {e}");
                            error.set(e);
                        }
                    }
                },
            );
        }
    }

    pub fn filtered_artisan(&self) -> Vec<ArtisanCmd> {
        let q = self.artisan.query.get();
        self.artisan_cmds.with(|c| artisan::filter(c, &q))
    }

    /// Move from picking a command to giving it arguments.
    pub fn artisan_pick(&self, index: usize) {
        let list = self.filtered_artisan();
        let Some(cmd) = list.get(index).cloned() else {
            return;
        };
        let a = self.artisan;
        a.chosen.set(Some(cmd));
        a.args.set(String::new());
        a.stage.set(Stage::Args);
        a.args_pulse.update(|x| *x += 1);
    }

    /// Run the chosen command with the typed arguments in a terminal tab.
    pub fn artisan_run(&self) {
        let a = self.artisan;
        let Some(cmd) = a.chosen.get_untracked() else {
            return;
        };
        let args = a.args.get_untracked().trim().to_string();
        if args.is_empty() && !cmd.required_args.is_empty() {
            Self::notify(&format!(
                "{} needs {}",
                cmd.name,
                cmd.required_args
                    .iter()
                    .map(|r| format!("<{r}>"))
                    .collect::<Vec<_>>()
                    .join(" ")
            ));
            return;
        }
        a.open.set(false);
        let command = if args.is_empty() {
            format!("php artisan {}", cmd.name)
        } else {
            format!("php artisan {} {args}", cmd.name)
        };
        self.run_task(&format!("artisan: {}", cmd.name), &command);
    }

    /// Escape: back to the command list, or close.
    pub fn artisan_back(&self) {
        let a = self.artisan;
        if a.stage.get_untracked() == Stage::Args {
            a.stage.set(Stage::Pick);
            a.focus_pulse.update(|x| *x += 1);
        } else {
            a.open.set(false);
        }
    }
}

pub fn artisan_palette(state: AppState) -> impl IntoView {
    let a = state.artisan;

    let pick_selected = move || {
        let idx = a.selected.get_untracked();
        state.artisan_pick(idx);
    };

    let query = text_input(a.query)
        .placeholder("Artisan command…")
        .on_enter(pick_selected)
        .style(move |s| {
            let s = theme::input_colors(s)
                .width_full()
                .height(36.0)
                .padding_horiz(10.0)
                .border(0.0)
                .border_bottom(1.0);
            if a.stage.get() == Stage::Pick {
                s
            } else {
                s.hide()
            }
        })
        .request_focus(move || {
            a.focus_pulse.get();
        })
        .on_key_down(
            Key::Named(NamedKey::Escape),
            |_| true,
            move |_| state.artisan_back(),
        )
        .on_key_down(
            Key::Named(NamedKey::ArrowDown),
            |_| true,
            move |_| {
                let len = state.filtered_artisan().len();
                if len > 0 {
                    a.selected.update(|i| *i = (*i + 1).min(len - 1));
                }
            },
        )
        .on_key_down(
            Key::Named(NamedKey::ArrowUp),
            |_| true,
            move |_| a.selected.update(|i| *i = i.saturating_sub(1)),
        );

    // Stage two: the chosen command's name, its usage as the hint, and the
    // argument line.
    let chosen_name = label(move || {
        a.chosen
            .get()
            .map(|c| format!("php artisan {}", c.name))
            .unwrap_or_default()
    })
    .style(|s| {
        s.font_family("monospace".to_string())
            .font_size(12.0)
            .color(theme::fg())
            .padding_horiz(10.0)
            .padding_top(8.0)
    });
    let usage = label(move || {
        a.chosen
            .get()
            .map(|c| format!("{}  —  {}", c.args_hint(), c.description))
            .unwrap_or_default()
    })
    .style(|s| {
        s.font_size(11.0)
            .color(theme::fg_dim())
            .padding_horiz(10.0)
            .padding_bottom(4.0)
            .text_ellipsis()
    });
    let args = text_input(a.args)
        .placeholder("arguments and flags, e.g. Order -mf")
        .on_enter(move || state.artisan_run())
        .style(|s| {
            theme::input_colors(s)
                .width_full()
                .height(36.0)
                .padding_horiz(10.0)
                .border(0.0)
                .border_top(1.0)
        })
        .request_focus(move || {
            a.args_pulse.get();
        })
        .on_key_down(
            Key::Named(NamedKey::Escape),
            |_| true,
            move |_| state.artisan_back(),
        );
    let args_stage = stack((chosen_name, usage, args)).style(move |s| {
        let s = s.flex_col().width_full();
        if a.stage.get() == Stage::Args {
            s
        } else {
            s.hide()
        }
    });

    let rows = dyn_stack(
        move || {
            if a.stage.get() != Stage::Pick {
                return Vec::new();
            }
            state
                .filtered_artisan()
                .into_iter()
                .enumerate()
                .collect::<Vec<_>>()
        },
        |(i, c)| (*i, c.name.clone()),
        move |(i, c)| {
            let name = c.name.clone();
            let desc = c.description.clone();
            stack((
                label(move || name.clone()).style(|s| {
                    s.color(theme::fg())
                        .font_family("monospace".to_string())
                        .font_size(12.0)
                }),
                label(move || desc.clone())
                    .style(|s| s.color(theme::fg_dim()).font_size(11.0).text_ellipsis()),
            ))
            .style(move |s| {
                let s = s
                    .flex_col()
                    .gap(1.0)
                    .width_full()
                    .padding_horiz(12.0)
                    .padding_vert(4.0)
                    .cursor(floem::style::CursorStyle::Pointer);
                if a.selected.get() == i {
                    s.background(theme::bg_active())
                } else {
                    s.hover(|s| s.background(theme::bg_hover()))
                }
            })
            .on_click_stop(move |_| {
                a.selected.set(i);
                state.artisan_pick(i);
            })
        },
    )
    .style(|s| s.flex_col().width_full());

    let rows_scroll = scroll(rows)
        .scroll_to_percent(move || {
            let n = state.filtered_artisan().len().max(1) as f32;
            a.selected.get() as f32 / n
        })
        .style(|s| s.max_height(400.0).width_full());

    let empty = label(move || {
        if state.artisan_loading.get() {
            "Asking the app for its commands…".to_string()
        } else {
            let error = state.artisan_error.get();
            if error.is_empty() {
                "No Artisan commands (is `php` on PATH and this a Laravel project?)".to_string()
            } else {
                format!("No Artisan commands: {error}")
            }
        }
    })
    .style(move |s| {
        let s = s.color(theme::fg_dim()).padding(14.0).font_size(12.0);
        if a.stage.get() == Stage::Pick && state.filtered_artisan().is_empty() {
            s
        } else {
            s.hide()
        }
    });

    let box_ = stack((query, args_stage, rows_scroll, empty))
        .style(|s| {
            s.flex_col()
                .width(620.0)
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
            if a.open.get() {
                s
            } else {
                s.hide()
            }
        })
        .on_click_stop(move |_| a.open.set(false))
}
