//! The Database panel (right side by default, toggled with ⌘3) and the results
//! overlay. Inspired by the Conductor database panel.

use floem::peniko::Color;
use floem::reactive::{
    create_effect, create_rw_signal, RwSignal, SignalGet, SignalUpdate, SignalWith,
};
use floem::views::{
    container, dyn_container, dyn_stack, empty, label, scroll, stack, text_input, Decorators,
};
use floem::IntoView;

use crate::state::{AppState, DbEntry, DbForm};
use crate::theme;

/// The accent colour for a connection's environment (green / amber / red).
fn env_color(env: e_db::Environment) -> Color {
    match env {
        e_db::Environment::Local => Color::from_rgb8(0x98, 0xc3, 0x79),
        e_db::Environment::Staging => Color::from_rgb8(0xe5, 0xc0, 0x7b),
        e_db::Environment::Production => Color::from_rgb8(0xe0, 0x6c, 0x75),
    }
}

/// Nudge the results-grid scroll by `(dx, dy)`; the tick makes each call a
/// distinct signal value so the `scroll_delta` effect always re-fires.
fn db_scroll(state: AppState, dx: f64, dy: f64) {
    state.db_scroll.update(|(x, y, t)| {
        *x = dx;
        *y = dy;
        *t = t.wrapping_add(1);
    });
}

/// Collapse a cell value to a single line for the grid (DB values often hold
/// multi-line text, which would otherwise blow up the row height).
fn sanitize_cell(s: &str) -> String {
    let mut out: String = s
        .chars()
        .take(200)
        .map(|c| {
            if matches!(c, '\n' | '\r' | '\t') {
                ' '
            } else {
                c
            }
        })
        .collect();
    if s.chars().nth(200).is_some() {
        out.push('…');
    }
    out
}

fn engine_icon(engine: &str) -> &'static str {
    match engine {
        "mysql" | "mariadb" => "🐬",
        "postgres" | "postgresql" | "pgsql" => "🐘",
        "sqlite" => "📦",
        "clickhouse" | "ch" => "🟡",
        _ => "🗄",
    }
}

/// One connection row + its (lazy) table list.
fn conn_row(state: AppState, entry: DbEntry) -> impl IntoView {
    let e1 = entry.clone();
    let e_toggle = entry.clone();
    let caret = label(move || {
        if e1.connecting.get() {
            "◌".to_string()
        } else if e1.conn.get().is_some() && e1.expanded.get() {
            "▾".to_string()
        } else {
            "▸".to_string()
        }
    })
    .style(|s| s.width(12.0).color(theme::fg_dim()));

    let eng = entry.config.engine.clone();
    let glyph = label(move || engine_icon(&eng).to_string());
    let name_cfg = entry.config.clone();
    let name = label(move || name_cfg.display_name()).style(|s| {
        s.flex_grow(1.0)
            .color(theme::fg())
            .text_ellipsis()
            .min_width(0.0)
    });
    let e_count = entry.clone();
    let count = label(move || {
        let c = e_count.tables.get().len();
        if e_count.conn.get().is_some() {
            c.to_string()
        } else {
            String::new()
        }
    })
    .style(|s| s.color(theme::fg_dim()).font_size(10.0));

    // Read-only lock: amber 🔒 when protected (defaults on for prod), dim 🔓 when
    // writes are enabled. Click to toggle.
    let e_lock = entry.clone();
    let e_lock2 = entry.clone();
    let lock = label(move || {
        if e_lock.read_only.get() {
            "🔒".to_string()
        } else {
            "🔓".to_string()
        }
    })
    .style(move |s| {
        let ro = e_lock.read_only.get();
        s.font_size(10.0)
            .cursor(floem::style::CursorStyle::Pointer)
            .color(if ro {
                Color::from_rgb8(0xe5, 0xc0, 0x7b)
            } else {
                theme::fg_dim()
            })
    })
    .on_click_stop(move |_| state.db_toggle_read_only(e_lock2.clone()));

    // Environment: coloured dot (green local / amber staging / red production),
    // plus an explicit badge for non-local so writes are never a surprise.
    let env = entry.config.environment();
    let env_color = env_color(env);
    let env_dot = label(|| "●".to_string()).style(move |s| s.font_size(9.0).color(env_color));
    let env_badge = label(move || env.label().to_uppercase()).style(move |s| {
        let s = s
            .font_size(9.0)
            .padding_horiz(4.0)
            .border_radius(3.0)
            .color(env_color)
            .border(1.0)
            .border_color(env_color);
        if env.is_local() {
            s.hide()
        } else {
            s
        }
    });

    let head = stack((caret, env_dot, glyph, name, env_badge, lock, count))
        .style(|s| {
            s.flex_row()
                .items_center()
                .gap(6.0)
                .flex_grow(1.0)
                .padding_horiz(4.0)
                .padding_vert(4.0)
                .min_width(0.0)
                .cursor(floem::style::CursorStyle::Pointer)
        })
        .on_click_stop(move |_| state.db_toggle(e_toggle.clone()));

    // Action buttons (query / refresh / disconnect / remove).
    let e_q = entry.clone();
    let q_btn = action_glyph("⌗", move || state.db_new_query(e_q.clone()));
    let e_r = entry.clone();
    let r_btn = action_glyph("⟳", move || state.db_refresh_tables(e_r.clone()));
    let e_snap = entry.clone();
    let snap_btn = action_glyph("⤓", move || state.db_snapshot(e_snap.clone()));
    let e_d = entry.clone();
    let d_btn = action_glyph("⏏", move || state.db_disconnect(e_d.clone()));
    let e_e = entry.clone();
    let edit_btn = action_glyph("✎", move || state.db_start_edit(e_e.clone()));
    let key_rm = entry.key();
    let x_btn = action_glyph("✕", move || state.db_remove(key_rm.clone()));
    let actions = stack((q_btn, r_btn, snap_btn, d_btn, edit_btn, x_btn))
        .style(|s| s.flex_row().gap(2.0).items_center());

    let row = stack((head, actions)).style(|s| {
        s.flex_row()
            .items_center()
            .width_full()
            .border_radius(5.0)
            .hover(|s| s.background(theme::bg_hover()))
    });

    // Error line.
    let e_err = entry.clone();
    let err = label(move || e_err.error.get().unwrap_or_default()).style(move |s| {
        let s = s
            .color(Color::from_rgb8(0xf7, 0x76, 0x8e))
            .font_size(11.0)
            .padding_horiz(22.0)
            .padding_vert(2.0);
        if e_err.error.get().is_some() {
            s
        } else {
            s.height(0.0).hide()
        }
    });

    // Table list (shown when expanded + connected).
    let e_filter = entry.clone();
    let filter = text_input(entry.filter)
        .placeholder("Filter tables…")
        .style(move |s| {
            let s = theme::input_colors(s)
                .margin_left(20.0)
                .margin_vert(2.0)
                .font_size(11.0)
                .padding_horiz(6.0)
                .padding_vert(2.0);
            if e_filter.expanded.get() && e_filter.conn.get().is_some() {
                s
            } else {
                s.height(0.0).hide()
            }
        });

    let e_views_read = entry.clone();
    let e_views_open = entry.clone();
    let e_tables = entry.clone();
    let tables = dyn_stack(
        move || {
            let f = e_tables.filter.get().to_lowercase();
            if e_tables.expanded.get() && e_tables.conn.get().is_some() {
                e_tables
                    .tables
                    .get()
                    .into_iter()
                    .filter(|t| f.is_empty() || t.to_lowercase().contains(&f))
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            }
        },
        |t| t.clone(),
        move |t| {
            let entry = entry.clone();
            let table = t.clone();
            let tn = t.clone();
            let e_count = entry.clone();
            let t_count = t.clone();
            let count = label(move || {
                e_count
                    .table_counts
                    .with(|m| m.get(&t_count).map(|n| n.to_string()))
                    .unwrap_or_default()
            })
            .style(|s| s.color(theme::fg_dim()).font_size(10.0).flex_shrink(0.0));
            stack((
                label(|| "▦".to_string()).style(|s| s.color(theme::fg_dim()).font_size(11.0)),
                label(move || tn.clone()).style(|s| {
                    s.color(theme::fg())
                        .flex_grow(1.0)
                        .text_ellipsis()
                        .min_width(0.0)
                }),
                count,
            ))
            .style(|s| {
                s.flex_row()
                    .items_center()
                    .gap(7.0)
                    .width_full()
                    .padding_left(22.0)
                    .padding_vert(3.0)
                    .border_radius(5.0)
                    .cursor(floem::style::CursorStyle::Pointer)
                    .hover(|s| s.background(theme::bg_hover()))
            })
            .on_click_stop(move |_| state.db_open_table(entry.clone(), table.clone()))
        },
    )
    .style(|s| s.flex_col().width_full());

    // Views: same look as tables, opened (browsed) the same way.
    let views = dyn_stack(
        move || {
            let f = e_views_read.filter.get().to_lowercase();
            if e_views_read.expanded.get() && e_views_read.conn.get().is_some() {
                e_views_read
                    .views
                    .get()
                    .into_iter()
                    .filter(|t| f.is_empty() || t.to_lowercase().contains(&f))
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            }
        },
        |t| t.clone(),
        move |t| {
            let entry = e_views_open.clone();
            let view = t.clone();
            let vn = t.clone();
            stack((
                label(|| "◉".to_string()).style(|s| s.color(theme::fg_dim()).font_size(11.0)),
                label(move || vn.clone())
                    .style(|s| s.color(theme::fg()).text_ellipsis().min_width(0.0)),
            ))
            .style(|s| {
                s.flex_row()
                    .items_center()
                    .gap(7.0)
                    .width_full()
                    .padding_left(22.0)
                    .padding_vert(3.0)
                    .border_radius(5.0)
                    .cursor(floem::style::CursorStyle::Pointer)
                    .hover(|s| s.background(theme::bg_hover()))
            })
            .on_click_stop(move |_| state.db_open_table(entry.clone(), view.clone()))
        },
    )
    .style(|s| s.flex_col().width_full());

    stack((row, err, filter, tables, views)).style(|s| s.flex_col().width_full().margin_bottom(2.0))
}

fn action_glyph(glyph: &'static str, on: impl Fn() + 'static) -> impl IntoView {
    label(move || glyph.to_string())
        .style(|s| {
            s.padding_horiz(5.0)
                .padding_vert(2.0)
                .border_radius(4.0)
                .color(theme::fg_dim())
                .font_size(12.0)
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::bg()).color(theme::fg()))
        })
        .on_click_stop(move |_| on())
}

fn form_field(
    label_text: &'static str,
    sig: RwSignal<String>,
    placeholder: &'static str,
    _password: bool,
) -> impl IntoView {
    let input = text_input(sig).placeholder(placeholder);
    stack((
        label(move || label_text.to_string()).style(|s| s.font_size(11.0).color(theme::fg_dim())),
        input.style(|s| {
            theme::input_colors(s)
                .width_full()
                .font_size(12.0)
                .padding_horiz(6.0)
                .padding_vert(3.0)
        }),
    ))
    .style(|s| s.flex_col().gap(2.0).width_full())
}

