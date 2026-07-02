//! The graphical settings page (⌘,). A two-pane dialog: a category sidebar on
//! the left, the selected category's rows on the right, and a search box that
//! filters rows across every category. Changes apply live where possible and
//! are persisted to `config.json`.

use floem::peniko::Color;
use floem::reactive::{
    create_effect, create_rw_signal, RwSignal, SignalGet, SignalUpdate, SignalWith,
};
use floem::views::{
    container, dyn_container, empty, label, scroll, stack, stack_from_iter, text_input, Decorators,
};
use floem::{AnyView, IntoView};

use crate::config;
use crate::state::AppState;
use crate::theme;

/// Sidebar categories: `(glyph, name)`. Indices are used as the active key.
const CATS: &[(&str, &str)] = &[
    ("🎨", "Appearance"),
    ("✎", "Editor"),
    ("💾", "On Save"),
    ("▦", "Panels"),
    ("🐘", "Laravel"),
    ("🤖", "Agents"),
];

/// Shared row styling: uniform height, rhythm, and a hairline divider.
fn row_style(s: floem::style::Style) -> floem::style::Style {
    s.items_center()
        .width_full()
        .min_height(48.0)
        .padding_vert(6.0)
        .gap(8.0)
        .border_bottom(1.0)
        .border_color(theme::border())
}

/// A small "needs restart" pill for rows that don't yet apply live.
fn restart_badge() -> impl IntoView {
    label(|| "restart".to_string()).style(|s| {
        s.font_size(9.5)
            .padding_horiz(6.0)
            .padding_vert(1.0)
            .margin_left(8.0)
            .border_radius(4.0)
            .color(theme::fg_dim())
            .background(theme::bg_hover())
    })
}

/// The title + optional note + optional restart badge column of a row.
fn row_label(text: &'static str, note: &'static str, restart: bool) -> impl IntoView {
    let title: AnyView = if restart {
        stack((
            label(move || text.to_string()).style(|s| s.color(theme::fg()).font_size(13.0)),
            restart_badge(),
        ))
        .style(|s| s.items_center())
        .into_any()
    } else {
        label(move || text.to_string())
            .style(|s| s.color(theme::fg()).font_size(13.0))
            .into_any()
    };
    let mut col: Vec<AnyView> = vec![title.into_any()];
    if !note.is_empty() {
        col.push(
            label(move || note.to_string())
                .style(|s| {
                    s.color(theme::fg_dim())
                        .font_size(11.0)
                        .margin_top(2.0)
                        .margin_right(16.0)
                })
                .into_any(),
        );
    }
    stack_from_iter(col).style(|s| s.flex_col().flex_grow(1.0).min_width(0.0))
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

    stack((row_label(text, note, false), switch)).style(row_style)
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
        row_label(
            "App URL",
            "For request-replay. Empty = https://<folder>.test (Grove).",
            false,
        ),
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
        row_label(text, "", false),
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
    restart: bool,
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
    let seg_stack = stack_from_iter(segs).style(|s| {
        s.items_center()
            .border(1.0)
            .border_color(theme::border())
            .border_radius(6.0)
    });
    stack((row_label(text, "", restart), seg_stack)).style(row_style)
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

    stack((row_label("Default agent", "", false), menu)).style(row_style)
}

/// One settings row plus the metadata used for sidebar grouping and search.
struct RowItem {
    cat: usize,
    text: &'static str,
    note: &'static str,
    view: AnyView,
}

