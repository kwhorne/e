//! The Database panel (right side by default, toggled with ⌘3) and the results
//! overlay. Inspired by the Conductor database panel.

use floem::peniko::Color;
use floem::reactive::{create_rw_signal, RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::views::{
    container, dyn_container, dyn_stack, empty, label, scroll, stack, text_input, Decorators,
};
use floem::IntoView;

use crate::state::{AppState, DbEntry, DbForm};
use crate::theme;

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
    let mut out = String::with_capacity(s.len().min(200));
    let mut n = 0usize;
    for c in s.chars() {
        if n >= 200 {
            out.push('…');
            break;
        }
        out.push(if c == '\n' || c == '\r' || c == '\t' {
            ' '
        } else {
            c
        });
        n += 1;
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

    let head = stack((caret, glyph, name, count))
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
    let e_d = entry.clone();
    let d_btn = action_glyph("⏏", move || state.db_disconnect(e_d.clone()));
    let e_e = entry.clone();
    let edit_btn = action_glyph("✎", move || state.db_start_edit(e_e.clone()));
    let key_rm = entry.key();
    let x_btn = action_glyph("✕", move || state.db_remove(key_rm.clone()));
    let actions = stack((q_btn, r_btn, d_btn, edit_btn, x_btn))
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
            stack((
                label(|| "▦".to_string()).style(|s| s.color(theme::fg_dim()).font_size(11.0)),
                label(move || tn.clone())
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
            .on_click_stop(move |_| state.db_open_table(entry.clone(), table.clone()))
        },
    )
    .style(|s| s.flex_col().width_full());

    stack((row, err, filter, tables)).style(|s| s.flex_col().width_full().margin_bottom(2.0))
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
    let header = stack((title, add)).style(|s| {
        s.flex_row()
            .items_center()
            .gap(6.0)
            .padding_horiz(12.0)
            .padding_top(10.0)
            .padding_bottom(6.0)
            .width_full()
    });

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

    stack((header, form, empty_hint, list)).style(|s| {
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
    let header = stack((title, close)).style(|s| {
        s.flex_row()
            .items_center()
            .padding_horiz(12.0)
            .padding_vert(8.0)
            .width_full()
            .border_bottom(1.0)
            .border_color(theme::border())
    });

    // Query editor row.
    let sql = text_input(state.db_query_text)
        .placeholder("SQL — ⌘↵ to run")
        .style(|s| {
            theme::input_colors(s)
                .width_full()
                .min_height(40.0)
                .font_family("monospace".to_string())
                .font_size(13.0)
                .padding_horiz(10.0)
                .padding_vert(10.0)
        })
        .on_key_down(
            floem::keyboard::Key::Named(floem::keyboard::NamedKey::Enter),
            |m| m.meta() || m.control(),
            move |_| state.db_run_query(),
        );
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
    let sql_wrap = floem::views::container(sql).style(|s| s.flex_grow(1.0).min_width(0.0));
    let query_row = stack((sql_wrap, run)).style(|s| {
        s.flex_row()
            .items_center()
            .gap(8.0)
            .padding(10.0)
            .width_full()
            .border_bottom(1.0)
            .border_color(theme::border())
    });

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
    let page_lbl = label(move || format!("p{}", state.db_page.get() + 1))
        .style(|s| s.font_size(11.0).color(theme::fg_dim()).items_center());
    let pager = stack((prev, page_lbl, next)).style(move |s| {
        let s = s.flex_row().gap(4.0).items_center();
        if state.db_result_table.get().is_some() && state.db_subview.get() == "data" {
            s
        } else {
            s.hide()
        }
    });

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
    let export = toolbar_btn("⬇ CSV", Box::new(move || state.db_export_csv()));

    let toolbar = stack((
        subview_chips,
        pager,
        spacer,
        saved_menu,
        name_input,
        save_btn,
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
        status,
        grid,
        db_edit_popup(state),
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

    let card = stack((title, input, buttons)).style(|s| {
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
                            let s = if is_null {
                                s.color(theme::fg_dim())
                            } else {
                                s.color(theme::fg())
                            };
                            if state.db_editable() {
                                s.cursor(floem::style::CursorStyle::Text)
                            } else {
                                s
                            }
                        })
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

    stack((header, rows)).style(|s| s.flex_col().width(520.0))
}
