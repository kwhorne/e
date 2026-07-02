//! The graphical settings page (⌘,). Changes apply live where possible and are
//! persisted to `config.json`.

use floem::peniko::Color;
use floem::reactive::{create_effect, create_rw_signal, SignalGet, SignalUpdate, SignalWith};
use floem::views::{container, empty, label, scroll, stack, text_input, Decorators};
use floem::IntoView;

use crate::config;
use crate::state::AppState;
use crate::theme;

fn section(title: &'static str) -> impl IntoView {
    label(move || title.to_string()).style(|s| {
        s.color(theme::fg_dim())
            .font_size(11.0)
            .margin_top(18.0)
            .margin_bottom(6.0)
    })
}

/// A labelled on/off switch row.
fn toggle_row(
    text: &'static str,
    note: &'static str,
    value: impl Fn() -> bool + 'static + Copy,
    on_set: impl Fn(bool) + 'static,
) -> impl IntoView {
    let knob = empty().style(move |s| {
        let on = value();
        s.width(16.0)
            .height(16.0)
            .border_radius(8.0)
            .background(Color::from_rgb8(0x14, 0x16, 0x1b))
            .margin_left(if on { 18.0 } else { 2.0 })
            .margin_top(2.0)
    });
    let switch = stack((knob,))
        .style(move |s| {
            let on = value();
            s.width(38.0)
                .height(20.0)
                .border_radius(10.0)
                .cursor(floem::style::CursorStyle::Pointer)
                .background(if on { theme::accent() } else { theme::border() })
        })
        .on_click_stop(move |_| on_set(!value()));

    let mut col: Vec<floem::AnyView> = vec![label(move || text.to_string())
        .style(|s| s.color(theme::fg()).font_size(13.0))
        .into_any()];
    if !note.is_empty() {
        col.push(
            label(move || note.to_string())
                .style(|s| {
                    s.color(theme::fg_dim())
                        .font_size(11.0)
                        .margin_top(1.0)
                        .margin_right(16.0)
                })
                .into_any(),
        );
    }
    let text_col =
        floem::views::stack_from_iter(col).style(|s| s.flex_col().flex_grow(1.0).min_width(0.0));

    stack((text_col, switch)).style(row_style)
}

/// Shared row styling so every settings row has the same height and rhythm.
fn row_style(s: floem::style::Style) -> floem::style::Style {
    s.items_center().width_full().min_height(40.0).gap(8.0)
}

/// A labelled text-input row for the app base URL (persisted live).
fn app_url_row(state: AppState) -> impl IntoView {
    let sig = create_rw_signal(state.settings.get_untracked().app_url.clone());
    create_effect(move |prev: Option<()>| {
        let v = sig.get();
        if prev.is_some() {
            state.settings.update(|s| s.app_url = v.clone());
            config::set_str("app_url", &v);
        }
    });
    stack((
        stack((
            label(|| "App URL".to_string()).style(|s| s.color(theme::fg()).font_size(13.0)),
            label(|| "For request-replay. Empty = https://<folder>.test (Grove).".to_string())
                .style(|s| s.color(theme::fg_dim()).font_size(11.0)),
        ))
        .style(|s| s.flex_col().flex_grow(1.0).min_width(0.0)),
        text_input(sig)
            .placeholder("https://myapp.test")
            .style(|s| {
                theme::input_colors(s)
                    .width(260.0)
                    .font_size(12.0)
                    .padding_horiz(8.0)
                    .padding_vert(4.0)
            }),
    ))
    .style(row_style)
}

/// A labelled number row with − / + steppers.
fn number_row(
    text: &'static str,
    value: impl Fn() -> usize + 'static + Copy,
    min: usize,
    max: usize,
    on_set: impl Fn(usize) + 'static + Copy,
) -> impl IntoView {
    let step = |delta: i64| {
        move |_: &_| {
            let next = (value() as i64 + delta).clamp(min as i64, max as i64) as usize;
            on_set(next);
        }
    };
    let btn = |glyph: &'static str| {
        label(move || glyph.to_string()).style(|s| {
            s.width(26.0)
                .height(26.0)
                .items_center()
                .justify_center()
                .border(1.0)
                .border_color(theme::border())
                .border_radius(5.0)
                .color(theme::fg())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::bg_hover()))
        })
    };
    stack((
        label(move || text.to_string())
            .style(|s| s.color(theme::fg()).font_size(13.0).flex_grow(1.0)),
        btn("−").on_click_stop(step(-1)),
        label(move || format!("{}", value())).style(|s| {
            s.width(34.0)
                .items_center()
                .justify_center()
                .color(theme::fg())
        }),
        btn("+").on_click_stop(step(1)),
    ))
    .style(row_style)
}