/// Build every settings row once, tagged with its category and search text.
fn all_rows(s: AppState) -> Vec<RowItem> {
    let mut rows: Vec<RowItem> = Vec::new();
    let mut push = |cat, text, note, view: AnyView| {
        rows.push(RowItem {
            cat,
            text,
            note,
            view,
        })
    };

    // 0 — Appearance
    push(
        0,
        "Dark theme",
        "",
        toggle_row(
            "Dark theme",
            "",
            move || s.settings.get().dark,
            move |v| {
                s.settings.update(|st| st.dark = v);
                theme::set_dark(v);
            },
        )
        .into_any(),
    );

    // 1 — Editor
    push(
        1,
        "Font size",
        "",
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
        )
        .into_any(),
    );
    push(
        1,
        "Tab width",
        "",
        number_row(
            "Tab width",
            move || s.settings.get().tab_width,
            1,
            16,
            move |n| {
                s.settings.update(|st| st.tab_width = n);
                config::set_usize("tab_width", n);
            },
        )
        .into_any(),
    );
    push(
        1,
        "Indent guides",
        "",
        toggle_row(
            "Indent guides",
            "",
            move || s.settings.get().indent_guides,
            move |v| {
                s.settings.update(|st| st.indent_guides = v);
                config::set_bool("indent_guides", v);
            },
        )
        .into_any(),
    );
    push(
        1,
        "Auto-close brackets & quotes",
        "",
        toggle_row(
            "Auto-close brackets & quotes",
            "",
            move || s.settings.get().auto_close,
            move |v| {
                s.settings.update(|st| st.auto_close = v);
                config::set_bool("auto_close", v);
            },
        )
        .into_any(),
    );
    push(
        1,
        "Inlay hints",
        "Inline type & parameter hints",
        toggle_row(
            "Inlay hints",
            "Inline type & parameter hints",
            move || s.settings.get().inlay_hints,
            move |v| {
                s.settings.update(|st| st.inlay_hints = v);
                config::set_bool("inlay_hints", v);
            },
        )
        .into_any(),
    );
    push(
        1,
        "Sticky scroll",
        "",
        toggle_row(
            "Sticky scroll",
            "",
            move || s.settings.get().sticky_scroll,
            move |v| {
                s.settings.update(|st| st.sticky_scroll = v);
                config::set_bool("sticky_scroll", v);
            },
        )
        .into_any(),
    );

    // 2 — On Save
    push(
        2,
        "Format on save",
        "",
        toggle_row(
            "Format on save",
            "",
            move || s.settings.get().format_on_save,
            move |v| {
                s.settings.update(|st| st.format_on_save = v);
                config::set_bool("format_on_save", v);
            },
        )
        .into_any(),
    );
    push(
        2,
        "Trim trailing whitespace",
        "",
        toggle_row(
            "Trim trailing whitespace",
            "",
            move || s.settings.get().trim_on_save,
            move |v| {
                s.settings.update(|st| st.trim_on_save = v);
                config::set_bool("trim_on_save", v);
            },
        )
        .into_any(),
    );
    push(
        2,
        "Auto-save",
        "Save after a short idle period",
        toggle_row(
            "Auto-save",
            "Save after a short idle period",
            move || s.settings.get().autosave,
            move |v| {
                s.settings.update(|st| st.autosave = v);
                config::set_bool("autosave", v);
            },
        )
        .into_any(),
    );

    // 3 — Panels (side placement — needs restart)
    push(
        3,
        "Explorer / Git sidebar",
        "",
        segmented_row(
            "Explorer / Git sidebar",
            true,
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
        )
        .into_any(),
    );
    push(
        3,
        "Agent panel",
        "",
        segmented_row(
            "Agent panel",
            true,
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
        )
        .into_any(),
    );
    push(
        3,
        "Database panel",
        "",
        segmented_row(
            "Database panel",
            true,
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
        )
        .into_any(),
    );

    // 4 — Laravel
    push(
        4,
        "Laravel features",
        "Completion, hover & go-to-definition for Laravel",
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
        )
        .into_any(),
    );
    push(
        4,
        "App URL",
        "For request-replay. Empty = https://<folder>.test (Grove).",
        app_url_row(s).into_any(),
    );

    // 5 — Agents
    push(5, "Default agent", "", default_agent_row(s).into_any());

    rows
}

/// A sidebar category button.
fn cat_item(
    active: RwSignal<usize>,
    search: RwSignal<String>,
    idx: usize,
    glyph: &'static str,
    name: &'static str,
) -> impl IntoView {
    label(move || format!("{glyph}   {name}"))
        .style(move |s| {
            let selected = search.with(|q| q.trim().is_empty()) && active.get() == idx;
            let s = s
                .width_full()
                .padding_horiz(12.0)
                .padding_vert(9.0)
                .border_radius(7.0)
                .font_size(13.0)
                .cursor(floem::style::CursorStyle::Pointer);
            if selected {
                s.background(theme::bg_hover()).color(theme::fg())
            } else {
                s.color(theme::fg_dim())
                    .hover(|s| s.background(theme::bg_hover()).color(theme::fg()))
            }
        })
        .on_click_stop(move |_| {
            active.set(idx);
            search.set(String::new());
        })
}