fn add_form(state: AppState) -> impl IntoView {
    // Bind the form struct's fields to local signals, syncing back on change.
    let f = state.db_form.get_untracked();
    let engine = create_rw_signal(f.engine.clone());
    let host = create_rw_signal(f.host.clone());
    let port = create_rw_signal(f.port.clone());
    let database = create_rw_signal(f.database.clone());
    let username = create_rw_signal(f.username.clone());
    let password = create_rw_signal(f.password.clone());
    let path = create_rw_signal(f.path.clone());
    let group = create_rw_signal(f.group.clone());
    let use_ssh = create_rw_signal(f.use_ssh);
    let ssh_host = create_rw_signal(f.ssh_host.clone());
    let ssh_port = create_rw_signal(f.ssh_port.clone());
    let ssh_user = create_rw_signal(f.ssh_user.clone());
    let ssh_auth = create_rw_signal(f.ssh_auth.clone());
    let ssh_password = create_rw_signal(f.ssh_password.clone());
    let ssh_key_path = create_rw_signal(f.ssh_key_path.clone());
    let ssh_passphrase = create_rw_signal(f.ssh_passphrase.clone());

    let sync = std::rc::Rc::new(move || {
        state.db_form.set(DbForm {
            engine: engine.get_untracked(),
            host: host.get_untracked(),
            port: port.get_untracked(),
            database: database.get_untracked(),
            username: username.get_untracked(),
            password: password.get_untracked(),
            path: path.get_untracked(),
            group: group.get_untracked(),
            use_ssh: use_ssh.get_untracked(),
            ssh_host: ssh_host.get_untracked(),
            ssh_port: ssh_port.get_untracked(),
            ssh_user: ssh_user.get_untracked(),
            ssh_auth: ssh_auth.get_untracked(),
            ssh_password: ssh_password.get_untracked(),
            ssh_key_path: ssh_key_path.get_untracked(),
            ssh_passphrase: ssh_passphrase.get_untracked(),
        });
    });
    let sync_test = sync.clone();

    // Engine selector (clickable chips).
    let engine_chip = move |id: &'static str, name: &'static str| {
        label(move || name.to_string())
            .style(move |s| {
                let active = engine.get() == id;
                let s = s
                    .padding_horiz(8.0)
                    .padding_vert(3.0)
                    .border_radius(5.0)
                    .font_size(11.0)
                    .cursor(floem::style::CursorStyle::Pointer);
                if active {
                    s.background(theme::accent())
                        .color(Color::from_rgb8(0x14, 0x16, 0x1b))
                } else {
                    s.border(1.0)
                        .border_color(theme::border())
                        .color(theme::fg())
                }
            })
            .on_click_stop(move |_| {
                engine.set(id.to_string());
                let def = match id {
                    "mysql" => "3306",
                    "postgres" => "5432",
                    "clickhouse" => "8123",
                    _ => "",
                };
                if !def.is_empty() {
                    port.set(def.to_string());
                }
            })
    };

    let engines = stack((
        engine_chip("mysql", "MySQL"),
        engine_chip("postgres", "Postgres"),
        engine_chip("sqlite", "SQLite"),
        engine_chip("clickhouse", "ClickHouse"),
    ))
    .style(|s| {
        s.flex_row()
            .gap(5.0)
            .flex_wrap(floem::taffy::style::FlexWrap::Wrap)
    });

    let from_env = label(|| "From .env".to_string())
        .style(|s| {
            s.width_full()
                .height(28.0)
                .items_center()
                .justify_center()
                .border_radius(5.0)
                .font_size(12.0)
                .background(theme::accent())
                .color(Color::from_rgb8(0x14, 0x16, 0x1b))
                .cursor(floem::style::CursorStyle::Pointer)
        })
        .on_click_stop(move |_| {
            state.db_editing_key.set(None);
            state.db_add_from_env();
            state.db_adding.set(false);
        });

    // Connection fields — sqlite shows just a path; others show host/port/etc.
    let net_fields = dyn_stack(
        move || {
            if engine.get() == "sqlite" {
                vec!["path"]
            } else {
                vec!["host", "port", "database", "username", "password"]
            }
        },
        |k| k.to_string(),
        move |k| match k {
            "path" => form_field("File path", path, "/path/to/database.sqlite", false).into_any(),
            "host" => form_field("Host", host, "127.0.0.1", false).into_any(),
            "port" => form_field("Port", port, "3306", false).into_any(),
            "database" => form_field("Database", database, "", false).into_any(),
            "username" => form_field("User", username, "root", false).into_any(),
            "password" => form_field("Password", password, "", true).into_any(),
            _ => empty().into_any(),
        },
    )
    .style(|s| s.flex_col().gap(6.0).width_full());

    // SSH tunnel toggle + fields (hidden for sqlite).
    let ssh_toggle = label(move || {
        if use_ssh.get() {
            "☑ Use SSH tunnel".to_string()
        } else {
            "☐ Use SSH tunnel".to_string()
        }
    })
    .style(move |s| {
        let s = s
            .font_size(12.0)
            .color(theme::fg_dim())
            .cursor(floem::style::CursorStyle::Pointer)
            .hover(|s| s.color(theme::fg()));
        if engine.get() == "sqlite" {
            s.hide()
        } else {
            s
        }
    })
    .on_click_stop(move |_| use_ssh.update(|v| *v = !*v));

    let auth_chip = move |id: &'static str, name: &'static str| {
        label(move || name.to_string())
            .style(move |s| {
                let active = ssh_auth.get() == id;
                let s = s
                    .padding_horiz(8.0)
                    .padding_vert(2.0)
                    .border_radius(5.0)
                    .font_size(11.0)
                    .cursor(floem::style::CursorStyle::Pointer);
                if active {
                    s.background(theme::accent())
                        .color(Color::from_rgb8(0x14, 0x16, 0x1b))
                } else {
                    s.border(1.0)
                        .border_color(theme::border())
                        .color(theme::fg())
                }
            })
            .on_click_stop(move |_| ssh_auth.set(id.to_string()))
    };
    let ssh_fields = dyn_stack(
        move || {
            if use_ssh.get() && engine.get() != "sqlite" {
                let mut v = vec!["host", "port", "user", "auth"];
                if ssh_auth.get() == "password" {
                    v.push("sshpass");
                } else {
                    v.push("key");
                    v.push("passphrase");
                }
                v
            } else {
                vec![]
            }
        },
        |k| k.to_string(),
        move |k| match k {
            "host" => form_field("SSH host", ssh_host, "ssh.example.com", false).into_any(),
            "port" => form_field("SSH port", ssh_port, "22", false).into_any(),
            "user" => form_field("SSH user", ssh_user, "deploy", false).into_any(),
            "auth" => stack((
                label(|| "Auth".to_string()).style(|s| s.font_size(11.0).color(theme::fg_dim())),
                stack((
                    auth_chip("key", "Public key"),
                    auth_chip("password", "Password"),
                ))
                .style(|s| s.flex_row().gap(5.0)),
            ))
            .style(|s| s.flex_col().gap(3.0))
            .into_any(),
            "sshpass" => form_field("SSH password", ssh_password, "", true).into_any(),
            "key" => form_field("Private key", ssh_key_path, "~/.ssh/id_ed25519", false).into_any(),
            "passphrase" => form_field("Passphrase", ssh_passphrase, "", true).into_any(),
            _ => empty().into_any(),
        },
    )
    .style(|s| {
        s.flex_col()
            .gap(6.0)
            .width_full()
            .padding(8.0)
            .border(1.0)
            .border_color(theme::border())
            .border_radius(8.0)
            .background(theme::bg_hover())
    });

    // Test connection + status.
    let test = label(|| "Test".to_string())
        .style(|s| {
            s.padding_horiz(12.0)
                .height(26.0)
                .items_center()
                .justify_center()
                .border_radius(5.0)
                .font_size(12.0)
                .border(1.0)
                .border_color(theme::border())
                .color(theme::fg())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::bg_hover()))
        })
        .on_click_stop(move |_| {
            sync_test();
            state.db_test_connection();
        });
    let test_status = label(move || match state.db_test_state.get().as_str() {
        "" => String::new(),
        "testing" => "testing…".to_string(),
        "ok" => "✓ OK".to_string(),
        _ => "✗ failed".to_string(),
    })
    .style(move |s| {
        let st = state.db_test_state.get();
        let s = s.font_size(11.0).items_center();
        match st.as_str() {
            "ok" => s.color(Color::from_rgb8(0x9e, 0xce, 0x6a)),
            "" | "testing" => s.color(theme::fg_dim()),
            _ => s.color(Color::from_rgb8(0xf7, 0x76, 0x8e)),
        }
    });
    let test_row = stack((test, test_status)).style(|s| s.flex_row().gap(8.0).items_center());

    let connect = label(move || {
        if state.db_editing_key.get().is_some() {
            "Save & reconnect".to_string()
        } else {
            "Connect & save".to_string()
        }
    })
    .style(|s| {
        s.width_full()
            .height(28.0)
            .items_center()
            .justify_center()
            .border_radius(5.0)
            .font_size(12.0)
            .border(1.0)
            .border_color(theme::border())
            .color(theme::fg())
            .cursor(floem::style::CursorStyle::Pointer)
            .hover(|s| s.background(theme::bg_hover()))
    })
    .on_click_stop(move |_| {
        sync();
        state.db_submit_form();
    });

    let hint = label(|| "Saved in ~/.config/e — never written to the project.".to_string())
        .style(|s| s.font_size(10.0).color(theme::fg_dim()));

    stack((
        from_env,
        label(|| "or connect manually:".to_string())
            .style(|s| s.font_size(11.0).color(theme::fg_dim())),
        engines,
        net_fields,
        ssh_toggle,
        ssh_fields,
        form_field("Group", group, "(optional)", false),
        test_row,
        connect,
        hint,
    ))
    .style(|s| {
        s.flex_col()
            .gap(8.0)
            .padding(12.0)
            .width_full()
            .border_bottom(1.0)
            .border_color(theme::border())
    })
}