/// A labelled segmented control (pick one of `options`).
fn segmented_row(
    text: &'static str,
    options: &'static [(&'static str, &'static str)],
    value: impl Fn() -> String + 'static + Copy,
    on_set: impl Fn(&'static str) + 'static + Copy,
) -> impl IntoView {
    let segs = options.iter().map(|(id, lbl)| {
        let id = *id;
        let lbl = *lbl;
        label(move || lbl.to_string())
            .style(move |s| {
                let active = value() == id;
                let s = s
                    .padding_horiz(12.0)
                    .height(26.0)
                    .items_center()
                    .font_size(12.0)
                    .cursor(floem::style::CursorStyle::Pointer);
                if active {
                    s.background(theme::accent())
                        .color(Color::from_rgb8(0x14, 0x16, 0x1b))
                } else {
                    s.color(theme::fg())
                        .hover(|s| s.background(theme::bg_hover()))
                }
            })
            .on_click_stop(move |_| on_set(id))
    });
    let seg_stack = floem::views::stack_from_iter(segs).style(|s| {
        s.items_center()
            .border(1.0)
            .border_color(theme::border())
            .border_radius(6.0)
    });
    stack((
        label(move || text.to_string())
            .style(|s| s.color(theme::fg()).font_size(13.0).flex_grow(1.0)),
        seg_stack,
    ))
    .style(row_style)
}

pub fn settings_view(state: AppState) -> impl IntoView {
    let s = state;

    let appearance = stack((
        section("APPEARANCE"),
        toggle_row(
            "Dark theme",
            "",
            move || s.settings.get().dark,
            move |v| {
                s.settings.update(|st| st.dark = v);
                theme::set_dark(v);
            },
        ),
    ))
    .style(|s| s.flex_col().width_full());

    let editor = stack((
        section("EDITOR"),
        number_row(
            "Font size",
            move || s.settings.get().font_size,
            8,
            32,
            move |n| {
                s.settings.update(|st| st.font_size = n);
                s.font_size.set(n);
                config::set_usize("font_size", n);
            },
        ),
        number_row(
            "Tab width",
            move || s.settings.get().tab_width,
            1,
            16,
            move |n| {
                s.settings.update(|st| st.tab_width = n);
                config::set_usize("tab_width", n);
            },
        ),
        toggle_row(
            "Indent guides",
            "",
            move || s.settings.get().indent_guides,
            move |v| {
                s.settings.update(|st| st.indent_guides = v);
                config::set_bool("indent_guides", v);
            },
        ),
        toggle_row(
            "Auto-close brackets & quotes",
            "",
            move || s.settings.get().auto_close,
            move |v| {
                s.settings.update(|st| st.auto_close = v);
                config::set_bool("auto_close", v);
            },
        ),
        toggle_row(
            "Inlay hints",
            "Inline type & parameter hints",
            move || s.settings.get().inlay_hints,
            move |v| {
                s.settings.update(|st| st.inlay_hints = v);
                config::set_bool("inlay_hints", v);
            },
        ),
        toggle_row(
            "Sticky scroll",
            "",
            move || s.settings.get().sticky_scroll,
            move |v| {
                s.settings.update(|st| st.sticky_scroll = v);
                config::set_bool("sticky_scroll", v);
            },
        ),
        toggle_row(
            "Laravel features",
            "Completion, hover & go-to-definition for Laravel",
            move || s.settings.get().laravel,
            move |v| {
                s.settings.update(|st| st.laravel = v);
                config::set_bool("laravel", v);
                if v {
                    s.load_laravel();
                }
            },
        ),
    ))
    .style(|s| s.flex_col().width_full());

    let on_save = stack((
        section("ON SAVE"),
        toggle_row(
            "Format on save",
            "",
            move || s.settings.get().format_on_save,
            move |v| {
                s.settings.update(|st| st.format_on_save = v);
                config::set_bool("format_on_save", v);
            },
        ),
        toggle_row(
            "Trim trailing whitespace",
            "",
            move || s.settings.get().trim_on_save,
            move |v| {
                s.settings.update(|st| st.trim_on_save = v);
                config::set_bool("trim_on_save", v);
            },
        ),
        toggle_row(
            "Auto-save",
            "Save after a short idle period",
            move || s.settings.get().autosave,
            move |v| {
                s.settings.update(|st| st.autosave = v);
                config::set_bool("autosave", v);
            },
        ),
    ))
    .style(|s| s.flex_col().width_full());

    let panels = stack((
        section("PANELS  (restart to apply)"),
        segmented_row(
            "Explorer / Git sidebar",
            &[("left", "Left"), ("right", "Right")],
            move || {
                if s.settings.get().sidebar_right {
                    "right".into()
                } else {
                    "left".into()
                }
            },
            move |v| {
                s.settings.update(|st| st.sidebar_right = v == "right");
                config::set_str("sidebar_side", v);
            },
        ),
        segmented_row(
            "Agent panel",
            &[("left", "Left"), ("right", "Right")],
            move || {
                if s.settings.get().agent_left {
                    "left".into()
                } else {
                    "right".into()
                }
            },
            move |v| {
                s.settings.update(|st| st.agent_left = v == "left");
                config::set_str("agent_side", v);
            },
        ),
        segmented_row(
            "Database panel",
            &[("left", "Left"), ("right", "Right")],
            move || {
                if s.settings.get().database_left {
                    "left".into()
                } else {
                    "right".into()
                }
            },
            move |v| {
                s.settings.update(|st| st.database_left = v == "left");
                config::set_str("database_side", v);
            },
        ),
        section("LARAVEL"),
        app_url_row(state),
        section("AGENT"),
        default_agent_row(state),
    ))
    .style(|s| s.flex_col().width_full());

    let body = stack((
        label(|| "Settings".to_string()).style(|s| s.color(theme::fg()).font_size(20.0)),
        appearance,
        editor,
        on_save,
        panels,
        label(|| "Close".to_string())
            .style(|s| {
                s.margin_top(22.0)
                    .padding_horiz(24.0)
                    .padding_vert(8.0)
                    .background(theme::bg())
                    .color(theme::fg())
                    .border(1.0)
                    .border_color(theme::border())
                    .border_radius(8.0)
                    .cursor(floem::style::CursorStyle::Pointer)
                    .hover(|s| s.background(theme::bg_hover()))
            })
            .on_click_stop(move |_| state.settings_open.set(false)),
    ))
    .style(|s| s.flex_col().width_full());

    let box_ = scroll(body.style(|s| s.padding(24.0)))
        .style(|s| {
            s.width(520.0)
                .max_height(640.0)
                .background(theme::bg_panel())
                .border(1.0)
                .border_color(theme::border())
                .border_radius(12.0)
        })
        .on_click_stop(|_| {});

    container(box_)
        .style(move |s| {
            let s = s
                .absolute()
                .inset(0.0)
                .size_full()
                .items_center()
                .justify_center()
                .background(Color::from_rgba8(0, 0, 0, 0xCC));
            if state.settings_open.get() {
                s
            } else {
                s.hide()
            }
        })
        .on_click_stop(move |_| state.settings_open.set(false))
}

/// Default-agent picker built from the configured agents.
fn default_agent_row(state: AppState) -> impl IntoView {
    let menu = label(move || {
        let id = state.agent_current.get();
        state
            .agents
            .with(|list| {
                list.iter()
                    .find(|a| a.id == id)
                    .map(|a| format!("{}  ▾", a.name))
            })
            .unwrap_or_default()
    })
    .style(|s| {
        s.padding_horiz(12.0)
            .height(26.0)
            .items_center()
            .border(1.0)
            .border_color(theme::border())
            .border_radius(6.0)
            .color(theme::fg())
            .font_size(12.0)
            .cursor(floem::style::CursorStyle::Pointer)
            .hover(|s| s.background(theme::bg_hover()))
    })
    .popout_menu(move || {
        let cur = state.agent_current.get_untracked();
        let mut m = floem::menu::Menu::new("");
        for a in state.agents.get_untracked() {
            let id = a.id.clone();
            let mark = if id == cur { "● " } else { "   " };
            m = m.entry(
                floem::menu::MenuItem::new(format!("{mark}{}", a.name))
                    .action(move || state.select_agent(&id)),
            );
        }
        m
    });

    stack((
        label(|| "Default agent".to_string())
            .style(|s| s.color(theme::fg()).font_size(13.0).flex_grow(1.0)),
        menu,
    ))
    .style(|s| s.items_center().width_full().padding_vert(6.0))
}