pub fn settings_view(state: AppState) -> impl IntoView {
    let active = create_rw_signal(1usize); // Editor by default
    let search = create_rw_signal(String::new());

    // ---- header: title + search + close ----
    let title = label(|| "Settings".to_string()).style(|s| {
        s.color(theme::fg())
            .font_size(18.0)
            .font_bold()
            .width(180.0)
    });
    let search_box = text_input(search)
        .placeholder("Search settings…")
        .style(|s| {
            theme::input_colors(s)
                .flex_grow(1.0)
                .font_size(13.0)
                .padding_horiz(12.0)
                .padding_vert(7.0)
                .border_radius(8.0)
        });
    let close = label(|| "✕".to_string())
        .style(|s| {
            s.margin_left(14.0)
                .padding_horiz(6.0)
                .font_size(16.0)
                .color(theme::fg_dim())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.color(theme::fg()))
        })
        .on_click_stop(move |_| state.settings_open.set(false));
    let header = stack((title, search_box, close)).style(|s| {
        s.flex_row()
            .items_center()
            .width_full()
            .padding_horiz(20.0)
            .padding_vert(14.0)
            .border_bottom(1.0)
            .border_color(theme::border())
    });

    // ---- sidebar ----
    let items = CATS
        .iter()
        .enumerate()
        .map(|(i, (glyph, name))| cat_item(active, search, i, glyph, name).into_any());
    let sidebar = stack_from_iter(items).style(|s| {
        s.flex_col()
            .width(210.0)
            .gap(2.0)
            .padding(12.0)
            .border_right(1.0)
            .border_color(theme::border())
    });

    // ---- content pane (reactive to category + search) ----
    let content = dyn_container(
        move || (active.get(), search.get()),
        move |(cat, q)| {
            let ql = q.trim().to_lowercase();
            let searching = !ql.is_empty();
            let heading = if searching {
                "Results".to_string()
            } else {
                CATS[cat].1.to_string()
            };
            let views: Vec<AnyView> = all_rows(state)
                .into_iter()
                .filter(|r| {
                    if searching {
                        r.text.to_lowercase().contains(&ql) || r.note.to_lowercase().contains(&ql)
                    } else {
                        r.cat == cat
                    }
                })
                .map(|r| r.view)
                .collect();
            let list: AnyView = if views.is_empty() {
                label(|| "No matching settings.".to_string())
                    .style(|s| s.color(theme::fg_dim()).font_size(13.0).margin_top(12.0))
                    .into_any()
            } else {
                stack_from_iter(views)
                    .style(|s| s.flex_col().width_full())
                    .into_any()
            };
            stack((
                label(move || heading.clone()).style(|s| {
                    s.color(theme::fg())
                        .font_size(17.0)
                        .font_bold()
                        .margin_bottom(6.0)
                }),
                list,
            ))
            .style(|s| s.flex_col().width_full())
            .into_any()
        },
    );
    let content_pane = scroll(content.style(|s| s.padding_horiz(24.0).padding_vert(18.0)))
        .style(|s| s.flex_grow(1.0).min_width(0.0).height_full());

    let middle = stack((sidebar, content_pane))
        .style(|s| s.flex_row().flex_grow(1.0).min_height(0.0).width_full());

    // ---- footer ----
    let footer = stack((
        label(|| "Changes are saved automatically".to_string())
            .style(|s| s.flex_grow(1.0).color(theme::fg_dim()).font_size(12.0)),
        label(|| "Open config.json".to_string())
            .style(|s| {
                s.color(theme::accent())
                    .font_size(12.0)
                    .cursor(floem::style::CursorStyle::Pointer)
                    .hover(|s| s.color(theme::fg()))
            })
            .on_click_stop(move |_| {
                if let Some(p) = config::settings_path() {
                    state.settings_open.set(false);
                    state.open_path(p);
                }
            }),
    ))
    .style(|s| {
        s.flex_row()
            .items_center()
            .width_full()
            .padding_horiz(20.0)
            .padding_vert(14.0)
            .border_top(1.0)
            .border_color(theme::border())
    });

    let box_ = stack((header, middle, footer))
        .style(|s| {
            s.flex_col()
                .width(780.0)
                .height(560.0)
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