pub fn database_panel(state: AppState) -> impl IntoView {
    let title = label(|| "Database".to_string()).style(|s| {
        s.flex_grow(1.0)
            .font_size(13.0)
            .font_bold()
            .color(theme::fg())
    });
    let add = label(|| "＋".to_string())
        .style(|s| {
            s.padding_horiz(8.0)
                .padding_vert(1.0)
                .border(1.0)
                .border_color(theme::border())
                .border_radius(6.0)
                .color(theme::fg_dim())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.color(theme::fg()))
        })
        .on_click_stop(move |_| state.db_adding.update(|a| *a = !*a));
    let rel = label(|| "⇄".to_string())
        .style(|s| {
            s.padding_horiz(8.0)
                .padding_vert(1.0)
                .border(1.0)
                .border_color(theme::border())
                .border_radius(6.0)
                .color(theme::fg_dim())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.color(theme::fg()))
        })
        .on_click_stop(move |_| state.db_show_erd());
    let header = stack((title, rel, add)).style(|s| {
        s.flex_row()
            .items_center()
            .gap(6.0)
            .padding_horiz(12.0)
            .padding_top(10.0)
            .padding_bottom(6.0)
            .width_full()
    });
    // Search across all tables' text columns (DB-805); Enter runs it.
    let search_all = text_input(state.db_search_query)
        .placeholder("Search all data…")
        .style(|s| {
            theme::input_colors(s)
                .width_full()
                .height(26.0)
                .font_size(11.0)
                .padding_horiz(8.0)
                .margin_horiz(12.0)
                .margin_bottom(6.0)
        })
        .on_key_down(
            floem::keyboard::Key::Named(floem::keyboard::NamedKey::Enter),
            |_| true,
            move |_| state.db_search_all(),
        );

    let form = dyn_stack(
        move || {
            if state.db_adding.get() {
                vec![0]
            } else {
                vec![]
            }
        },
        |i| *i,
        move |_| add_form(state),
    );

    let list = scroll(
        dyn_stack(
            move || state.db_conns.get(),
            |e| e.key(),
            move |e| conn_row(state, e),
        )
        .style(|s| {
            s.flex_col()
                .width_full()
                .padding_horiz(6.0)
                .padding_bottom(10.0)
        }),
    )
    .style(|s| s.flex_col().flex_grow(1.0).width_full());

    let empty_hint = label(|| "No connections yet.".to_string()).style(move |s| {
        let s = s.padding(16.0).color(theme::fg_dim()).font_size(12.0);
        if state.db_conns.with(|c| c.is_empty()) && !state.db_adding.get() {
            s
        } else {
            s.hide()
        }
    });

    stack((header, search_all, form, empty_hint, list)).style(|s| {
        s.flex_col()
            .height_full()
            .width_full()
            .background(theme::bg())
    })
}

// ---- Results overlay ------------------------------------------------------

pub fn db_result_overlay(state: AppState) -> impl IntoView {
    let title = label(move || state.db_result_title.get()).style(|s| {
        s.flex_grow(1.0)
            .font_size(13.0)
            .font_bold()
            .color(theme::fg())
    });
    let close = label(|| "✕".to_string())
        .style(|s| {
            s.padding_horiz(8.0)
                .color(theme::fg_dim())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.color(theme::fg()))
        })
        .on_click_stop(move |_| state.close_db_result());
    // Environment dot for the active connection (green/amber/red), so it's always
    // clear which database's data you're looking at (DB-104).
    let env_dot = label(|| "●".to_string()).style(move |s| {
        let env = state.db_result_key.get().and_then(|key| {
            state.db_conns.with(|c| {
                c.iter()
                    .find(|e| e.key() == key)
                    .map(|e| e.config.environment())
            })
        });
        match env {
            Some(env) => s.font_size(10.0).color(env_color(env)),
            None => s.hide(),
        }
    });
    let header = stack((env_dot, title, close)).style(|s| {
        s.flex_row()
            .items_center()
            .gap(6.0)
            .padding_horiz(12.0)
            .padding_vert(8.0)
            .width_full()
            .border_bottom(1.0)
            .border_color(theme::border())
    });

    // Query editor row: a syntax-highlighted SQL console.
    let sql = crate::db_console::sql_console(state);
    let run = label(|| "Run".to_string())
        .style(|s| {
            s.padding_horiz(18.0)
                .height(40.0)
                .items_center()
                .justify_center()
                .border_radius(5.0)
                .font_size(12.0)
                .background(theme::accent())
                .color(Color::from_rgb8(0x14, 0x16, 0x1b))
                .cursor(floem::style::CursorStyle::Pointer)
        })
        .on_click_stop(move |_| state.db_run_query());
    let history_btn = label(|| "○ History".to_string())
        .style(|s| {
            s.padding_horiz(12.0)
                .height(28.0)
                .items_center()
                .justify_center()
                .border_radius(5.0)
                .font_size(12.0)
                .border(1.0)
                .border_color(theme::border())
                .color(theme::fg())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::bg_hover()))
        })
        .on_click_stop(move |_| state.db_open_history());
    let run_col = stack((run, history_btn)).style(|s| s.flex_col().gap(6.0).flex_shrink(0.0));
    let sql_wrap = floem::views::container(sql).style(move |s| {
        s.flex_grow(1.0)
            .min_width(0.0)
            .height(state.db_console_height.get())
            .flex_shrink(0.0)
            .border(1.0)
            .border_color(theme::border())
            .border_radius(5.0)
            .background(theme::bg())
    });
    let query_row = stack((sql_wrap, run_col)).style(|s| {
        s.flex_row()
            .items_start()
            .gap(8.0)
            .padding_horiz(10.0)
            .padding_top(10.0)
            .padding_bottom(2.0)
            .width_full()
    });

    // Draggable handle to resize the console vertically (drag down for more room).
    let query_resize = {
        let drag_start: RwSignal<Option<f64>> = RwSignal::new(None);
        let v = floem::views::empty();
        let id = floem::View::id(&v);
        v.on_event_stop(floem::event::EventListener::PointerDown, move |e| {
            id.request_active();
            if let floem::event::Event::PointerDown(pe) = e {
                drag_start.set(Some(pe.pos.y));
            }
        })
        .on_event_stop(floem::event::EventListener::PointerMove, move |e| {
            if let floem::event::Event::PointerMove(pe) = e {
                if let Some(start) = drag_start.get_untracked() {
                    let delta = pe.pos.y - start;
                    let cur = state.db_console_height.get_untracked();
                    state
                        .db_console_height
                        .set((cur + delta).clamp(60.0, 600.0));
                }
            }
        })
        .on_event_stop(floem::event::EventListener::PointerUp, move |_| {
            drag_start.set(None)
        })
        .style(|s| {
            s.height(7.0)
                .width_full()
                .cursor(floem::style::CursorStyle::RowResize)
                .border_bottom(1.0)
                .border_color(theme::border())
                .hover(|s| s.background(theme::accent()))
        })
    };
    let query_row = stack((query_row, query_resize)).style(|s| s.flex_col().width_full());

    // Status / error line.
    let status = label(move || {
        if state.db_result_loading.get() {
            "Running…".to_string()
        } else if let Some(e) = state.db_result_error.get() {
            e
        } else if let Some(r) = state.db_result.get() {
            if r.is_select {
                let t = if r.truncated { " (truncated)" } else { "" };
                format!("{} rows · {} ms{}", r.rows.len(), r.elapsed_ms, t)
            } else {
                format!(
                    "{} rows affected · {} ms",
                    r.rows_affected.unwrap_or(0),
                    r.elapsed_ms
                )
            }
        } else {
            String::new()
        }
    })
    .style(move |s| {
        let s = s.padding_horiz(12.0).padding_vert(4.0).font_size(11.0);
        if state.db_result_error.get().is_some() {
            s.color(Color::from_rgb8(0xf7, 0x76, 0x8e))
        } else {
            s.color(theme::fg_dim())
        }
    });

    // "Explain with agent" appears only when the query errored.
    let explain = label(|| "✨ Explain with agent".to_string())
        .style(move |s| {
            let s = s
                .font_size(11.0)
                .padding_horiz(8.0)
                .padding_vert(2.0)
                .border_radius(4.0)
                .color(theme::accent())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::bg_hover()));
            if state.db_result_error.get().is_some() {
                s
            } else {
                s.hide()
            }
        })
        .on_click_stop(move |_| {
            if let Some(err) = state.db_result_error.get_untracked() {
                let sql = state.db_query_text.get_untracked();
                state.send_to_agent(&format!(
                    "This SQL failed with an error. Explain the error and give a corrected query.\nSQL:\n{sql}\nError:\n{err}"
                ));
            }
        });
    // Cancel button, shown only while a query is running.
    let cancel = label(|| "✕ Cancel".to_string())
        .style(move |s| {
            let s = s
                .font_size(11.0)
                .padding_horiz(8.0)
                .padding_vert(2.0)
                .border_radius(4.0)
                .color(Color::from_rgb8(0xe0, 0x6c, 0x75))
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::bg_hover()));
            if state.db_result_loading.get() {
                s
            } else {
                s.hide()
            }
        })
        .on_click_stop(move |_| state.db_cancel_query());
    let status = stack((status, explain, cancel)).style(|s| s.flex_row().items_center().gap(8.0));

    // Toolbar: Data/Structure (table mode), pagination, export.
    let chip = move |id: &'static str, name: &'static str| {
        label(move || name.to_string())
            .style(move |s| {
                let active = state.db_subview.get() == id;
                let s = s
                    .padding_horiz(10.0)
                    .padding_vert(2.0)
                    .border_radius(5.0)
                    .font_size(11.0)
                    .cursor(floem::style::CursorStyle::Pointer);
                if active {
                    s.background(theme::accent())
                        .color(Color::from_rgb8(0x14, 0x16, 0x1b))
                } else {
                    s.color(theme::fg_dim()).hover(|s| s.color(theme::fg()))
                }
            })
            .on_click_stop(move |_| state.db_set_subview(id))
    };
    let subview_chips =
        stack((chip("data", "Data"), chip("structure", "Structure"))).style(move |s| {
            let s = s.flex_row().gap(4.0).items_center();
            if state.db_result_table.get().is_some() {
                s
            } else {
                s.hide()
            }
        });

    let toolbar_btn = |glyph: &'static str, on: Box<dyn Fn()>| {
        label(move || glyph.to_string())
            .style(|s| {
                s.padding_horiz(8.0)
                    .padding_vert(2.0)
                    .border_radius(4.0)
                    .font_size(12.0)
                    .color(theme::fg_dim())
                    .cursor(floem::style::CursorStyle::Pointer)
                    .hover(|s| s.background(theme::bg_hover()).color(theme::fg()))
            })
            .on_click_stop(move |_| on())
    };
    let prev = toolbar_btn("‹ Prev", Box::new(move || state.db_page_by(-1)));
    let next = toolbar_btn("Next ›", Box::new(move || state.db_page_by(1)));
    let page_lbl = label(move || {
        let page = state.db_page.get() + 1;
        match state.db_total_rows.get() {
            Some(total) => {
                let pages = (total.max(1) as usize).div_ceil(crate::db_state::DB_PAGE);
                format!("p{page}/{pages} · {total} rows")
            }
            None => format!("p{page}"),
        }
    })
    .style(|s| s.font_size(11.0).color(theme::fg_dim()).items_center());
    // Jump-to-page: type a page number and press Enter.
    let jump = create_rw_signal(String::new());
    let jump_box = text_input(jump)
        .placeholder("#")
        .style(|s| {
            theme::input_colors(s)
                .width(44.0)
                .height(22.0)
                .font_size(11.0)
                .padding_horiz(6.0)
        })
        .on_key_down(
            floem::keyboard::Key::Named(floem::keyboard::NamedKey::Enter),
            |_| true,
            move |_| {
                if let Ok(n) = jump.get_untracked().trim().parse::<usize>() {
                    if n >= 1 {
                        state.db_goto_page(n - 1);
                    }
                }
                jump.set(String::new());
            },
        );
    let pager = stack((prev, page_lbl, next, jump_box)).style(move |s| {
        let s = s.flex_row().gap(4.0).items_center();
        if state.db_result_table.get().is_some() && state.db_subview.get() == "data" {
            s
        } else {
            s.hide()
        }
    });

    let filter_chip = label(move || {
        state
            .db_filter
            .get()
            .map(|(c, v)| match v {
                Some(v) => format!("⚑ {c} = {v}  ✕"),
                None => format!("⚑ {c} IS NULL  ✕"),
            })
            .unwrap_or_default()
    })
    .style(move |s| {
        let s = s
            .padding_horiz(8.0)
            .padding_vert(2.0)
            .border_radius(5.0)
            .font_size(11.0)
            .background(theme::bg_hover())
            .color(theme::fg())
            .cursor(floem::style::CursorStyle::Pointer)
            .hover(|s| s.color(theme::fg_dim()));
        if state.db_filter.get().is_some() && state.db_subview.get() == "data" {
            s
        } else {
            s.hide()
        }
    })
    .on_click_stop(move |_| state.db_clear_filter());

    let spacer = empty().style(|s| s.flex_grow(1.0));

    // Saved queries: a popout menu to load them, and a save button + name input.
    let saved_menu = label(|| "Saved ▾".to_string())
        .style(|s| {
            s.padding_horiz(8.0)
                .padding_vert(2.0)
                .border_radius(4.0)
                .font_size(11.0)
                .color(theme::fg_dim())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::bg_hover()).color(theme::fg()))
        })
        .popout_menu(move || {
            use floem::menu::{Menu, MenuItem};
            let queries = state.db_queries.get_untracked();
            if queries.is_empty() {
                return Menu::new("").entry(MenuItem::new("(no saved queries)"));
            }
            let mut menu = Menu::new("");
            for q in queries {
                let sql = q.sql.clone();
                let name = q.name.clone();
                let del_name = q.name.clone();
                menu = menu.entry(
                    MenuItem::new(q.name.clone()).action(move || state.db_load_query(sql.clone())),
                );
                let _ = (name, del_name);
            }
            menu
        });
    let save_btn = toolbar_btn(
        "💾",
        Box::new(move || {
            state.db_query_name.set(String::new());
            state.db_saving_query.set(true);
        }),
    );
    let name_input = text_input(state.db_query_name)
        .placeholder("query name — ↵ to save")
        .style(move |s| {
            let s = theme::input_colors(s)
                .width(180.0)
                .font_size(11.0)
                .padding_horiz(6.0)
                .padding_vert(2.0);
            if state.db_saving_query.get() {
                s
            } else {
                s.width(0.0).hide()
            }
        })
        .on_key_down(
            floem::keyboard::Key::Named(floem::keyboard::NamedKey::Enter),
            |_| true,
            move |_| state.db_save_query(),
        )
        .on_key_down(
            floem::keyboard::Key::Named(floem::keyboard::NamedKey::Escape),
            |_| true,
            move |_| state.db_saving_query.set(false),
        )
        .request_focus(move || {
            state.db_saving_query.get();
        });
    let export_csv = toolbar_btn(
        "CSV",
        Box::new(move || state.db_export(crate::db_export::Format::Csv)),
    );
    let export_json = toolbar_btn(
        "JSON",
        Box::new(move || state.db_export(crate::db_export::Format::Json)),
    );
    let export_sql = toolbar_btn(
        "SQL",
        Box::new(move || state.db_export(crate::db_export::Format::SqlInserts)),
    );
    let export = stack((
        label(|| "⬇".to_string()).style(|s| s.font_size(11.0).color(theme::fg_dim())),
        export_csv,
        export_json,
        export_sql,
    ))
    .style(|s| s.flex_row().items_center().gap(2.0));
    // Copy to clipboard: TSV (spreadsheet-friendly) or a Markdown table.
    let copy_tsv = toolbar_btn(
        "TSV",
        Box::new(move || state.db_copy_result(crate::db_export::Format::Tsv)),
    );
    let copy_md = toolbar_btn(
        "MD",
        Box::new(move || state.db_copy_result(crate::db_export::Format::Markdown)),
    );
    let copy = stack((
        label(|| "⧉".to_string()).style(|s| s.font_size(11.0).color(theme::fg_dim())),
        copy_tsv,
        copy_md,
    ))
    .style(|s| s.flex_row().items_center().gap(2.0));
    let write_log = toolbar_btn("Log", Box::new(move || state.db_open_write_log()));
    let add_row = label(|| "+ Row".to_string())
        .style(move |s| {
            let s = s
                .padding_horiz(8.0)
                .padding_vert(2.0)
                .border_radius(4.0)
                .font_size(12.0)
                .color(theme::fg_dim())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::bg_hover()).color(theme::fg()));
            if state.db_result_table.get().is_some() && state.db_subview.get() == "data" {
                s
            } else {
                s.hide()
            }
        })
        .on_click_stop(move |_| state.db_begin_insert());
    let import_csv = label(|| "⬆ CSV".to_string())
        .style(move |s| {
            let s = s
                .padding_horiz(8.0)
                .padding_vert(2.0)
                .border_radius(4.0)
                .font_size(12.0)
                .color(theme::fg_dim())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::bg_hover()).color(theme::fg()));
            if state.db_result_table.get().is_some() && state.db_subview.get() == "data" {
                s
            } else {
                s.hide()
            }
        })
        .on_click_stop(move |_| state.db_import_csv());
    let seed = label(|| "Seed 10".to_string())
        .style(move |s| {
            let s = s
                .padding_horiz(8.0)
                .padding_vert(2.0)
                .border_radius(4.0)
                .font_size(12.0)
                .color(theme::fg_dim())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::bg_hover()).color(theme::fg()));
            // Local tables only (factory seeding runs through Tinker/the app DB).
            let local = state.db_result_key.get().and_then(|key| {
                state.db_conns.with(|c| {
                    c.iter()
                        .find(|e| e.key() == key)
                        .map(|e| e.config.environment().is_local())
                })
            });
            if local == Some(true) && state.db_subview.get() == "data" {
                s
            } else {
                s.hide()
            }
        })
        .on_click_stop(move |_| state.db_seed_table(10));

    let toolbar = stack((
        subview_chips,
        pager,
        filter_chip,
        spacer,
        add_row,
        import_csv,
        seed,
        saved_menu,
        name_input,
        save_btn,
        write_log,
        copy,
        export,
    ))
    .style(|s| {
        s.flex_row()
            .items_center()
            .gap(10.0)
            .padding_horiz(12.0)
            .padding_vert(5.0)
            .width_full()
            .border_bottom(1.0)
            .border_color(theme::border())
    });

    let grid = scroll(
        dyn_container(
            move || state.db_subview.get(),
            move |v| {
                if v == "structure" {
                    structure_grid(state).into_any()
                } else {
                    result_grid(state).into_any()
                }
            },
        )
        .style(move |s| {
            let w = if state.db_subview.get() == "structure" {
                520.0
            } else {
                let n = state
                    .db_result
                    .with(|r| r.as_ref().map(|r| r.columns.len()).unwrap_or(0));
                (n.max(1) as f64) * 180.0
            };
            s.items_start().width(w)
        }),
    )
    .scroll_delta(move || {
        let (x, y, _) = state.db_scroll.get();
        floem::kurbo::Vec2::new(x, y)
    })
    .style(|s| s.flex_grow(1.0).width_full())
    .keyboard_navigable()
    .request_focus(move || {
        state.db_result_open.get();
    })
    .on_key_down(
        floem::keyboard::Key::Named(floem::keyboard::NamedKey::ArrowRight),
        |_| true,
        move |_| db_scroll(state, 90.0, 0.0),
    )
    .on_key_down(
        floem::keyboard::Key::Named(floem::keyboard::NamedKey::ArrowLeft),
        |_| true,
        move |_| db_scroll(state, -90.0, 0.0),
    )
    .on_key_down(
        floem::keyboard::Key::Named(floem::keyboard::NamedKey::ArrowDown),
        |_| true,
        move |_| db_scroll(state, 0.0, 60.0),
    )
    .on_key_down(
        floem::keyboard::Key::Named(floem::keyboard::NamedKey::ArrowUp),
        |_| true,
        move |_| db_scroll(state, 0.0, -60.0),
    );

    stack((
        header,
        query_row,
        toolbar,
        result_tabs_strip(state),
        explain_banner(state),
        status,
        grid,
        pending_bar(state),
        db_value_viewer(state),
        db_edit_popup(state),
        db_insert_popup(state),
        db_history_panel(state),
        db_write_log_panel(state),
    ))
    .style(move |s| {
        let s = s
            .absolute()
            .inset(0.0)
            .size_full()
            .flex_col()
            .background(theme::bg());
        if state.db_result_open.get() {
            s
        } else {
            s.hide()
        }
    })
}

/// The consent dialog shown when the AI agent proposes a query to run.
/// The schema-relationships (ERD) panel: every foreign key in the database as
/// `table.column → ref_table.ref_column`, grouped by source table (DB-207).
pub fn db_erd_panel(state: AppState) -> impl IntoView {
    let close = label(|| "✕".to_string())
        .style(|s| {
            s.padding_horiz(8.0)
                .color(theme::fg_dim())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.color(theme::fg()))
        })
        .on_click_stop(move |_| state.db_erd_open.set(false));
    let header = stack((
        label(|| "Schema relationships".to_string()).style(|s| {
            s.flex_grow(1.0)
                .font_size(13.0)
                .font_bold()
                .color(theme::fg())
        }),
        close,
    ))
    .style(|s| {
        s.flex_row()
            .items_center()
            .width_full()
            .padding(10.0)
            .border_bottom(1.0)
            .border_color(theme::border())
    });
    let rows = dyn_stack(
        move || {
            let mut fks = state.db_erd.get();
            fks.sort_by(|a, b| a.table.cmp(&b.table).then(a.column.cmp(&b.column)));
            fks.into_iter().enumerate().collect::<Vec<_>>()
        },
        |(i, _)| *i,
        move |(_, fk)| {
            let text = format!(
                "{}.{}  →  {}.{}",
                fk.table, fk.column, fk.ref_table, fk.ref_column
            );
            label(move || text.clone()).style(|s| {
                s.font_family("monospace".to_string())
                    .font_size(12.0)
                    .color(theme::fg())
                    .width_full()
                    .padding_horiz(12.0)
                    .padding_vert(4.0)
                    .text_ellipsis()
            })
        },
    )
    .style(|s| s.flex_col().width_full());
    let empty_hint = label(|| "No foreign keys in this database.".to_string()).style(move |s| {
        let s = s.padding(16.0).font_size(12.0).color(theme::fg_dim());
        if state.db_erd.with(|f| f.is_empty()) {
            s
        } else {
            s.hide()
        }
    });
    let list = scroll(stack((rows, empty_hint)).style(|s| s.flex_col().width_full()))
        .style(|s| s.flex_grow(1.0).width_full());
    let card = stack((header, list)).style(|s| {
        s.flex_col()
            .width(620.0)
            .height(460.0)
            .border(1.0)
            .border_color(theme::border())
            .border_radius(10.0)
            .background(theme::bg())
    });
    container(card).style(move |s| {
        let s = s
            .absolute()
            .inset(0.0)
            .size_full()
            .items_center()
            .justify_center()
            .background(Color::from_rgba8(0, 0, 0, 120));
        if state.db_erd_open.get() {
            s
        } else {
            s.hide()
        }
    })
}

/// Prompt for `:param` values before running a console query (DB-408).
pub fn db_params_dialog(state: AppState) -> impl IntoView {
    let fields = dyn_stack(
        move || {
            state
                .db_params
                .get()
                .map(|p| p.fields)
                .unwrap_or_default()
                .into_iter()
                .enumerate()
                .collect::<Vec<_>>()
        },
        |(i, _)| *i,
        move |(_, (name, sig))| {
            let label_name = name.clone();
            stack((
                label(move || format!(":{label_name}")).style(|s| {
                    s.width(120.0)
                        .flex_shrink(0.0)
                        .font_family("monospace".to_string())
                        .font_size(12.0)
                        .color(theme::accent())
                }),
                text_input(sig)
                    .style(|s| {
                        theme::input_colors(s)
                            .flex_grow(1.0)
                            .min_width(0.0)
                            .height(28.0)
                            .font_size(12.0)
                            .padding_horiz(8.0)
                    })
                    .on_key_down(
                        floem::keyboard::Key::Named(floem::keyboard::NamedKey::Enter),
                        |_| true,
                        move |_| state.db_params_run(),
                    ),
            ))
            .style(|s| {
                s.flex_row()
                    .items_center()
                    .gap(8.0)
                    .width_full()
                    .margin_bottom(6.0)
            })
        },
    )
    .style(|s| s.flex_col().width_full());

    let cancel = label(|| "Cancel".to_string())
        .style(|s| {
            s.padding_horiz(14.0)
                .height(28.0)
                .items_center()
                .border_radius(5.0)
                .font_size(12.0)
                .border(1.0)
                .border_color(theme::border())
                .color(theme::fg())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::bg_hover()))
        })
        .on_click_stop(move |_| state.db_params_cancel());
    let run = label(|| "Run".to_string())
        .style(|s| {
            s.padding_horiz(14.0)
                .height(28.0)
                .items_center()
                .border_radius(5.0)
                .font_size(12.0)
                .background(theme::accent())
                .color(Color::from_rgb8(0x14, 0x16, 0x1b))
                .cursor(floem::style::CursorStyle::Pointer)
        })
        .on_click_stop(move |_| state.db_params_run());
    let buttons = stack((empty().style(|s| s.flex_grow(1.0)), cancel, run)).style(|s| {
        s.flex_row()
            .items_center()
            .gap(8.0)
            .width_full()
            .margin_top(12.0)
    });

    let card = stack((
        label(|| "Query parameters".to_string()).style(|s| {
            s.font_size(14.0)
                .font_bold()
                .color(theme::fg())
                .margin_bottom(10.0)
        }),
        fields,
        buttons,
    ))
    .style(|s| {
        s.flex_col()
            .width(460.0)
            .padding(16.0)
            .border(1.0)
            .border_color(theme::border())
            .border_radius(10.0)
            .background(theme::bg_panel())
    });
    container(card).style(move |s| {
        let s = s
            .absolute()
            .inset(0.0)
            .size_full()
            .items_center()
            .justify_center()
            .background(Color::from_rgba8(0, 0, 0, 120));
        if state.db_params.get().is_some() {
            s
        } else {
            s.hide()
        }
    })
}

/// Confirmation dialog for destructive or non-local console runs (DB-702/703).
pub fn db_confirm_dialog(state: AppState) -> impl IntoView {
    let title = label(move || match state.db_confirm.get() {
        Some(c) => format!("{} on {}?", c.verb, c.env.label().to_uppercase()),
        None => String::new(),
    })
    .style(move |s| {
        let color = state
            .db_confirm
            .get()
            .map(|c| env_color(c.env))
            .unwrap_or(theme::fg());
        s.font_size(15.0)
            .font_bold()
            .color(color)
            .margin_bottom(6.0)
    });
    let subtitle = label(move || match state.db_confirm.get() {
        Some(c) => format!(
            "{} statement(s) will run on this database. Review them:",
            c.statements.len()
        ),
        None => String::new(),
    })
    .style(|s| s.font_size(12.0).color(theme::fg_dim()).margin_bottom(10.0));

    let list = dyn_stack(
        move || {
            state
                .db_confirm
                .get()
                .map(|c| c.statements)
                .unwrap_or_default()
                .into_iter()
                .enumerate()
                .collect::<Vec<_>>()
        },
        |(i, _)| *i,
        move |(_, stmt)| {
            label(move || stmt.clone()).style(|s| {
                s.font_family("monospace".to_string())
                    .font_size(12.0)
                    .color(Color::from_rgb8(0xe0, 0x6c, 0x75))
                    .padding_vert(2.0)
                    .width_full()
                    .text_ellipsis()
            })
        },
    )
    .style(|s| s.flex_col().width_full());
    let list = scroll(list).style(|s| {
        s.max_height(180.0)
            .width_full()
            .padding(8.0)
            .border(1.0)
            .border_color(theme::border())
            .border_radius(6.0)
            .background(theme::bg())
    });

    let ack = label(move || match state.db_confirm.get() {
        Some(c) => format!(
            "{}  I understand this affects {}",
            if c.ack.get() { "☑" } else { "☐" },
            c.env.label()
        ),
        None => String::new(),
    })
    .style(move |s| {
        let s = s
            .font_size(12.0)
            .color(theme::fg())
            .margin_top(10.0)
            .cursor(floem::style::CursorStyle::Pointer);
        if state
            .db_confirm
            .with(|c| c.as_ref().map(|c| c.needs_ack).unwrap_or(false))
        {
            s
        } else {
            s.hide()
        }
    })
    .on_click_stop(move |_| {
        if let Some(c) = state.db_confirm.get_untracked() {
            c.ack.update(|a| *a = !*a);
        }
    });

    let cancel = label(|| "Cancel".to_string())
        .style(|s| {
            s.padding_horiz(14.0)
                .height(30.0)
                .items_center()
                .border_radius(5.0)
                .font_size(12.0)
                .border(1.0)
                .border_color(theme::border())
                .color(theme::fg())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::bg_hover()))
        })
        .on_click_stop(move |_| state.db_confirm_cancel());
    let confirm = label(move || match state.db_confirm.get() {
        Some(c) => format!("{} {} statement(s)", c.verb, c.statements.len()),
        None => "Run".to_string(),
    })
    .style(move |s| {
        let color = state
            .db_confirm
            .get()
            .map(|c| env_color(c.env))
            .unwrap_or(theme::accent());
        s.padding_horiz(14.0)
            .height(30.0)
            .items_center()
            .border_radius(5.0)
            .font_size(12.0)
            .background(color)
            .color(Color::from_rgb8(0x14, 0x16, 0x1b))
            .cursor(floem::style::CursorStyle::Pointer)
    })
    .on_click_stop(move |_| state.db_confirm_run());
    let buttons = stack((empty().style(|s| s.flex_grow(1.0)), cancel, confirm)).style(|s| {
        s.flex_row()
            .items_center()
            .gap(8.0)
            .width_full()
            .margin_top(12.0)
    });

    let card = stack((title, subtitle, list, ack, buttons)).style(|s| {
        s.flex_col()
            .width(560.0)
            .padding(16.0)
            .border(1.0)
            .border_color(theme::border())
            .border_radius(10.0)
            .background(theme::bg_panel())
    });
    container(card).style(move |s| {
        let s = s
            .absolute()
            .inset(0.0)
            .size_full()
            .items_center()
            .justify_center()
            .background(Color::from_rgba8(0, 0, 0, 130));
        if state.db_confirm.get().is_some() {
            s
        } else {
            s.hide()
        }
    })
}

pub fn db_consent_dialog(state: AppState) -> impl IntoView {
    let title = label(|| "Agent wants to run a query".to_string())
        .style(|s| s.font_size(14.0).font_bold().color(theme::fg()));
    let subtitle = label(move || match state.db_consent.get() {
        Some(c) => format!("on “{}” — allow?", c.db_name),
        None => String::new(),
    })
    .style(|s| s.font_size(12.0).color(theme::fg_dim()).margin_bottom(8.0));

    let sql = label(move || state.db_consent.get().map(|c| c.sql).unwrap_or_default()).style(|s| {
        theme::input_colors(s)
            .width_full()
            .font_family("monospace".to_string())
            .font_size(12.0)
            .padding(10.0)
            .margin_bottom(12.0)
    });

    let deny = label(|| "Deny".to_string())
        .style(|s| {
            s.padding_horiz(16.0)
                .height(30.0)
                .items_center()
                .border_radius(5.0)
                .font_size(12.0)
                .border(1.0)
                .border_color(theme::border())
                .color(theme::fg())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::bg_hover()))
        })
        .on_click_stop(move |_| state.db_consent_deny());
    let allow = label(|| "Allow & run".to_string())
        .style(|s| {
            s.padding_horiz(16.0)
                .height(30.0)
                .items_center()
                .border_radius(5.0)
                .font_size(12.0)
                .background(theme::accent())
                .color(Color::from_rgb8(0x14, 0x16, 0x1b))
                .cursor(floem::style::CursorStyle::Pointer)
        })
        .on_click_stop(move |_| state.db_consent_allow());
    let buttons = stack((empty().style(|s| s.flex_grow(1.0)), deny, allow))
        .style(|s| s.flex_row().gap(8.0).items_center().width_full());

    let card = stack((title, subtitle, sql, buttons)).style(|s| {
        s.flex_col()
            .width(520.0)
            .padding(18.0)
            .border(1.0)
            .border_color(theme::border())
            .border_radius(10.0)
            .background(theme::bg())
    });

    container(card).style(move |s| {
        let s = s
            .absolute()
            .inset(0.0)
            .size_full()
            .items_center()
            .justify_center()
            .background(Color::from_rgba8(0, 0, 0, 140));
        if state.db_consent.get().is_some() {
            s
        } else {
            s.hide()
        }
    })
}

/// The inline cell-edit popup (double-click a cell in a browsed table).
fn db_edit_popup(state: AppState) -> impl IntoView {
    let title = label(move || match state.db_edit.get() {
        Some((_, _, col)) => format!("Edit  {col}"),
        None => String::new(),
    })
    .style(|s| {
        s.font_size(13.0)
            .font_bold()
            .color(theme::fg())
            .margin_bottom(8.0)
    });

    let input = text_input(state.db_edit_value)
        .placeholder("value")
        .style(move |s| {
            let s = theme::input_colors(s)
                .width_full()
                .min_height(34.0)
                .font_family("monospace".to_string())
                .font_size(13.0)
                .padding_horiz(8.0)
                .padding_vert(6.0);
            if state.db_edit_null.get() {
                s.color(theme::fg_dim())
            } else {
                s
            }
        })
        .on_key_down(
            floem::keyboard::Key::Named(floem::keyboard::NamedKey::Enter),
            |m| m.meta() || m.control(),
            move |_| state.db_commit_edit(),
        )
        .on_key_down(
            floem::keyboard::Key::Named(floem::keyboard::NamedKey::Escape),
            |_| true,
            move |_| state.db_cancel_edit(),
        )
        .request_focus(move || {
            state.db_edit.get();
        });

    let null_toggle = label(move || {
        if state.db_edit_null.get() {
            "☑ NULL".to_string()
        } else {
            "☐ NULL".to_string()
        }
    })
    .style(|s| {
        s.font_size(12.0)
            .color(theme::fg_dim())
            .cursor(floem::style::CursorStyle::Pointer)
            .hover(|s| s.color(theme::fg()))
    })
    .on_click_stop(move |_| state.db_edit_null.update(|n| *n = !*n));

    let save = label(|| "Save  ⌘↵".to_string())
        .style(|s| {
            s.padding_horiz(14.0)
                .height(28.0)
                .items_center()
                .border_radius(5.0)
                .font_size(12.0)
                .background(theme::accent())
                .color(Color::from_rgb8(0x14, 0x16, 0x1b))
                .cursor(floem::style::CursorStyle::Pointer)
        })
        .on_click_stop(move |_| state.db_commit_edit());
    let cancel = label(|| "Cancel".to_string())
        .style(|s| {
            s.padding_horiz(14.0)
                .height(28.0)
                .items_center()
                .border_radius(5.0)
                .font_size(12.0)
                .border(1.0)
                .border_color(theme::border())
                .color(theme::fg())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::bg_hover()))
        })
        .on_click_stop(move |_| state.db_cancel_edit());

    let delete_row = label(|| "Delete row".to_string())
        .style(|s| {
            s.padding_horiz(12.0)
                .height(28.0)
                .items_center()
                .border_radius(5.0)
                .font_size(12.0)
                .border(1.0)
                .border_color(Color::from_rgb8(0x6b, 0x2b, 0x2b))
                .color(Color::from_rgb8(0xe0, 0x6c, 0x75))
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(Color::from_rgba8(0xe0, 0x6c, 0x75, 30)))
        })
        .on_click_stop(move |_| state.db_delete_row());
    let follow_fk = label(|| "Follow FK →".to_string())
        .style(|s| {
            s.padding_horiz(12.0)
                .height(28.0)
                .items_center()
                .border_radius(5.0)
                .font_size(12.0)
                .border(1.0)
                .border_color(theme::border())
                .color(theme::fg())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::bg_hover()))
        })
        .on_click_stop(move |_| state.db_hop_fk());
    let related = label(|| "Related →".to_string())
        .style(|s| {
            s.padding_horiz(12.0)
                .height(28.0)
                .items_center()
                .border_radius(5.0)
                .font_size(12.0)
                .border(1.0)
                .border_color(theme::border())
                .color(theme::fg())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::bg_hover()))
        })
        .on_click_stop(move |_| state.db_show_related());
    let filter_to = label(|| "Filter to value".to_string())
        .style(|s| {
            s.padding_horiz(12.0)
                .height(28.0)
                .items_center()
                .border_radius(5.0)
                .font_size(12.0)
                .border(1.0)
                .border_color(theme::border())
                .color(theme::fg())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::bg_hover()))
        })
        .on_click_stop(move |_| state.db_filter_to_cell());
    let row_actions = stack((delete_row, follow_fk, related, filter_to)).style(|s| {
        s.flex_row()
            .flex_wrap(floem::taffy::style::FlexWrap::Wrap)
            .items_center()
            .gap(8.0)
            .width_full()
            .margin_top(10.0)
    });

    let buttons = stack((
        null_toggle,
        empty().style(|s| s.flex_grow(1.0)),
        cancel,
        save,
    ))
    .style(|s| {
        s.flex_row()
            .items_center()
            .gap(8.0)
            .width_full()
            .margin_top(12.0)
    });

    let card = stack((title, input, row_actions, buttons)).style(|s| {
        s.flex_col()
            .width(420.0)
            .padding(16.0)
            .border(1.0)
            .border_color(theme::border())
            .border_radius(10.0)
            .background(theme::bg())
    });

    container(card).style(move |s| {
        let s = s
            .absolute()
            .inset(0.0)
            .size_full()
            .items_center()
            .justify_center()
            .background(Color::from_rgba8(0, 0, 0, 120));
        if state.db_edit.get().is_some() {
            s
        } else {
            s.hide()
        }
    })
}

/// The "insert row" dialog: one labelled input per column + a NULL toggle.
fn db_insert_popup(state: AppState) -> impl IntoView {
    let title = label(move || {
        format!(
            "Insert row · {}",
            state.db_result_table.get().unwrap_or_default()
        )
    })
    .style(|s| {
        s.font_size(13.0)
            .font_bold()
            .color(theme::fg())
            .margin_bottom(10.0)
    });

    let fields = dyn_stack(
        move || state.db_insert_fields.get(),
        |f| f.name.clone(),
        move |f| {
            let name = f.name.clone();
            let dt = f.data_type.clone();
            let nullable = f.nullable;
            let val = f.value;
            let is_null = f.is_null;
            let name_lbl = label(move || format!("{name}  {dt}")).style(|s| {
                s.width(150.0)
                    .flex_shrink(0.0)
                    .font_size(11.0)
                    .text_ellipsis()
                    .color(theme::fg_dim())
            });
            let input = text_input(val).placeholder("value").style(move |s| {
                let s = theme::input_colors(s)
                    .flex_grow(1.0)
                    .min_width(0.0)
                    .height(30.0)
                    .font_size(12.0)
                    .padding_horiz(8.0);
                if is_null.get() {
                    s.color(theme::fg_dim())
                } else {
                    s
                }
            });
            let null_toggle = label(move || {
                if is_null.get() {
                    "☑ NULL".to_string()
                } else {
                    "☐ NULL".to_string()
                }
            })
            .style(move |s| {
                let s = s
                    .width(64.0)
                    .flex_shrink(0.0)
                    .font_size(11.0)
                    .cursor(floem::style::CursorStyle::Pointer);
                if nullable {
                    s.color(theme::fg_dim()).hover(|s| s.color(theme::fg()))
                } else {
                    s.color(theme::border())
                }
            })
            .on_click_stop(move |_| {
                if nullable {
                    is_null.update(|n| *n = !*n);
                }
            });
            stack((name_lbl, input, null_toggle)).style(|s| {
                s.flex_row()
                    .items_center()
                    .gap(8.0)
                    .width_full()
                    .margin_bottom(6.0)
            })
        },
    )
    .style(|s| s.flex_col().width_full());
    let fields = scroll(fields).style(|s| s.width_full().max_height(360.0));

    let cancel = label(|| "Cancel".to_string())
        .style(|s| {
            s.padding_horiz(14.0)
                .height(28.0)
                .items_center()
                .border_radius(5.0)
                .font_size(12.0)
                .border(1.0)
                .border_color(theme::border())
                .color(theme::fg())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::bg_hover()))
        })
        .on_click_stop(move |_| state.db_cancel_insert());
    let save = label(|| "Insert".to_string())
        .style(|s| {
            s.padding_horiz(14.0)
                .height(28.0)
                .items_center()
                .border_radius(5.0)
                .font_size(12.0)
                .background(theme::accent())
                .color(Color::from_rgb8(0x14, 0x16, 0x1b))
                .cursor(floem::style::CursorStyle::Pointer)
        })
        .on_click_stop(move |_| state.db_commit_insert());
    let buttons = stack((empty().style(|s| s.flex_grow(1.0)), cancel, save)).style(|s| {
        s.flex_row()
            .items_center()
            .gap(8.0)
            .width_full()
            .margin_top(12.0)
    });

    let card = stack((title, fields, buttons)).style(|s| {
        s.flex_col()
            .width(520.0)
            .padding(16.0)
            .border(1.0)
            .border_color(theme::border())
            .border_radius(10.0)
            .background(theme::bg())
    });

    container(card).style(move |s| {
        let s = s
            .absolute()
            .inset(0.0)
            .size_full()
            .items_center()
            .justify_center()
            .background(Color::from_rgba8(0, 0, 0, 120));
        if state.db_insert_open.get() {
            s
        } else {
            s.hide()
        }
    })
}

/// Pretty-print a cell value for the viewer: JSON objects/arrays are
/// re-indented; everything else is shown verbatim.
pub(crate) fn pretty_value(s: &str) -> String {
    let t = s.trim_start();
    if t.starts_with('{') || t.starts_with('[') {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(s) {
            if let Ok(p) = serde_json::to_string_pretty(&v) {
                return p;
            }
        }
    }
    s.to_string()
}

/// A read-only inspector for the selected cell (full value, pretty-printed JSON)
/// docked at the bottom of the result panel — like PhpStorm's value viewer.
fn db_value_viewer(state: AppState) -> impl IntoView {
    let head = label(move || match state.db_selected_cell.get() {
        Some((r, c)) => {
            let col = state.db_result.with(|res| {
                res.as_ref()
                    .and_then(|res| res.columns.get(c).cloned())
                    .unwrap_or_default()
            });
            format!("{col}  ·  row {}", r + 1)
        }
        None => String::new(),
    })
    .style(|s| {
        s.font_size(11.0)
            .font_bold()
            .color(theme::fg_dim())
            .padding_horiz(10.0)
            .padding_vert(5.0)
            .border_bottom(1.0)
            .border_color(theme::border())
    });

    let body = label(move || match state.db_selected_cell.get() {
        Some((r, c)) => state.db_result.with(|res| {
            res.as_ref()
                .and_then(|res| res.rows.get(r).and_then(|row| row.get(c)))
                .map(|cell| match cell {
                    Some(v) => pretty_value(v),
                    None => "NULL".to_string(),
                })
                .unwrap_or_default()
        }),
        None => String::new(),
    })
    .style(|s| {
        s.font_family("monospace".to_string())
            .font_size(12.0)
            .color(theme::fg())
            .padding(10.0)
    });
    let body = scroll(body).style(|s| s.flex_grow(1.0).width_full());

    let close = label(|| "✕".to_string())
        .style(|s| {
            s.padding_horiz(8.0)
                .color(theme::fg_dim())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.color(theme::fg()))
        })
        .on_click_stop(move |_| state.db_selected_cell.set(None));
    let head_row = stack((head.style(|s| s.flex_grow(1.0)), close)).style(|s| {
        s.flex_row()
            .items_center()
            .width_full()
            .border_bottom(1.0)
            .border_color(theme::border())
    });

    stack((head_row, body)).style(move |s| {
        let s = s
            .flex_col()
            .width_full()
            .height(160.0)
            .flex_shrink(0.0)
            .border_top(1.0)
            .border_color(theme::border())
            .background(theme::bg_panel());
        if state.db_selected_cell.get().is_some() {
            s
        } else {
            s.hide()
        }
    })
}

/// A compact "N min ago" for an epoch-ms timestamp.
fn rel_ago(ts_ms: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(ts_ms);
    let secs = ((now - ts_ms) / 1000).max(0);
    if secs < 60 {
        "just now".to_string()
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86_400)
    }
}

/// The session write-log (undo-log): every write executed this session, with a
/// generated reverse where possible (Undo runs it). (DB-705)
fn db_write_log_panel(state: AppState) -> impl IntoView {
    let close = label(|| "✕".to_string())
        .style(|s| {
            s.padding_horiz(8.0)
                .color(theme::fg_dim())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.color(theme::fg()))
        })
        .on_click_stop(move |_| state.db_write_log_open.set(false));
    let header = stack((
        label(|| "Session write log".to_string()).style(|s| {
            s.flex_grow(1.0)
                .font_size(13.0)
                .font_bold()
                .color(theme::fg())
        }),
        close,
    ))
    .style(|s| {
        s.flex_row()
            .items_center()
            .width_full()
            .padding(10.0)
            .border_bottom(1.0)
            .border_color(theme::border())
    });
    let rows = dyn_stack(
        move || {
            // Newest first.
            let mut v = state.db_write_log.get();
            v.reverse();
            v.into_iter().enumerate().collect::<Vec<_>>()
        },
        |(i, _)| *i,
        move |(_, e)| {
            let fwd = e.forward.clone();
            let rev = e.reverse.clone();
            let has_rev = rev.is_some();
            let undo = label(|| "Undo".to_string())
                .style(move |s| {
                    let s = s
                        .font_size(10.5)
                        .flex_shrink(0.0)
                        .padding_horiz(8.0)
                        .border_radius(4.0)
                        .border(1.0)
                        .border_color(theme::border())
                        .color(theme::accent())
                        .cursor(floem::style::CursorStyle::Pointer)
                        .hover(|s| s.background(theme::bg_hover()));
                    if has_rev {
                        s
                    } else {
                        s.hide()
                    }
                })
                .on_click_stop(move |_| {
                    if let Some(r) = &rev {
                        state.db_undo_write(r.clone());
                    }
                });
            stack((
                label(move || fwd.clone()).style(|s| {
                    s.flex_grow(1.0)
                        .font_family("monospace".to_string())
                        .font_size(12.0)
                        .color(theme::fg())
                        .text_ellipsis()
                }),
                undo,
            ))
            .style(|s| {
                s.flex_row()
                    .items_center()
                    .gap(8.0)
                    .width_full()
                    .padding_horiz(10.0)
                    .padding_vert(5.0)
                    .border_bottom(1.0)
                    .border_color(theme::border())
            })
        },
    )
    .style(|s| s.flex_col().width_full());
    let empty_hint = label(|| "No writes this session.".to_string()).style(move |s| {
        let s = s.padding(16.0).font_size(12.0).color(theme::fg_dim());
        if state.db_write_log.with(|l| l.is_empty()) {
            s
        } else {
            s.hide()
        }
    });
    let list = scroll(stack((rows, empty_hint)).style(|s| s.flex_col().width_full()))
        .style(|s| s.flex_grow(1.0).width_full());
    let card = stack((header, list)).style(|s| {
        s.flex_col()
            .width(680.0)
            .height(460.0)
            .border(1.0)
            .border_color(theme::border())
            .border_radius(10.0)
            .background(theme::bg())
    });
    container(card).style(move |s| {
        let s = s
            .absolute()
            .inset(0.0)
            .size_full()
            .items_center()
            .justify_center()
            .background(Color::from_rgba8(0, 0, 0, 120));
        if state.db_write_log_open.get() {
            s
        } else {
            s.hide()
        }
    })
}

/// The query-history panel: a searchable list of past runs; click one to load
/// it back into the console.
fn db_history_panel(state: AppState) -> impl IntoView {
    // Reload the list whenever the search text changes (while open).
    create_effect(move |_| {
        let _ = state.db_history_query.get();
        if state.db_history_open.get_untracked() {
            state.db_reload_history();
        }
    });

    let search = text_input(state.db_history_query)
        .placeholder("Search history…")
        .style(|s| {
            theme::input_colors(s)
                .flex_grow(1.0)
                .height(28.0)
                .padding_horiz(8.0)
        });
    let clear = label(|| "Clear".to_string())
        .style(|s| {
            s.padding_horiz(10.0)
                .height(28.0)
                .items_center()
                .border_radius(5.0)
                .font_size(12.0)
                .border(1.0)
                .border_color(theme::border())
                .color(theme::fg_dim())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.color(theme::fg()))
        })
        .on_click_stop(move |_| state.db_clear_history());
    let close = label(|| "✕".to_string())
        .style(|s| {
            s.padding_horiz(8.0)
                .color(theme::fg_dim())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.color(theme::fg()))
        })
        .on_click_stop(move |_| state.db_history_open.set(false));
    let header = stack((
        label(|| "Query History".to_string())
            .style(|s| s.font_size(13.0).font_bold().color(theme::fg())),
        search,
        clear,
        close,
    ))
    .style(|s| {
        s.flex_row()
            .items_center()
            .gap(8.0)
            .width_full()
            .padding(10.0)
            .border_bottom(1.0)
            .border_color(theme::border())
    });

    let rows = dyn_stack(
        move || {
            state
                .db_history
                .get()
                .into_iter()
                .enumerate()
                .collect::<Vec<_>>()
        },
        |(i, _)| *i,
        move |(_, e)| {
            let sql = e.sql.clone();
            let sql_for_click = sql.clone();
            let meta = {
                let count = match e.rows {
                    Some(n) if e.ok => format!("{n} rows"),
                    _ => String::new(),
                };
                let status = if e.ok { count } else { "error".to_string() };
                format!(
                    "{} · {} · {}ms · {}",
                    e.connection,
                    status,
                    e.duration_ms,
                    rel_ago(e.ts)
                )
            };
            let ok = e.ok;
            stack((
                label(move || sanitize_cell(&sql)).style(move |s| {
                    s.width_full()
                        .font_family("monospace".to_string())
                        .font_size(12.0)
                        .text_ellipsis()
                        .color(if ok {
                            theme::fg()
                        } else {
                            Color::from_rgb8(0xe0, 0x6c, 0x75)
                        })
                }),
                label(move || meta.clone())
                    .style(|s| s.font_size(10.5).color(theme::fg_dim()).margin_top(2.0)),
            ))
            .style(|s| {
                s.flex_col()
                    .width_full()
                    .padding_horiz(10.0)
                    .padding_vert(6.0)
                    .border_bottom(1.0)
                    .border_color(theme::border())
                    .cursor(floem::style::CursorStyle::Pointer)
                    .hover(|s| s.background(theme::bg_hover()))
            })
            .on_click_stop(move |_| state.db_reopen_history(sql_for_click.clone()))
        },
    )
    .style(|s| s.flex_col().width_full());
    let empty_hint = label(|| "No queries yet.".to_string()).style(move |s| {
        let s = s.padding(16.0).font_size(12.0).color(theme::fg_dim());
        if state.db_history.with(|h| h.is_empty()) {
            s
        } else {
            s.hide()
        }
    });
    let list = scroll(stack((rows, empty_hint)).style(|s| s.flex_col().width_full()))
        .style(|s| s.flex_grow(1.0).width_full());

    let card = stack((header, list)).style(|s| {
        s.flex_col()
            .width(640.0)
            .height(460.0)
            .border(1.0)
            .border_color(theme::border())
            .border_radius(10.0)
            .background(theme::bg())
    });
    container(card).style(move |s| {
        let s = s
            .absolute()
            .inset(0.0)
            .size_full()
            .items_center()
            .justify_center()
            .background(Color::from_rgba8(0, 0, 0, 120));
        if state.db_history_open.get() {
            s
        } else {
            s.hide()
        }
    })
}

/// The transactional "pending changes" bar: shown at the bottom of the grid
/// while there are staged edits/deletes, with Submit (one transaction, via the
/// confirmation dialog) and Revert.
fn pending_bar(state: AppState) -> impl IntoView {
    let summary = label(move || {
        let e = state.db_pending_edits.with(|m| m.len());
        let d = state.db_pending_deletes.with(|m| m.len());
        let total = e + d;
        format!(
            "⚠ {total} pending change{}  ·  {e} update{}, {d} delete{}",
            if total == 1 { "" } else { "s" },
            if e == 1 { "" } else { "s" },
            if d == 1 { "" } else { "s" },
        )
    })
    .style(|s| {
        s.flex_grow(1.0)
            .font_size(12.0)
            .color(Color::from_rgb8(0xe5, 0xc0, 0x7b))
    });
    let revert = label(|| "Revert".to_string())
        .style(|s| {
            s.padding_horiz(12.0)
                .height(28.0)
                .items_center()
                .border_radius(5.0)
                .font_size(12.0)
                .border(1.0)
                .border_color(theme::border())
                .color(theme::fg())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::bg_hover()))
        })
        .on_click_stop(move |_| state.db_revert_changes());
    let submit = label(|| "Submit…".to_string())
        .style(|s| {
            s.padding_horiz(14.0)
                .height(28.0)
                .items_center()
                .border_radius(5.0)
                .font_size(12.0)
                .background(theme::accent())
                .color(Color::from_rgb8(0x14, 0x16, 0x1b))
                .cursor(floem::style::CursorStyle::Pointer)
        })
        .on_click_stop(move |_| state.db_submit_changes());
    stack((summary, revert, submit)).style(move |s| {
        let s = s
            .flex_row()
            .items_center()
            .gap(8.0)
            .width_full()
            .padding_horiz(12.0)
            .padding_vert(8.0)
            .border_top(1.0)
            .border_color(theme::border())
            .background(theme::bg_panel());
        let any = state.db_pending_edits.with(|m| !m.is_empty())
            || state.db_pending_deletes.with(|m| !m.is_empty());
        if any {
            s
        } else {
            s.hide()
        }
    })
}

/// The console result-tab strip: one tab per statement of the last run, plus
/// pinned tabs. Click to switch, pin to keep, ✕ to close. Hidden when empty.
/// A banner listing EXPLAIN findings (full scans / missing indexes) for the
/// current plan, with a hint to ask the agent for an index migration (DB-602).
fn explain_banner(state: AppState) -> impl IntoView {
    let issues = label(move || {
        let list = state.db_explain_issues.get();
        if list.is_empty() {
            String::new()
        } else {
            format!("⚠ {}", list.join("  ·  "))
        }
    })
    .style(|s| {
        s.flex_grow(1.0)
            .font_size(11.0)
            .color(Color::from_rgb8(0xe0, 0x6c, 0x75))
            .text_ellipsis()
    });
    let hint = label(|| "Suggest Index → ask the agent".to_string())
        .style(|s| s.font_size(10.5).color(theme::fg_dim()).flex_shrink(0.0));
    stack((issues, hint)).style(move |s| {
        let s = s
            .flex_row()
            .items_center()
            .gap(10.0)
            .width_full()
            .padding_horiz(12.0)
            .padding_vert(5.0)
            .border_bottom(1.0)
            .border_color(theme::border())
            .background(Color::from_rgba8(0xe0, 0x6c, 0x75, 24));
        if state.db_explain_issues.with(|i| i.is_empty()) {
            s.hide()
        } else {
            s
        }
    })
}

fn result_tabs_strip(state: AppState) -> impl IntoView {
    let tabs = dyn_stack(
        move || {
            state
                .db_result_tabs
                .get()
                .into_iter()
                .enumerate()
                .collect::<Vec<_>>()
        },
        |(i, t)| (*i, t.pinned),
        move |(i, t)| {
            let title = t.title.clone();
            let pinned = t.pinned;
            let is_err = t.error.is_some();
            let pin = label(move || if pinned { "★" } else { "☆" }.to_string())
                .style(move |s| {
                    s.font_size(11.0)
                        .color(if pinned {
                            Color::from_rgb8(0xe5, 0xc0, 0x7b)
                        } else {
                            theme::fg_dim()
                        })
                        .cursor(floem::style::CursorStyle::Pointer)
                        .hover(|s| s.color(theme::fg()))
                })
                .on_click_stop(move |_| state.db_toggle_pin(i));
            let name = label(move || title.clone()).style(move |s| {
                s.font_size(12.0).color(if is_err {
                    Color::from_rgb8(0xe0, 0x6c, 0x75)
                } else {
                    theme::fg()
                })
            });
            let close = label(|| "✕".to_string())
                .style(|s| {
                    s.font_size(11.0)
                        .color(theme::fg_dim())
                        .cursor(floem::style::CursorStyle::Pointer)
                        .hover(|s| s.color(theme::fg()))
                })
                .on_click_stop(move |_| state.db_close_tab(i));
            stack((pin, name, close))
                .style(move |s| {
                    let active = state.db_active_tab.get() == i;
                    let s = s
                        .flex_row()
                        .items_center()
                        .gap(6.0)
                        .padding_horiz(10.0)
                        .padding_vert(5.0)
                        .border_right(1.0)
                        .border_color(theme::border())
                        .cursor(floem::style::CursorStyle::Pointer);
                    if active {
                        s.background(theme::bg_active())
                    } else {
                        s.hover(|s| s.background(theme::bg_hover()))
                    }
                })
                .on_click_stop(move |_| state.db_activate_tab(i))
        },
    )
    .style(|s| s.flex_row().items_center());

    floem::views::scroll(tabs).style(move |s| {
        let s = s
            .width_full()
            .border_bottom(1.0)
            .border_color(theme::border())
            .background(theme::bg_hover());
        if state.db_result_tabs.with(|t| t.is_empty()) {
            s.hide()
        } else {
            s
        }
    })
}

fn result_grid(state: AppState) -> impl IntoView {
    // Header row.
    let header = dyn_stack(
        move || state.db_result.get().map(|r| r.columns).unwrap_or_default(),
        |c| c.clone(),
        move |c| {
            let col = c.clone();
            let col2 = c.clone();
            label(move || {
                let arrow = match state.db_sort.get() {
                    Some((sc, true)) if sc == col2 => " ▲",
                    Some((sc, false)) if sc == col2 => " ▼",
                    _ => "",
                };
                let pk = state
                    .db_columns
                    .with(|cols| cols.iter().any(|c| c.name == col2 && c.key == "PRI"));
                let key = if pk { "🔑 " } else { "" };
                format!("{key}{col2}{arrow}")
            })
            .style(move |s| {
                let s = s
                    .width(180.0)
                    .flex_shrink(0.0)
                    .padding_horiz(8.0)
                    .padding_vert(5.0)
                    .font_size(11.0)
                    .font_bold()
                    .text_ellipsis()
                    .color(theme::fg_dim())
                    .border_right(1.0)
                    .border_color(theme::border());
                if state.db_result_table.get().is_some() {
                    s.cursor(floem::style::CursorStyle::Pointer)
                        .hover(|s| s.color(theme::fg()))
                } else {
                    s
                }
            })
            .on_click_stop(move |_| {
                if state.db_result_table.get_untracked().is_some() {
                    state.db_sort_by(col.clone());
                }
            })
        },
    )
    .style(|s| {
        s.flex_row()
            .border_bottom(1.0)
            .border_color(theme::border())
            .background(theme::bg_hover())
    });

    // Data rows.
    let rows = dyn_stack(
        move || {
            state
                .db_result
                .get()
                .map(|r| r.rows.into_iter().enumerate().collect::<Vec<_>>())
                .unwrap_or_default()
        },
        |(i, _)| *i,
        move |(ri, row)| {
            dyn_stack(
                move || row.clone().into_iter().enumerate().collect::<Vec<_>>(),
                |(i, _)| *i,
                move |(ci, cell)| {
                    let is_null = cell.is_none();
                    let text = match &cell {
                        Some(s) => sanitize_cell(s),
                        None => "NULL".to_string(),
                    };
                    label(move || text.clone())
                        .style(move |s| {
                            let selected = state.db_selected_cell.get() == Some((ri, ci));
                            let pending_edit =
                                state.db_pending_edits.with(|m| m.contains_key(&(ri, ci)));
                            let pending_del =
                                state.db_pending_deletes.with(|m| m.contains_key(&ri));
                            let s = s
                                .width(180.0)
                                .flex_shrink(0.0)
                                .height(26.0)
                                .padding_horiz(8.0)
                                .padding_vert(4.0)
                                .font_size(12.0)
                                .text_ellipsis()
                                .border_right(1.0)
                                .border_color(theme::border());
                            // Colour: deleted row (red) > pending edit (amber) >
                            // NULL (dim) > normal.
                            let s = if pending_del {
                                s.color(Color::from_rgb8(0xe0, 0x6c, 0x75))
                            } else if is_null {
                                s.color(theme::fg_dim())
                            } else {
                                s.color(theme::fg())
                            };
                            let s = if pending_edit {
                                s.background(Color::from_rgba8(0xe5, 0xc0, 0x7b, 40))
                            } else if selected {
                                s.background(theme::bg_active())
                            } else {
                                s
                            };
                            if state.db_editable() {
                                s.cursor(floem::style::CursorStyle::Text)
                            } else {
                                s
                            }
                        })
                        // Single click selects (→ value viewer); double click edits.
                        .on_click_stop(move |_| state.db_selected_cell.set(Some((ri, ci))))
                        .on_double_click_stop(move |_| state.db_begin_edit(ri, ci))
                },
            )
            .style(|s| {
                s.flex_row()
                    .items_center()
                    .height(26.0)
                    .border_bottom(1.0)
                    .border_color(theme::border())
                    .hover(|s| s.background(theme::bg_hover()))
            })
        },
    )
    .style(|s| s.flex_col());

    stack((header, rows)).style(move |s| {
        let n = state
            .db_result
            .with(|r| r.as_ref().map(|r| r.columns.len()).unwrap_or(0));
        s.flex_col().width((n.max(1) as f64) * 180.0)
    })
}

/// The structure (columns) view of the browsed table.
fn structure_grid(state: AppState) -> impl IntoView {
    let head_cell = |t: &'static str, w: f64| {
        label(move || t.to_string()).style(move |s| {
            s.width(w)
                .flex_shrink(0.0)
                .padding_horiz(8.0)
                .padding_vert(5.0)
                .font_size(11.0)
                .font_bold()
                .color(theme::fg_dim())
                .border_right(1.0)
                .border_color(theme::border())
        })
    };
    let header = stack((
        head_cell("Column", 200.0),
        head_cell("Type", 200.0),
        head_cell("Null", 60.0),
        head_cell("Key", 60.0),
    ))
    .style(|s| {
        s.flex_row()
            .border_bottom(1.0)
            .border_color(theme::border())
            .background(theme::bg_hover())
    });

    let rows = dyn_stack(
        move || {
            state
                .db_columns
                .get()
                .into_iter()
                .enumerate()
                .collect::<Vec<_>>()
        },
        |(i, _)| *i,
        move |(_, c)| {
            let cell = |t: String, w: f64, dim: bool| {
                label(move || t.clone()).style(move |s| {
                    let s = s
                        .width(w)
                        .flex_shrink(0.0)
                        .padding_horiz(8.0)
                        .padding_vert(4.0)
                        .font_size(12.0)
                        .text_ellipsis()
                        .border_right(1.0)
                        .border_color(theme::border());
                    if dim {
                        s.color(theme::fg_dim())
                    } else {
                        s.color(theme::fg())
                    }
                })
            };
            stack((
                cell(c.name.clone(), 200.0, false),
                cell(c.data_type.clone(), 200.0, true),
                cell(
                    if c.nullable {
                        "YES".into()
                    } else {
                        "NO".into()
                    },
                    60.0,
                    true,
                ),
                cell(c.key.clone(), 60.0, true),
            ))
            .style(|s| {
                s.flex_row()
                    .border_bottom(1.0)
                    .border_color(theme::border())
                    .hover(|s| s.background(theme::bg_hover()))
            })
        },
    )
    .style(|s| s.flex_col());

    // Indexes section (below the columns grid).
    let idx_title = label(|| "Indexes".to_string()).style(move |s| {
        let s = s
            .font_size(11.0)
            .font_bold()
            .color(theme::fg_dim())
            .padding_horiz(8.0)
            .padding_top(12.0)
            .padding_bottom(4.0);
        if state.db_indexes.with(|i| i.is_empty()) {
            s.hide()
        } else {
            s
        }
    });
    let idx_rows = dyn_stack(
        move || {
            state
                .db_indexes
                .get()
                .into_iter()
                .enumerate()
                .collect::<Vec<_>>()
        },
        |(i, _)| *i,
        move |(_, ix)| {
            let cols = ix.columns.join(", ");
            let name = ix.name.clone();
            let badge = if ix.unique { "UNIQUE" } else { "INDEX" };
            stack((
                label(move || badge.to_string()).style(move |s| {
                    s.width(70.0)
                        .flex_shrink(0.0)
                        .padding_horiz(8.0)
                        .padding_vert(4.0)
                        .font_size(10.5)
                        .color(if ix.unique {
                            Color::from_rgb8(0xe5, 0xc0, 0x7b)
                        } else {
                            theme::fg_dim()
                        })
                }),
                label(move || name.clone()).style(|s| {
                    s.width(200.0)
                        .flex_shrink(0.0)
                        .padding_horiz(8.0)
                        .padding_vert(4.0)
                        .font_size(12.0)
                        .color(theme::fg())
                        .text_ellipsis()
                }),
                label(move || cols.clone()).style(|s| {
                    s.flex_grow(1.0)
                        .padding_horiz(8.0)
                        .padding_vert(4.0)
                        .font_size(12.0)
                        .font_family("monospace".to_string())
                        .color(theme::fg_dim())
                        .text_ellipsis()
                }),
            ))
            .style(|s| {
                s.flex_row()
                    .border_bottom(1.0)
                    .border_color(theme::border())
                    .hover(|s| s.background(theme::bg_hover()))
            })
        },
    )
    .style(|s| s.flex_col());

    let copy_ddl = label(|| "⧉ Copy DDL".to_string())
        .style(|s| {
            s.font_size(11.0)
                .padding_horiz(8.0)
                .padding_vert(3.0)
                .margin_vert(6.0)
                .border_radius(4.0)
                .border(1.0)
                .border_color(theme::border())
                .color(theme::fg_dim())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::bg_hover()).color(theme::fg()))
        })
        .on_click_stop(move |_| state.db_copy_ddl());

    stack((header, rows, idx_title, idx_rows, copy_ddl)).style(|s| s.flex_col().width(520.0))
}

#[cfg(test)]
mod tests {
    use super::pretty_value;

    #[test]
    fn pretty_value_formats_json_objects_and_arrays() {
        assert_eq!(pretty_value("{\"a\":1}"), "{\n  \"a\": 1\n}");
        assert_eq!(pretty_value("[1,2]"), "[\n  1,\n  2\n]");
    }

    #[test]
    fn pretty_value_leaves_scalars_and_invalid_json_untouched() {
        assert_eq!(pretty_value("hello"), "hello");
        assert_eq!(pretty_value("123"), "123");
        assert_eq!(pretty_value("{not json"), "{not json");
        assert_eq!(pretty_value(""), "");
    }
}
