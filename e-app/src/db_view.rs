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
        e_db::Environment::Local => theme::success(),
        e_db::Environment::Staging => theme::warning(),
        e_db::Environment::Production => theme::error(),
    }
}

/// Nudge the results-grid scroll by `(dx, dy)`; the tick makes each call a
/// distinct signal value so the `scroll_delta` effect always re-fires.
fn db_scroll(state: AppState, dx: f64, dy: f64) {
    state.db.scroll.update(|(x, y, t)| {
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
        s.flex_grow(1.0_f32)
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
                theme::warning()
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
                .flex_grow(1.0_f32)
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
    // Snapshots (DB-704): take one, or put the database back as one has it.
    let e_snap = entry.clone();
    let snap_btn = label(|| "⤓".to_string())
        .style(|s| {
            s.padding_horiz(5.0)
                .padding_vert(2.0)
                .border_radius(4.0)
                .color(theme::fg_dim())
                .font_size(12.0)
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::bg()).color(theme::fg()))
        })
        .popout_menu(move || {
            use floem::menu::{Menu, MenuItem};
            let entry = e_snap.clone();
            if !entry.config.environment().is_local() {
                return Menu::new("")
                    .entry(MenuItem::new("Snapshots are for local databases only").enabled(false));
            }
            let take = entry.clone();
            let mut menu = Menu::new("").entry(
                MenuItem::new("Take snapshot now").action(move || state.db_snapshot(take.clone())),
            );
            let snaps = state.db_snapshots(&entry);
            let now = crate::state::now_ms_epoch() / 1000;
            if snaps.is_empty() {
                menu = menu.entry(MenuItem::new("No snapshots yet").enabled(false));
            } else {
                menu = menu.separator();
                for snap in snaps.into_iter().take(12) {
                    let text = format!(
                        "Restore to {} · {}",
                        crate::db_state::ago(now, snap.taken),
                        crate::db_state::file_size(snap.bytes)
                    );
                    let entry = entry.clone();
                    menu = menu
                        .entry(MenuItem::new(text).action(move || {
                            state.db_restore_snapshot(entry.clone(), snap.clone())
                        }));
                }
                if let Some(dir) = crate::config::snapshot_dir() {
                    let folder = e_db::snapshot_folder(&dir, &entry.config);
                    menu = menu.separator().entry(
                        MenuItem::new("Show snapshots in Finder")
                            .action(move || crate::platform::reveal(&folder)),
                    );
                }
            }
            menu
        });
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
            .color(theme::error())
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
                    .with(|m| {
                        m.get(&t_count).map(|n| match n.bytes {
                            // Rows, and the space the table takes (DB-206).
                            Some(b) => {
                                format!("{} · {}", e_db::row_count_label(*n), e_db::size_label(b))
                            }
                            None => e_db::row_count_label(*n),
                        })
                    })
                    .unwrap_or_default()
            })
            .style(|s| {
                s.color(theme::fg_dim())
                    .font_size(10.0)
                    .flex_shrink(0.0_f32)
            });
            stack((
                label(|| "▦".to_string()).style(|s| s.color(theme::fg_dim()).font_size(11.0)),
                label(move || tn.clone()).style(|s| {
                    s.color(theme::fg())
                        .flex_grow(1.0_f32)
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
    password: bool,
) -> impl IntoView {
    let field_style = |s: floem::style::Style| {
        theme::input_colors(s)
            .width_full()
            .font_size(12.0)
            .padding_horiz(6.0)
            .padding_vert(3.0)
    };
    // The text input has no masked mode, so a secret is shown as dots until
    // the user asks to see or change it — opening the form (say, on a shared
    // screen) doesn't print the password in clear. That holds for an empty
    // one too: a new password would otherwise show as it is typed, before
    // anyone thought to hide it.
    let revealed = create_rw_signal(!password);
    let field = dyn_container(
        move || revealed.get(),
        move |shown| {
            if shown {
                let input = text_input(sig).placeholder(placeholder).style(field_style);
                if password {
                    // Shown after a click on Show/Enter: straight to typing.
                    floem::View::id(&input).request_focus();
                }
                input.into_any()
            } else {
                stack((
                    label(move || {
                        let n = sig.with(|v| v.chars().count());
                        if n == 0 {
                            "Not set".to_string()
                        } else {
                            "•".repeat(n.clamp(8, 16))
                        }
                    })
                    .style(move |s| field_style(s).flex_grow(1.0_f32)),
                    label(move || {
                        if sig.with(|v| v.is_empty()) {
                            "Enter".to_string()
                        } else {
                            "Show".to_string()
                        }
                    })
                    .style(|s| {
                        s.font_size(11.0)
                            .color(theme::accent())
                            .cursor(floem::style::CursorStyle::Pointer)
                    })
                    .on_click_stop(move |_| revealed.set(true)),
                ))
                .style(|s| s.items_center().gap(6.0).width_full())
                .into_any()
            }
        },
    )
    .style(|s| s.width_full());
    stack((
        label(move || label_text.to_string()).style(|s| s.font_size(11.0).color(theme::fg_dim())),
        field,
    ))
    .style(|s| s.flex_col().gap(2.0).width_full())
}

fn add_form(state: AppState) -> impl IntoView {
    // Bind the form struct's fields to local signals, syncing back on change.
    let f = state.db.form.get_untracked();
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
    let tls = create_rw_signal(f.tls.clone());
    let tls_ca = create_rw_signal(f.tls_ca.clone());
    let env_label = create_rw_signal(f.env_label.clone());

    // Identity isn't edited in the dialog, but it has to survive the round trip
    // or the connection would be re-keyed in the credential store on every save.
    let form_id = f.id.clone();
    let form_in_keychain = f.secrets_in_keychain;
    // Nor is the label, which may be what marks the connection production.
    let form_label = f.label.clone();

    let sync = std::rc::Rc::new(move || {
        state.db.form.set(DbForm {
            id: form_id.clone(),
            secrets_in_keychain: form_in_keychain,
            engine: engine.get_untracked(),
            host: host.get_untracked(),
            port: port.get_untracked(),
            database: database.get_untracked(),
            username: username.get_untracked(),
            password: password.get_untracked(),
            path: path.get_untracked(),
            group: group.get_untracked(),
            label: form_label.clone(),
            use_ssh: use_ssh.get_untracked(),
            ssh_host: ssh_host.get_untracked(),
            ssh_port: ssh_port.get_untracked(),
            ssh_user: ssh_user.get_untracked(),
            ssh_auth: ssh_auth.get_untracked(),
            ssh_password: ssh_password.get_untracked(),
            ssh_key_path: ssh_key_path.get_untracked(),
            ssh_passphrase: ssh_passphrase.get_untracked(),
            tls: tls.get_untracked(),
            tls_ca: tls_ca.get_untracked(),
            env_label: env_label.get_untracked(),
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
                    s.background(theme::accent()).color(theme::on_accent())
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
                .color(theme::on_accent())
                .cursor(floem::style::CursorStyle::Pointer)
        })
        .on_click_stop(move |_| {
            state.db.editing_key.set(None);
            state.db_add_from_env();
            state.db.adding.set(false);
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

    // TLS to the server (hidden for sqlite).
    let tls_chip = move |id: &'static str, name: &'static str| {
        label(move || name.to_string())
            .style(move |s| {
                let active = tls.get() == id;
                let s = s
                    .padding_horiz(8.0)
                    .padding_vert(2.0)
                    .border_radius(5.0)
                    .font_size(11.0)
                    .cursor(floem::style::CursorStyle::Pointer);
                if active {
                    s.background(theme::accent()).color(theme::on_accent())
                } else {
                    s.border(1.0)
                        .border_color(theme::border())
                        .color(theme::fg())
                }
            })
            .on_click_stop(move |_| tls.set(id.to_string()))
    };
    let tls_hint = move || match e_db::TlsMode::parse(&tls.get()) {
        e_db::TlsMode::Auto if engine.get() == "clickhouse" => {
            "HTTPS on ports 8443 and 443, plain HTTP otherwise".to_string()
        }
        e_db::TlsMode::Auto => {
            "Encrypted when the server offers it (not checked); local hosts and SSH go without"
                .to_string()
        }
        e_db::TlsMode::Disable => "Never encrypted".to_string(),
        e_db::TlsMode::Require => "Always encrypted; the certificate isn't checked".to_string(),
        // The MySQL driver can't be told the real host name through a
        // tunnel (where it connects to 127.0.0.1), so there it isn't checked.
        e_db::TlsMode::Verify if engine.get() == "mysql" && use_ssh.get() => {
            "Always encrypted; certificate checked, but not the host name (MySQL through SSH)"
                .to_string()
        }
        e_db::TlsMode::Verify => "Always encrypted; certificate and host name checked".to_string(),
    };
    let tls_fields = stack((
        label(|| "TLS".to_string()).style(|s| s.font_size(11.0).color(theme::fg_dim())),
        stack((
            tls_chip("", "Auto"),
            tls_chip("disable", "Off"),
            tls_chip("require", "Require"),
            tls_chip("verify", "Verify"),
        ))
        .style(|s| s.flex_row().gap(5.0)),
        label(tls_hint).style(|s| s.font_size(11.0).color(theme::fg_dim())),
        dyn_stack(
            move || {
                if e_db::TlsMode::parse(&tls.get()) == e_db::TlsMode::Verify {
                    vec!["ca"]
                } else {
                    vec![]
                }
            },
            |k| k.to_string(),
            move |_| {
                form_field(
                    "CA certificate (optional)",
                    tls_ca,
                    "/path/to/ca.pem",
                    false,
                )
                .into_any()
            },
        )
        .style(|s| s.flex_col().width_full()),
    ))
    .style(move |s| {
        let s = s.flex_col().gap(4.0).width_full();
        if engine.get() == "sqlite" {
            s.hide()
        } else {
            s
        }
    });

    // Environment: guessed from the names and host, or set by hand (DB-102).
    // It decides the colour, the read-only default and which writes ask first.
    let guessed = {
        let form_label = f.label.clone();
        move || {
            e_db::DbConfig {
                engine: engine.get(),
                host: host.get(),
                database: database.get(),
                path: path.get(),
                label: form_label.clone(),
                use_ssh: use_ssh.get(),
                ssh_host: ssh_host.get(),
                ..Default::default()
            }
            .guessed_environment()
        }
    };
    let env_chip = {
        let guessed = guessed.clone();
        move |id: &'static str| {
            let guessed = guessed.clone();
            label(move || match e_db::Environment::parse(id) {
                Some(env) => env.label().to_string(),
                None => format!("Auto ({})", guessed().label()),
            })
            .style(move |s| {
                let active = env_label.get() == id;
                let s = s
                    .padding_horiz(8.0)
                    .padding_vert(2.0)
                    .border_radius(5.0)
                    .font_size(11.0)
                    .cursor(floem::style::CursorStyle::Pointer);
                if active {
                    s.background(theme::accent()).color(theme::on_accent())
                } else {
                    s.border(1.0)
                        .border_color(theme::border())
                        .color(theme::fg())
                }
            })
            .on_click_stop(move |_| env_label.set(id.to_string()))
        }
    };
    let env_hint = move || {
        let set = e_db::Environment::parse(&env_label.get());
        let guess = guessed();
        match set {
            Some(e_db::Environment::Local) if guess != e_db::Environment::Local => format!(
                "Treated as local: writes run without asking, though it looks like {}",
                guess.label()
            ),
            Some(env) => format!("Set by hand: {}", env.label()),
            None => "Guessed from the name, database and host".to_string(),
        }
    };
    let env_fields = stack((
        label(|| "Environment".to_string()).style(|s| s.font_size(11.0).color(theme::fg_dim())),
        stack((
            env_chip(""),
            env_chip("local"),
            env_chip("staging"),
            env_chip("production"),
        ))
        .style(|s| {
            s.flex_row()
                .gap(5.0)
                .flex_wrap(floem::style::FlexWrap::Wrap)
        }),
        label(env_hint).style(|s| s.font_size(11.0).color(theme::fg_dim())),
    ))
    .style(|s| s.flex_col().gap(4.0).width_full());

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
                    s.background(theme::accent()).color(theme::on_accent())
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
    .style(move |s| {
        let s = s
            .flex_col()
            .gap(6.0)
            .width_full()
            .padding(8.0)
            .border(1.0)
            .border_color(theme::border())
            .border_radius(8.0)
            .background(theme::bg_hover());
        // An empty box when there's no tunnel (or SQLite) to fill in.
        if use_ssh.get() && engine.get() != "sqlite" {
            s
        } else {
            s.hide()
        }
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
    let test_status = label(move || match state.db.test_state.get().as_str() {
        "" => String::new(),
        "testing" => "testing…".to_string(),
        "ok" => "✓ OK".to_string(),
        _ => "✗ failed".to_string(),
    })
    .style(move |s| {
        let st = state.db.test_state.get();
        let s = s.font_size(11.0).items_center();
        match st.as_str() {
            "ok" => s.color(theme::success()),
            "" | "testing" => s.color(theme::fg_dim()),
            _ => s.color(theme::error()),
        }
    });
    let test_row = stack((test, test_status)).style(|s| s.flex_row().gap(8.0).items_center());

    let connect = label(move || {
        if state.db.editing_key.get().is_some() {
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
        tls_fields,
        env_fields,
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
        s.flex_grow(1.0_f32)
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
        .on_click_stop(move |_| state.db.adding.update(|a| *a = !*a));
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
    let active_row = stack((active_conn_chip(state),)).style(move |s| {
        let s = s
            .flex_row()
            .padding_horiz(12.0)
            .margin_bottom(6.0)
            .width_full();
        if state.db.conns.with(|c| c.is_empty()) {
            s.hide()
        } else {
            s
        }
    });
    // Search across all tables' text columns (DB-805); Enter runs it.
    let search_all = text_input(state.db.search_query)
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
            if state.db.adding.get() {
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
            move || state.db.conns.get(),
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
    .style(|s| s.flex_col().flex_grow(1.0_f32).width_full());

    let empty_hint = label(|| "No connections yet.".to_string()).style(move |s| {
        let s = s.padding(16.0).color(theme::fg_dim()).font_size(12.0);
        if state.db.conns.with(|c| c.is_empty()) && !state.db.adding.get() {
            s
        } else {
            s.hide()
        }
    });

    stack((header, active_row, search_all, form, empty_hint, list)).style(|s| {
        s.flex_col()
            .height_full()
            .width_full()
            .background(theme::bg())
    })
}

/// The active connection, in its environment's colour, with a menu to
/// switch to another (DB-104). Everything the panel runs — the console,
/// search, EXPLAIN, relationships — goes to this one.
fn active_conn_chip(state: AppState) -> impl IntoView {
    let active = move || {
        state.db.result_key.get().and_then(|key| {
            state
                .db
                .conns
                .with(|c| c.iter().find(|e| e.key() == key).cloned())
        })
    };
    let dot = label(|| "●".to_string()).style(move |s| match active() {
        Some(e) => s.font_size(10.0).color(env_color(e.config.environment())),
        None => s.hide(),
    });
    let name = label(move || match active() {
        Some(e) => format!(
            "{} · {} ▾",
            e.config.display_name(),
            e.config.environment().label()
        ),
        None => "No database ▾".to_string(),
    })
    .style(move |s| {
        let s = s.font_size(11.0).text_ellipsis().min_width(0.0);
        match active() {
            Some(e) => s.color(env_color(e.config.environment())),
            None => s.color(theme::fg_dim()),
        }
    });
    stack((dot, name))
        .style(|s| {
            s.flex_row()
                .items_center()
                .gap(5.0)
                .min_width(0.0)
                .padding_horiz(8.0)
                .padding_vert(2.0)
                .border(1.0)
                .border_color(theme::border())
                .border_radius(6.0)
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::bg_hover()))
        })
        .popout_menu(move || {
            use floem::menu::{Menu, MenuItem};
            let conns = state.db.conns.get_untracked();
            if conns.is_empty() {
                return Menu::new("").entry(MenuItem::new("(no connections)"));
            }
            let current = state.db.result_key.get_untracked();
            let mut menu = Menu::new("");
            for e in conns {
                let mark = if current.as_deref() == Some(e.key().as_str()) {
                    "✓ "
                } else {
                    "   "
                };
                let closed = if e.conn.get_untracked().is_none() {
                    " (connect)"
                } else {
                    ""
                };
                let text = format!(
                    "{mark}{} — {}{closed}",
                    e.config.display_name(),
                    e.config.environment().label()
                );
                menu = menu
                    .entry(MenuItem::new(text).action(move || state.db_choose_active(e.clone())));
            }
            menu
        })
}

// ---- Results overlay ------------------------------------------------------

pub fn db_result_overlay(state: AppState) -> impl IntoView {
    let title = label(move || state.db.result_title.get()).style(|s| {
        s.flex_grow(1.0_f32)
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
    // The active connection in its environment's colour, so it's always
    // clear which database's data you're looking at, and a switch (DB-104).
    let env_chip = active_conn_chip(state);
    let header = stack((env_chip, title, close)).style(|s| {
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
                .color(theme::on_accent())
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
    let run_col = stack((run, history_btn)).style(|s| s.flex_col().gap(6.0).flex_shrink(0.0_f32));
    let sql_wrap = floem::views::container(sql).style(move |s| {
        s.flex_grow(1.0_f32)
            .min_width(0.0)
            .height(state.db.console_height.get())
            .flex_shrink(0.0_f32)
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
                    let cur = state.db.console_height.get_untracked();
                    state
                        .db
                        .console_height
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
        if state.db.result_loading.get() {
            "Running…".to_string()
        } else if let Some(e) = state.db.result_error.get() {
            e
        } else if let Some(r) = state.db.result.get() {
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
        if state.db.result_error.get().is_some() {
            s.color(theme::error())
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
            if state.db.result_error.get().is_some() {
                s
            } else {
                s.hide()
            }
        })
        .on_click_stop(move |_| {
            if let Some(err) = state.db.result_error.get_untracked() {
                let sql = state.db.query_text.get_untracked();
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
                .color(theme::error())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::bg_hover()));
            if state.db.result_loading.get() {
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
                let active = state.db.subview.get() == id;
                let s = s
                    .padding_horiz(10.0)
                    .padding_vert(2.0)
                    .border_radius(5.0)
                    .font_size(11.0)
                    .cursor(floem::style::CursorStyle::Pointer);
                if active {
                    s.background(theme::accent()).color(theme::on_accent())
                } else {
                    s.color(theme::fg_dim()).hover(|s| s.color(theme::fg()))
                }
            })
            .on_click_stop(move |_| state.db_set_subview(id))
    };
    let subview_chips =
        stack((chip("data", "Data"), chip("structure", "Structure"))).style(move |s| {
            let s = s.flex_row().gap(4.0).items_center();
            if state.db.result_table.get().is_some() {
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
        let page = state.db.page.get() + 1;
        match state.db.total_rows.get() {
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
        if state.db.result_table.get().is_some() && state.db.subview.get() == "data" {
            s
        } else {
            s.hide()
        }
    });

    let filter_chip = label(move || {
        state
            .db
            .filter
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
        if state.db.filter.get().is_some() && state.db.subview.get() == "data" {
            s
        } else {
            s.hide()
        }
    })
    .on_click_stop(move |_| state.db_clear_filter());

    let spacer = empty().style(|s| s.flex_grow(1.0_f32));

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
            let queries = state.db.queries.get_untracked();
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
            state.db.query_name.set(String::new());
            state.db.saving_query.set(true);
        }),
    );
    let name_input = text_input(state.db.query_name)
        .placeholder("query name — ↵ to save")
        .style(move |s| {
            let s = theme::input_colors(s)
                .width(180.0)
                .font_size(11.0)
                .padding_horiz(6.0)
                .padding_vert(2.0);
            if state.db.saving_query.get() {
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
            move |_| state.db.saving_query.set(false),
        )
        .request_focus(move || {
            state.db.saving_query.get();
        });
    // Export to a file and copy to the clipboard, each in every format
    // (DB-502/505).
    let format_menu = |glyph: &'static str, copy: bool| {
        label(move || format!("{glyph} ▾"))
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
                let mut menu = Menu::new("");
                for fmt in crate::db_export::Format::ALL {
                    menu = menu.entry(MenuItem::new(fmt.name()).action(move || {
                        if copy {
                            state.db_copy_result(fmt)
                        } else {
                            state.db_export(fmt)
                        }
                    }));
                }
                menu
            })
    };
    let export = format_menu("⬇ Export", false);
    let copy = format_menu("⧉ Copy", true);
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
            if state.db.result_table.get().is_some() && state.db.subview.get() == "data" {
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
            if state.db.result_table.get().is_some() && state.db.subview.get() == "data" {
                s
            } else {
                s.hide()
            }
        })
        .on_click_stop(move |_| state.db_import_csv());
    // Seed rows through the table's factory, or make the factory (DB-802).
    let seed = label(|| "Seed ▾".to_string())
        .style(move |s| {
            let s = s
                .padding_horiz(8.0)
                .padding_vert(2.0)
                .border_radius(4.0)
                .font_size(12.0)
                .color(theme::fg_dim())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::bg_hover()).color(theme::fg()));
            if state.db.result_table.get().is_some() && state.db.subview.get() == "data" {
                s
            } else {
                s.hide()
            }
        })
        .popout_menu(move || {
            use floem::menu::{Menu, MenuItem};
            let factory = state.db_factory_path();
            let local = state.db_table_is_local();
            let mut menu = Menu::new("");
            match factory {
                Some((model, path)) if path.is_file() => {
                    for n in [10usize, 100, 1000] {
                        let text = format!("Seed {} rows", group_digits(n as u64));
                        let item = MenuItem::new(text).action(move || state.db_seed_table(n));
                        menu = menu.entry(if local { item } else { item.enabled(false) });
                    }
                    if !local {
                        menu = menu.entry(
                            MenuItem::new("Seeding runs on local databases only").enabled(false),
                        );
                    }
                    menu = menu.separator().entry(
                        MenuItem::new(format!("Open {model}Factory"))
                            .action(move || state.open_path(path.clone())),
                    );
                }
                Some((model, _)) => {
                    menu = menu
                        .entry(
                            MenuItem::new(format!("Generate {model}Factory from the table"))
                                .action(move || state.db_generate_factory()),
                        )
                        .entry(MenuItem::new("Seeding needs a factory").enabled(false));
                }
                None => {
                    menu = menu.entry(MenuItem::new("Not a Laravel project").enabled(false));
                }
            }
            menu
        });

    // A plan shows as a tree, or as the rows the engine gave (DB-602).
    let plan_toggle = label(move || {
        if state.db.plan_as_table.get() {
            "Show as tree".to_string()
        } else {
            "Show as table".to_string()
        }
    })
    .style(move |s| {
        let s = s
            .padding_horiz(8.0)
            .padding_vert(2.0)
            .border_radius(4.0)
            .font_size(11.0)
            .color(theme::fg_dim())
            .cursor(floem::style::CursorStyle::Pointer)
            .hover(|s| s.background(theme::bg_hover()).color(theme::fg()));
        if active_plan(state).is_some() {
            s
        } else {
            s.hide()
        }
    })
    .on_click_stop(move |_| state.db.plan_as_table.update(|t| *t = !*t));

    let toolbar = stack((
        subview_chips,
        plan_toggle,
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
            move || (state.db.subview.get(), shown_plan(state)),
            move |(v, plan)| match plan {
                Some(plan) => plan_tree(plan).into_any(),
                None if v == "structure" => structure_grid(state).into_any(),
                None => result_grid(state).into_any(),
            },
        )
        .style(move |s| {
            let w = if shown_plan(state).is_some() {
                900.0
            } else if state.db.subview.get() == "structure" {
                520.0
            } else {
                let n = state
                    .db
                    .result
                    .with(|r| r.as_ref().map(|r| r.columns.len()).unwrap_or(0));
                (n.max(1) as f64) * 180.0
            };
            s.items_start().width(w)
        }),
    )
    .scroll_delta(move || {
        let (x, y, _) = state.db.scroll.get();
        floem::kurbo::Vec2::new(x, y)
    })
    .style(|s| s.flex_grow(1.0_f32).width_full())
    .keyboard_navigable()
    .request_focus(move || {
        state.db.result_open.get();
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
        conflict_bar(state),
        pending_bar(state),
        db_value_viewer(state),
        db_edit_popup(state),
        db_insert_popup(state),
        db_import_popup(state),
        db_related_popup(state),
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
        if state.db.result_open.get() {
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
        .on_click_stop(move |_| state.db.erd_open.set(false));
    let header = stack((
        label(|| "Schema relationships".to_string()).style(|s| {
            s.flex_grow(1.0_f32)
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
            let mut fks = state.db.erd.get();
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
                s.font_family(theme::mono_family())
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
        if state.db.erd.with(|f| f.is_empty()) {
            s
        } else {
            s.hide()
        }
    });
    let list = scroll(stack((rows, empty_hint)).style(|s| s.flex_col().width_full()))
        .style(|s| s.flex_grow(1.0_f32).width_full());
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
        if state.db.erd_open.get() {
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
                .db
                .params
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
                        .flex_shrink(0.0_f32)
                        .font_family(theme::mono_family())
                        .font_size(12.0)
                        .color(theme::accent())
                }),
                text_input(sig)
                    .style(|s| {
                        theme::input_colors(s)
                            .flex_grow(1.0_f32)
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
                .color(theme::on_accent())
                .cursor(floem::style::CursorStyle::Pointer)
        })
        .on_click_stop(move |_| state.db_params_run());
    let buttons = stack((empty().style(|s| s.flex_grow(1.0_f32)), cancel, run)).style(|s| {
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
        if state.db.params.get().is_some() {
            s
        } else {
            s.hide()
        }
    })
}

/// Confirmation dialog for destructive or non-local console runs (DB-702/703).
pub fn db_confirm_dialog(state: AppState) -> impl IntoView {
    let title = label(move || match state.db.confirm.get() {
        Some(c) => format!("{} on {}?", c.verb, c.env.label().to_uppercase()),
        None => String::new(),
    })
    .style(move |s| {
        let color = state
            .db
            .confirm
            .get()
            .map(|c| env_color(c.env))
            .unwrap_or(theme::fg());
        s.font_size(15.0)
            .font_bold()
            .color(color)
            .margin_bottom(6.0)
    });
    let subtitle = label(move || match state.db.confirm.get() {
        Some(c) => {
            // A console run lists only the statements that need a look;
            // the others run with them.
            let all = match &c.run {
                crate::state::ConfirmRun::Console(sql) => e_db::split_statements(sql).len(),
                crate::state::ConfirmRun::Transaction(_) => c.statements.len(),
            };
            let shown = c.statements.len();
            let rows = rows_summary(&c.counts.get());
            if all > shown {
                let these = if shown == 1 {
                    "this one needs".to_string()
                } else {
                    format!("these {shown} need")
                };
                format!("{all} statements will run on this database; {these} a look{rows}:")
            } else {
                format!("{shown} statement(s) will run on this database{rows}. Review them:")
            }
        }
        None => String::new(),
    })
    .style(|s| s.font_size(12.0).color(theme::fg_dim()).margin_bottom(10.0));

    let list = dyn_stack(
        move || {
            state
                .db
                .confirm
                .get()
                .map(|c| {
                    c.statements
                        .into_iter()
                        .enumerate()
                        .map(|(i, s)| (i, s, c.counts))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        },
        |(i, stmt, _)| (*i, stmt.clone()),
        move |(i, stmt, counts)| {
            let text = label(move || stmt.clone()).style(|s| {
                s.font_family(theme::mono_family())
                    .font_size(12.0)
                    .color(theme::error())
                    .flex_grow(1.0_f32)
                    .min_width(0.0)
                    .text_ellipsis()
            });
            // The rows this one touches (DB-703), right-aligned.
            let rows = label(move || {
                counts.with(|c| c.get(i).copied().map(estimate_label).unwrap_or_default())
            })
            .style(|s| {
                s.font_size(11.0)
                    .color(theme::fg_dim())
                    .flex_shrink(0.0_f32)
            });
            stack((text, rows)).style(|s| {
                s.flex_row()
                    .items_center()
                    .gap(10.0)
                    .padding_vert(2.0)
                    .width_full()
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

    let ack = label(move || match state.db.confirm.get() {
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
            .db
            .confirm
            .with(|c| c.as_ref().map(|c| c.needs_ack).unwrap_or(false))
        {
            s
        } else {
            s.hide()
        }
    })
    .on_click_stop(move |_| {
        if let Some(c) = state.db.confirm.get_untracked() {
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
    let confirm = label(move || match state.db.confirm.get() {
        // Every statement that runs, not just the ones listed.
        Some(c) => {
            let all = match &c.run {
                crate::state::ConfirmRun::Console(sql) => e_db::split_statements(sql).len(),
                crate::state::ConfirmRun::Transaction(_) => c.statements.len(),
            };
            format!("{} {all} statement(s)", c.verb)
        }
        None => "Run".to_string(),
    })
    .style(move |s| {
        let color = state
            .db
            .confirm
            .get()
            .map(|c| env_color(c.env))
            .unwrap_or(theme::accent());
        s.padding_horiz(14.0)
            .height(30.0)
            .items_center()
            .border_radius(5.0)
            .font_size(12.0)
            .background(color)
            .color(theme::on_accent())
            .cursor(floem::style::CursorStyle::Pointer)
    })
    .on_click_stop(move |_| state.db_confirm_run());
    let buttons = stack((empty().style(|s| s.flex_grow(1.0_f32)), cancel, confirm)).style(|s| {
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
        if state.db.confirm.get().is_some() {
            s
        } else {
            s.hide()
        }
    })
}

pub fn db_consent_dialog(state: AppState) -> impl IntoView {
    let title = label(|| "Agent wants to run a query".to_string())
        .style(|s| s.font_size(14.0).font_bold().color(theme::fg()));
    // Which database, in its environment's colour: an agent's query on
    // production should look like one.
    let subtitle = label(move || match state.db.consent.get() {
        Some(c) => format!(
            "on “{}” · {} — allow?",
            c.db_name,
            c.env.label().to_uppercase()
        ),
        None => String::new(),
    })
    .style(move |s| {
        let color = state
            .db
            .consent
            .with(|c| c.as_ref().map(|c| env_color(c.env)))
            .unwrap_or(theme::fg_dim());
        s.font_size(12.0).color(color).margin_bottom(8.0)
    });

    let sql = label(move || state.db.consent.get().map(|c| c.sql).unwrap_or_default()).style(|s| {
        theme::input_colors(s)
            .width_full()
            .font_family(theme::mono_family())
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
                .color(theme::on_accent())
                .cursor(floem::style::CursorStyle::Pointer)
        })
        .on_click_stop(move |_| state.db_consent_allow());
    let buttons = stack((empty().style(|s| s.flex_grow(1.0_f32)), deny, allow))
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
        if state.db.consent.get().is_some() {
            s
        } else {
            s.hide()
        }
    })
}

/// The inline cell-edit popup (double-click a cell in a browsed table).
fn db_edit_popup(state: AppState) -> impl IntoView {
    let title = label(move || match state.db.edit.get() {
        Some((_, _, col)) => format!("Edit  {col}"),
        None => String::new(),
    })
    .style(|s| {
        s.font_size(13.0)
            .font_bold()
            .color(theme::fg())
            .margin_bottom(8.0)
    });

    // A multi-line editor for the value (DB-304), made afresh for each cell
    // opened: ⌘↵ stages it, Esc cancels. Long text and JSON fit.
    let input = dyn_container(
        move || state.db.edit.get().map(|(r, c, _)| (r, c)),
        move |at| {
            if at.is_none() {
                return empty().into_any();
            }
            use floem::views::editor::command::CommandExecuted;
            use floem::views::editor::keypress::{default_key_handler, key::KeyInput};
            // ⌘↵ and Esc are the dialog's, taken before the editor would
            // make them a new line or a cleared selection.
            let editor = floem::views::text_editor_keys(
                state.db.edit_value.get_untracked(),
                move |editor_sig, kp, mods| {
                    use floem::keyboard::{Key, NamedKey};
                    if let KeyInput::Keyboard(key, _) = &kp.key {
                        if matches!(key, Key::Named(NamedKey::Enter))
                            && (mods.meta() || mods.control())
                        {
                            state.db_commit_edit();
                            return CommandExecuted::Yes;
                        }
                        if matches!(key, Key::Named(NamedKey::Escape)) {
                            state.db_cancel_edit();
                            return CommandExecuted::Yes;
                        }
                    }
                    default_key_handler(editor_sig)(kp, mods)
                },
            );
            let doc = editor.doc();
            editor
                .update(move |_| state.db.edit_value.set(doc.text().to_string()))
                .style(|s| {
                    s.width_full()
                        .height(180.0)
                        .font_family(theme::mono_family())
                        .font_size(13.0)
                        .padding(6.0)
                })
                .into_any()
        },
    )
    .style(move |s| {
        let s = s
            .width_full()
            .border(1.0)
            .border_color(theme::border())
            .border_radius(5.0)
            .background(theme::bg_panel());
        // NULL wins over the text, which is kept in case it's unticked.
        if state.db.edit_null.get() {
            s.border_color(theme::fg_dim())
                .background(theme::bg_hover())
        } else {
            s
        }
    });

    let null_toggle = label(move || {
        if state.db.edit_null.get() {
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
    .on_click_stop(move |_| state.db.edit_null.update(|n| *n = !*n));

    let save = label(|| "Save  ⌘↵".to_string())
        .style(|s| {
            s.padding_horiz(14.0)
                .height(28.0)
                .items_center()
                .border_radius(5.0)
                .font_size(12.0)
                .background(theme::accent())
                .color(theme::on_accent())
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
                .color(theme::error())
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
        empty().style(|s| s.flex_grow(1.0_f32)),
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
            .width(560.0)
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
        if state.db.edit.get().is_some() {
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
            state.db.result_table.get().unwrap_or_default()
        )
    })
    .style(|s| {
        s.font_size(13.0)
            .font_bold()
            .color(theme::fg())
            .margin_bottom(10.0)
    });

    let fields = dyn_stack(
        move || state.db.insert_fields.get(),
        |f| f.name.clone(),
        move |f| {
            let name = f.name.clone();
            let dt = f.data_type.clone();
            let nullable = f.nullable;
            let val = f.value;
            let is_null = f.is_null;
            let name_lbl = label(move || format!("{name}  {dt}")).style(|s| {
                s.width(150.0)
                    .flex_shrink(0.0_f32)
                    .font_size(11.0)
                    .text_ellipsis()
                    .color(theme::fg_dim())
            });
            let input = text_input(val).placeholder("value").style(move |s| {
                let s = theme::input_colors(s)
                    .flex_grow(1.0_f32)
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
                    .flex_shrink(0.0_f32)
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
    let save = label(|| "Add row".to_string())
        .style(|s| {
            s.padding_horiz(14.0)
                .height(28.0)
                .items_center()
                .border_radius(5.0)
                .font_size(12.0)
                .background(theme::accent())
                .color(theme::on_accent())
                .cursor(floem::style::CursorStyle::Pointer)
        })
        .on_click_stop(move |_| state.db_commit_insert());
    let buttons = stack((empty().style(|s| s.flex_grow(1.0_f32)), cancel, save)).style(|s| {
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
        if state.db.insert_open.get() {
            s
        } else {
            s.hide()
        }
    })
}

/// Related's choice, when more than one table points at the row (DB-308):
/// each with how many of its rows do. A click opens it filtered to them.
fn db_related_popup(state: AppState) -> impl IntoView {
    let card = dyn_container(
        move || state.db.related.get(),
        move |choice| {
            let Some(choice) = choice else {
                return empty().into_any();
            };
            let row = choice.row.clone();
            let title = label(move || format!("Rows that point at {row}")).style(|s| {
                s.font_size(13.0)
                    .font_bold()
                    .color(theme::fg())
                    .margin_bottom(10.0)
            });
            let items = choice
                .refs
                .iter()
                .cloned()
                .map(|(table, col, n)| {
                    let (key, value) = (choice.key.clone(), choice.value.clone());
                    let none = n == Some(0);
                    let count = match n {
                        Some(n) => {
                            format!("{} row{}", group_digits(n), if n == 1 { "" } else { "s" })
                        }
                        None => "?".to_string(),
                    };
                    let name = format!("{table}.{col}");
                    stack((
                        label(move || name.clone()).style(|s| {
                            s.flex_grow(1.0_f32)
                                .font_size(12.0)
                                .font_family(theme::mono_family())
                                .text_ellipsis()
                        }),
                        label(move || count.clone())
                            .style(|s| s.font_size(11.0).color(theme::fg_dim())),
                    ))
                    .style(move |s| {
                        s.flex_row()
                            .items_center()
                            .gap(12.0)
                            .width_full()
                            .padding_horiz(10.0)
                            .padding_vert(6.0)
                            .border_radius(5.0)
                            .color(if none { theme::fg_dim() } else { theme::fg() })
                            .cursor(floem::style::CursorStyle::Pointer)
                            .hover(|s| s.background(theme::bg_hover()))
                    })
                    .on_click_stop(move |_| {
                        state.db_open_related(&key, table.clone(), col.clone(), value.clone())
                    })
                    .into_any()
                })
                .collect::<Vec<_>>();
            let list = scroll(floem::views::v_stack_from_iter(items).style(|s| s.width_full()))
                .style(|s| s.width_full().max_height(320.0));
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
                .on_click_stop(move |_| state.db.related.set(None));
            let buttons = stack((empty().style(|s| s.flex_grow(1.0_f32)), cancel))
                .style(|s| s.flex_row().width_full().margin_top(12.0));
            stack((title, list, buttons))
                .style(|s| {
                    s.flex_col()
                        .width(440.0)
                        .padding(16.0)
                        .border(1.0)
                        .border_color(theme::border())
                        .border_radius(10.0)
                        .background(theme::bg())
                })
                .into_any()
        },
    );
    container(card).style(move |s| {
        let s = s
            .absolute()
            .inset(0.0)
            .size_full()
            .items_center()
            .justify_center()
            .background(Color::from_rgba8(0, 0, 0, 120));
        if state.db.related.with(Option::is_some) {
            s
        } else {
            s.hide()
        }
    })
}

/// The CSV import dialog (DB-503): where each CSV column goes, a preview of
/// the first rows as they'll go in, and what doesn't fit the table.
fn db_import_popup(state: AppState) -> impl IntoView {
    let card = dyn_container(
        move || state.db.import.get(),
        move |imp| match imp {
            Some(imp) => import_card(state, imp).into_any(),
            None => empty().into_any(),
        },
    );
    container(card).style(move |s| {
        let s = s
            .absolute()
            .inset(0.0)
            .size_full()
            .items_center()
            .justify_center()
            .background(Color::from_rgba8(0, 0, 0, 120));
        if state.db.import.with(Option::is_some) {
            s
        } else {
            s.hide()
        }
    })
}

fn import_card(state: AppState, imp: crate::state::DbImport) -> impl IntoView {
    let crate::state::DbImport {
        table,
        file,
        delimiter,
        header,
        rows,
        columns,
        mapping,
        empty_default,
        ..
    } = imp;
    let n = rows.len();
    let title = label(move || format!("Import “{file}” into {table}")).style(|s| {
        s.font_size(13.0)
            .font_bold()
            .color(theme::fg())
            .text_ellipsis()
    });
    let delim_name = match delimiter {
        ';' => "semicolons",
        '\t' => "tabs",
        _ => "commas",
    };
    let subtitle = label(move || {
        format!(
            "{} row{} · fields separated by {delim_name} · line 1 is the header",
            group_digits(n as u64),
            if n == 1 { "" } else { "s" }
        )
    })
    .style(|s| s.font_size(11.0).color(theme::fg_dim()).margin_bottom(10.0));

    // One line per CSV column: its name, where it goes, a few of its values.
    let map_rows = header
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let name = name.clone();
            let samples = rows
                .iter()
                .filter_map(|r| r.get(i).filter(|v| !v.is_empty()))
                .take(3)
                .map(|v| v.chars().take(24).collect::<String>())
                .collect::<Vec<_>>()
                .join(" · ");
            let cols = columns.clone();
            let target = label(
                move || match mapping.with(|m| m.get(i).cloned().flatten()) {
                    Some(c) => format!("→ {c} ▾"),
                    None => "skip ▾".to_string(),
                },
            )
            .style(move |s| {
                let mapped = mapping.with(|m| m.get(i).is_some_and(Option::is_some));
                s.width(190.0)
                    .flex_shrink(0.0_f32)
                    .padding_horiz(8.0)
                    .padding_vert(3.0)
                    .font_size(12.0)
                    .text_ellipsis()
                    .border(1.0)
                    .border_radius(5.0)
                    .border_color(theme::border())
                    .color(if mapped { theme::fg() } else { theme::fg_dim() })
                    .cursor(floem::style::CursorStyle::Pointer)
                    .hover(|s| s.background(theme::bg_hover()))
            })
            .popout_menu(move || {
                use floem::menu::{Menu, MenuItem};
                let mut menu = Menu::new("").entry(
                    MenuItem::new("Skip this column")
                        .action(move || mapping.update(|m| m[i] = None)),
                );
                for c in &cols {
                    let col = c.name.clone();
                    menu = menu.entry(
                        MenuItem::new(format!("{}  ·  {}", c.name, c.data_type))
                            .action(move || mapping.update(|m| m[i] = Some(col.clone()))),
                    );
                }
                menu
            });
            stack((
                label(move || name.clone()).style(|s| {
                    s.width(170.0)
                        .flex_shrink(0.0_f32)
                        .font_size(12.0)
                        .font_family(theme::mono_family())
                        .color(theme::fg())
                        .text_ellipsis()
                }),
                target,
                label(move || samples.clone()).style(|s| {
                    s.flex_grow(1.0_f32)
                        .min_width(0.0)
                        .font_size(11.0)
                        .color(theme::fg_dim())
                        .text_ellipsis()
                }),
            ))
            .style(|s| s.flex_row().items_center().gap(10.0).padding_vert(3.0))
            .into_any()
        })
        .collect::<Vec<_>>();
    let map_list = scroll(floem::views::v_stack_from_iter(map_rows).style(|s| s.width_full()))
        .style(|s| s.width_full().max_height(220.0));

    // Empty fields: NULL, or the column's default.
    let choice = move |text: &'static str, default: bool| {
        label(move || text.to_string())
            .style(move |s| {
                let on = empty_default.get() == default;
                let s = s
                    .padding_horiz(10.0)
                    .padding_vert(3.0)
                    .font_size(11.0)
                    .border(1.0)
                    .border_radius(5.0)
                    .cursor(floem::style::CursorStyle::Pointer);
                if on {
                    s.border_color(theme::accent()).color(theme::fg())
                } else {
                    s.border_color(theme::border()).color(theme::fg_dim())
                }
            })
            .on_click_stop(move |_| empty_default.set(default))
    };
    let empties = stack((
        label(|| "Empty fields go in as".to_string())
            .style(|s| s.font_size(11.0).color(theme::fg_dim())),
        choice("NULL", false),
        choice("the column's default", true),
    ))
    .style(|s| s.flex_row().items_center().gap(8.0).margin_top(10.0));

    // The first rows as they'll go in, under the columns they go into.
    let preview_rows = rows.clone();
    let preview = dyn_container(
        move || (mapping.get(), empty_default.get()),
        move |(m, empty_default)| {
            let cols: Vec<(usize, String)> = m
                .iter()
                .enumerate()
                .filter_map(|(i, c)| c.clone().map(|c| (i, c)))
                .collect();
            if cols.is_empty() {
                return label(|| "Nothing is mapped yet.".to_string())
                    .style(|s| s.font_size(11.0).color(theme::fg_dim()))
                    .into_any();
            }
            let cell = |text: String, dim: bool, head: bool| {
                label(move || text.clone()).style(move |s| {
                    let s = s
                        .width(130.0)
                        .flex_shrink(0.0_f32)
                        .padding_horiz(6.0)
                        .padding_vert(2.0)
                        .font_size(11.0)
                        .text_ellipsis()
                        .border_right(1.0)
                        .border_color(theme::border());
                    let s = if head { s.font_bold() } else { s };
                    s.color(if dim { theme::fg_dim() } else { theme::fg() })
                })
            };
            let head = floem::views::h_stack_from_iter(
                cols.iter().map(|(_, c)| cell(c.clone(), false, true)),
            )
            .style(|s| {
                s.border_bottom(1.0)
                    .border_color(theme::border())
                    .background(theme::bg_hover())
            });
            let body = preview_rows.iter().take(5).map(|r| {
                floem::views::h_stack_from_iter(cols.iter().map(|(i, _)| {
                    match r.get(*i).filter(|v| !v.is_empty()) {
                        Some(v) => cell(v.clone(), false, false),
                        None if empty_default => cell("default".into(), true, false),
                        None => cell("NULL".into(), true, false),
                    }
                }))
                .style(|s| s.border_bottom(1.0).border_color(theme::border()))
            });
            scroll(
                floem::views::v_stack_from_iter(
                    std::iter::once(head.into_any()).chain(body.map(|r| r.into_any())),
                )
                .style(|s| s.border(1.0).border_color(theme::border())),
            )
            .style(|s| s.width_full().max_height(150.0))
            .into_any()
        },
    )
    .style(|s| s.width_full());
    let preview_title = label(|| "Preview".to_string()).style(|s| {
        s.font_size(11.0)
            .font_bold()
            .color(theme::fg_dim())
            .margin_top(12.0)
            .margin_bottom(4.0)
    });

    // What doesn't fit, worked out again as the mapping changes.
    let check_rows = rows.clone();
    let check_cols = columns.clone();
    let width = header.len();
    let problems = move || {
        crate::db_import::problems(
            &check_rows,
            width,
            &mapping.get(),
            &check_cols,
            empty_default.get(),
        )
    };
    let problems = std::rc::Rc::new(problems);
    let shown = problems.clone();
    let problem_list = label(move || {
        shown()
            .iter()
            .map(|p| format!("{} {}", if p.blocking { "✕" } else { "⚠" }, p.text))
            .collect::<Vec<_>>()
            .join("\n")
    })
    .style(move |s| {
        s.font_size(11.0)
            .color(theme::warning())
            .margin_top(10.0)
            .width_full()
    });
    let blocked = {
        let problems = problems.clone();
        move || {
            mapping.with(|m| m.iter().all(Option::is_none)) || problems().iter().any(|p| p.blocking)
        }
    };
    let blocked_for_style = blocked.clone();

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
        .on_click_stop(move |_| state.db_cancel_import());
    let import = label(move || {
        format!(
            "Import {} row{}…",
            group_digits(n as u64),
            if n == 1 { "" } else { "s" }
        )
    })
    .style(move |s| {
        let s = s
            .padding_horiz(14.0)
            .height(28.0)
            .items_center()
            .border_radius(5.0)
            .font_size(12.0)
            .background(theme::accent())
            .color(theme::on_accent());
        if blocked_for_style() {
            s.background(theme::bg_hover()).color(theme::fg_dim())
        } else {
            s.cursor(floem::style::CursorStyle::Pointer)
        }
    })
    .on_click_stop(move |_| {
        if !blocked() {
            state.db_commit_import()
        }
    });
    let buttons = stack((empty().style(|s| s.flex_grow(1.0_f32)), cancel, import)).style(|s| {
        s.flex_row()
            .items_center()
            .gap(8.0)
            .width_full()
            .margin_top(14.0)
    });

    stack((
        title,
        subtitle,
        map_list,
        empties,
        preview_title,
        preview,
        problem_list,
        buttons,
    ))
    .style(|s| {
        s.flex_col()
            .width(760.0)
            .padding(16.0)
            .border(1.0)
            .border_color(theme::border())
            .border_radius(10.0)
            .background(theme::bg())
    })
}

/// Pretty-print a cell value for the viewer: JSON objects/arrays are
/// re-indented; everything else is shown verbatim.
/// A cell of a row staged for inserting.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum NewCell {
    Value(String),
    Null,
    /// Left out: the database fills it in.
    Default,
}

/// The cells of a staged row under the grid's `columns`.
fn new_row_cells(columns: &[String], values: &[(String, Option<String>)]) -> Vec<NewCell> {
    columns
        .iter()
        .map(|c| match values.iter().find(|(n, _)| n == c) {
            Some((_, Some(v))) => NewCell::Value(v.clone()),
            Some((_, None)) => NewCell::Null,
            None => NewCell::Default,
        })
        .collect()
}

/// One statement's rows in the confirmation dialog.
fn estimate_label(e: crate::state::RowEstimate) -> String {
    use crate::state::RowEstimate;
    match e {
        RowEstimate::Counting => "counting…".into(),
        RowEstimate::Known(e_db::Counted::Exactly(1)) => "1 row".into(),
        RowEstimate::Known(e_db::Counted::Exactly(n)) => format!("{} rows", group_digits(n)),
        RowEstimate::Known(e_db::Counted::Over) => {
            format!("{}+ rows", group_digits(e_db::COUNT_CAP))
        }
        RowEstimate::Unknown => "rows not counted".into(),
    }
}

/// The rows all the statements touch, for the dialog's subtitle: empty
/// until something is known.
fn rows_summary(counts: &[crate::state::RowEstimate]) -> String {
    use crate::state::RowEstimate;
    let mut total = 0u64;
    let (mut over, mut unknown, mut known) = (false, false, false);
    for c in counts {
        match c {
            RowEstimate::Known(e_db::Counted::Exactly(n)) => {
                total += n;
                known = true;
            }
            RowEstimate::Known(e_db::Counted::Over) => {
                total += e_db::COUNT_CAP;
                over = true;
                known = true;
            }
            RowEstimate::Counting => return ", counting the rows it touches…".into(),
            RowEstimate::Unknown => unknown = true,
        }
    }
    if !known {
        return String::new();
    }
    let n = group_digits(total);
    let rows = if total == 1 && !over { "row" } else { "rows" };
    match (over, unknown) {
        (true, _) => format!(", touching over {n} rows"),
        (false, true) => format!(", touching at least {n} {rows} (some not counted)"),
        (false, false) => format!(", touching {n} {rows}"),
    }
}

/// `12345` as `12,345`.
fn group_digits(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A value as the viewer shows it: JSON laid out, keys in their own order.
pub(crate) fn pretty_value(s: &str) -> String {
    crate::json_layout::pretty(s).unwrap_or_else(|| s.to_string())
}

/// A read-only inspector for the selected cell (full value, pretty-printed JSON)
/// docked at the bottom of the result panel — like PhpStorm's value viewer.
fn db_value_viewer(state: AppState) -> impl IntoView {
    let head = label(move || match state.db.selected_cell.get() {
        Some((r, c)) => {
            let col = state.db.result.with(|res| {
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

    let body = label(move || match state.db.selected_cell.get() {
        Some((r, c)) => state.db.result.with(|res| {
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
        s.font_family(theme::mono_family())
            .font_size(12.0)
            .color(theme::fg())
            .padding(10.0)
    });
    let body = scroll(body).style(|s| s.flex_grow(1.0_f32).width_full());

    let close = label(|| "✕".to_string())
        .style(|s| {
            s.padding_horiz(8.0)
                .color(theme::fg_dim())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.color(theme::fg()))
        })
        .on_click_stop(move |_| state.db.selected_cell.set(None));
    let head_row = stack((head.style(|s| s.flex_grow(1.0_f32)), close)).style(|s| {
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
            .flex_shrink(0.0_f32)
            .border_top(1.0)
            .border_color(theme::border())
            .background(theme::bg_panel());
        if state.db.selected_cell.get().is_some() {
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
        .on_click_stop(move |_| state.db.write_log_open.set(false));
    let header = stack((
        label(|| "Session write log".to_string()).style(|s| {
            s.flex_grow(1.0_f32)
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
            let mut v = state.db.write_log.get();
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
                        .flex_shrink(0.0_f32)
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
                    s.flex_grow(1.0_f32)
                        .font_family(theme::mono_family())
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
        if state.db.write_log.with(|l| l.is_empty()) {
            s
        } else {
            s.hide()
        }
    });
    let list = scroll(stack((rows, empty_hint)).style(|s| s.flex_col().width_full()))
        .style(|s| s.flex_grow(1.0_f32).width_full());
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
        if state.db.write_log_open.get() {
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
        let _ = state.db.history_query.get();
        if state.db.history_open.get_untracked() {
            state.db_reload_history();
        }
    });

    let search = text_input(state.db.history_query)
        .placeholder("Search history…")
        .style(|s| {
            theme::input_colors(s)
                .flex_grow(1.0_f32)
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
        .on_click_stop(move |_| state.db.history_open.set(false));
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
                .db
                .history
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
                        .font_family(theme::mono_family())
                        .font_size(12.0)
                        .text_ellipsis()
                        .color(if ok { theme::fg() } else { theme::error() })
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
        if state.db.history.with(|h| h.is_empty()) {
            s
        } else {
            s.hide()
        }
    });
    let list = scroll(stack((rows, empty_hint)).style(|s| s.flex_col().width_full()))
        .style(|s| s.flex_grow(1.0_f32).width_full());

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
        if state.db.history_open.get() {
            s
        } else {
            s.hide()
        }
    })
}

/// The transactional "pending changes" bar: shown at the bottom of the grid
/// while there are staged edits/deletes, with Submit (one transaction, via the
/// confirmation dialog) and Revert.
/// A statement as a tab's tooltip: trimmed, at most a dozen lines of up to
/// 120 characters each.
fn tab_tooltip(sql: &str) -> String {
    const LINES: usize = 12;
    const WIDTH: usize = 120;
    let lines: Vec<&str> = sql.trim().lines().collect();
    let mut out: Vec<String> = lines
        .iter()
        .take(LINES)
        .map(|l| {
            if l.chars().count() > WIDTH {
                format!("{}…", l.chars().take(WIDTH).collect::<String>())
            } else {
                l.to_string()
            }
        })
        .collect();
    if lines.len() > LINES {
        out.push(format!("… {} more lines", lines.len() - LINES));
    }
    out.join("\n")
}

/// What the last Submit found changed in the database since the grid read
/// it: nothing was written, and the changes it names are no longer staged.
fn conflict_bar(state: AppState) -> impl IntoView {
    let text =
        label(move || {
            let lines = state.db.conflicts.get();
            let n = lines.len();
            let head =
                format!(
            "✕ Nothing was written: {n} row{} changed since {} read. The table now shows {}.",
            if n == 1 { "" } else { "s" },
            if n == 1 { "it was" } else { "they were" },
            if n == 1 { "its current value" } else { "their current values" },
        );
            std::iter::once(head)
                .chain(lines.iter().take(5).map(|l| format!("   {l}")))
                .chain((n > 5).then(|| format!("   … and {} more", n - 5)))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .style(|s| s.flex_grow(1.0_f32).font_size(12.0).color(theme::error()));
    let dismiss = label(|| "✕".to_string())
        .style(|s| {
            s.padding_horiz(8.0)
                .height(24.0)
                .items_center()
                .border_radius(4.0)
                .font_size(12.0)
                .color(theme::fg_dim())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::bg_hover()))
        })
        .on_click_stop(move |_| state.db.conflicts.set(Vec::new()));
    stack((text, dismiss)).style(move |s| {
        let s = s
            .flex_row()
            .items_start()
            .gap(8.0)
            .width_full()
            .padding_horiz(12.0)
            .padding_vert(8.0)
            .border_top(1.0)
            .border_color(theme::error())
            .background(theme::bg_panel());
        if state.db.conflicts.with(Vec::is_empty) {
            s.hide()
        } else {
            s
        }
    })
}

fn pending_bar(state: AppState) -> impl IntoView {
    let summary = label(move || {
        let e = state.db.pending_edits.with(|m| m.len());
        let d = state.db.pending_deletes.with(|m| m.len());
        let n = state.db.pending_inserts.with(|v| v.len());
        let total = e + d + n;
        // The changes stay staged while another table or a console result is
        // shown, so say which table they are for.
        let table = state
            .db
            .pending_target
            .with(|t| t.as_ref().map(|(_, t)| t.clone()))
            .unwrap_or_default();
        format!(
            "⚠ {total} pending change{} to {table}  ·  {e} update{}, {n} new row{}, {d} delete{}",
            if total == 1 { "" } else { "s" },
            if e == 1 { "" } else { "s" },
            if n == 1 { "" } else { "s" },
            if d == 1 { "" } else { "s" },
        )
    })
    .style(|s| s.flex_grow(1.0_f32).font_size(12.0).color(theme::warning()));
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
                .color(theme::on_accent())
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
        let any = state.db.pending_edits.with(|m| !m.is_empty())
            || state.db.pending_deletes.with(|m| !m.is_empty())
            || state.db.pending_inserts.with(|v| !v.is_empty());
        if any {
            s
        } else {
            s.hide()
        }
    })
}

/// The plan of the shown tab, as a tree, if it has one.
fn active_plan(state: AppState) -> Option<e_db::PlanNode> {
    let i = state.db.active_tab.get()?;
    state.db.result_tabs.with(|t| t.get(i)?.plan.clone())
}

/// The plan to show as a tree, unless it's to be shown as rows.
fn shown_plan(state: AppState) -> Option<e_db::PlanNode> {
    (!state.db.plan_as_table.get())
        .then(|| active_plan(state))
        .flatten()
}

/// A plan as a tree (DB-602): each step under the one it feeds, full scans
/// in red, the step that costs the most on its own marked, and the cost and
/// rows the engine estimates.
fn plan_tree(plan: e_db::PlanNode) -> impl IntoView {
    let costliest = plan.costliest().map(|n| n as *const e_db::PlanNode);
    let rows = plan
        .flatten()
        .into_iter()
        .map(|(depth, n)| {
            let indent = depth as f64 * 18.0;
            let branch = if depth == 0 { "" } else { "└ " };
            let title = format!("{branch}{}", n.label);
            let full_scan = n.full_scan;
            let costly = costliest == Some(n as *const e_db::PlanNode);
            let mut marks = Vec::new();
            if full_scan {
                marks.push("✖ full scan");
            }
            if costly {
                marks.push("▲ costliest step");
            }
            let marks = marks.join("  ");
            let numbers = [
                n.cost.map(|c| format!("cost {}", plan_number(c))),
                n.rows.map(|r| format!("{} rows", plan_number(r))),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ");
            let details = n.details.join("\n");
            let head = stack((
                label(move || title.clone()).style(move |s| {
                    s.font_size(12.0)
                        .font_family(theme::mono_family())
                        .color(if full_scan {
                            theme::error()
                        } else {
                            theme::fg()
                        })
                }),
                label(move || marks.clone()).style(move |s| {
                    s.font_size(10.5).color(if full_scan {
                        theme::error()
                    } else {
                        theme::warning()
                    })
                }),
                empty().style(|s| s.flex_grow(1.0_f32)),
                label(move || numbers.clone()).style(|s| s.font_size(11.0).color(theme::fg_dim())),
            ))
            .style(|s| s.flex_row().items_center().gap(10.0).width_full());
            let has_details = !details.is_empty();
            let detail = label(move || details.clone()).style(move |s| {
                let s = s
                    .font_size(11.0)
                    .font_family(theme::mono_family())
                    .color(theme::fg_dim())
                    .padding_left(if depth == 0 { 0.0 } else { 14.0 });
                if has_details {
                    s
                } else {
                    s.hide()
                }
            });
            stack((head, detail))
                .style(move |s| {
                    let s = s
                        .flex_col()
                        .gap(2.0)
                        .width_full()
                        .padding_left(12.0 + indent)
                        .padding_right(12.0)
                        .padding_vert(5.0)
                        .border_bottom(1.0)
                        .border_color(theme::border());
                    if costly {
                        s.background(Color::from_rgba8(0xe5, 0xc0, 0x7b, 24))
                    } else {
                        s
                    }
                })
                .into_any()
        })
        .collect::<Vec<_>>();
    floem::views::v_stack_from_iter(rows).style(|s| s.width(900.0))
}

/// A plan's estimate: whole when it is, else to two places.
fn plan_number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        group_digits(n as u64)
    } else {
        format!("{n:.2}")
    }
}

/// A banner listing EXPLAIN findings (full scans / missing indexes) for the
/// current plan, with the index that would help as a migration (DB-603),
/// and a way to ask the agent instead (DB-602).
fn explain_banner(state: AppState) -> impl IntoView {
    let issues = label(move || {
        let list = state.db.explain_issues.get();
        if list.is_empty() {
            String::new()
        } else {
            format!("⚠ {}", list.join("  ·  "))
        }
    })
    .style(|s| {
        s.flex_grow(1.0_f32)
            .font_size(11.0)
            .color(theme::error())
            .text_ellipsis()
    });
    // The index the plan asks for, when one would do (DB-603).
    let advice = move || {
        state
            .db
            .active_tab
            .get()
            .and_then(|i| state.db.result_tabs.with(|t| t.get(i)?.advice.clone()))
    };
    let add = label(move || match advice() {
        Some(a) => format!("+ Index on {} → migration", a.describe()),
        None => String::new(),
    })
    .style(move |s| {
        let s = s
            .font_size(10.5)
            .padding_horiz(8.0)
            .padding_vert(2.0)
            .border(1.0)
            .border_radius(4.0)
            .border_color(theme::border())
            .color(theme::fg())
            .flex_shrink(0.0_f32)
            .cursor(floem::style::CursorStyle::Pointer)
            .hover(|s| s.background(theme::bg_hover()));
        if advice().is_some() {
            s
        } else {
            s.hide()
        }
    })
    .on_click_stop(move |_| state.db_suggest_index_for_plan());
    let hint = label(|| "Ask the agent".to_string())
        .style(|s| {
            s.font_size(10.5)
                .color(theme::fg_dim())
                .flex_shrink(0.0_f32)
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.color(theme::fg()))
        })
        .on_click_stop(move |_| state.db_ask_agent_for_index());
    stack((issues, add, hint)).style(move |s| {
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
        if state.db.explain_issues.with(|i| i.is_empty()) {
            s.hide()
        } else {
            s
        }
    })
}

/// The result-tab strip: the browsed table's tab, then one tab per statement
/// of the last run (or per table a search matched, or a plan), plus pinned
/// tabs. Click to switch, pin to keep, ✕ to close. Hidden when empty.
fn result_tabs_strip(state: AppState) -> impl IntoView {
    // The browsed table's own tab, first, so result tabs don't replace it.
    let table_tab = label(move || {
        state
            .db
            .browsed
            .get()
            .map(|(_, t)| format!("▦ {t}"))
            .unwrap_or_default()
    })
    .style(move |s| {
        let shown = state.db.active_tab.get().is_none() && state.db.result_table.get().is_some();
        let s = s
            .font_size(12.0)
            .padding_horiz(10.0)
            .padding_vert(5.0)
            .border_right(1.0)
            .border_color(theme::border())
            .color(theme::fg())
            .cursor(floem::style::CursorStyle::Pointer);
        let s = if shown {
            s.background(theme::bg_active())
        } else {
            s.hover(|s| s.background(theme::bg_hover()))
        };
        if state.db.browsed.with(|b| b.is_none()) {
            s.hide()
        } else {
            s
        }
    })
    .on_click_stop(move |_| state.db_show_browsed());
    let tabs = dyn_stack(
        move || {
            state
                .db
                .result_tabs
                .get()
                .into_iter()
                .enumerate()
                .collect::<Vec<_>>()
        },
        // The statement is in the key: a new run's "Result 1" is a new tab.
        |(i, t)| {
            (
                *i,
                t.title.clone(),
                t.pinned,
                t.error.is_some(),
                t.sql.clone(),
            )
        },
        move |(i, t)| {
            let title = t.title.clone();
            let sql = t.sql.clone();
            let pinned = t.pinned;
            let is_err = t.error.is_some();
            let pin = label(move || if pinned { "★" } else { "☆" }.to_string())
                .style(move |s| {
                    s.font_size(11.0)
                        .color(if pinned {
                            theme::warning()
                        } else {
                            theme::fg_dim()
                        })
                        .cursor(floem::style::CursorStyle::Pointer)
                        .hover(|s| s.color(theme::fg()))
                })
                .on_click_stop(move |_| state.db_toggle_pin(i));
            let name = label(move || title.clone()).style(move |s| {
                s.font_size(12.0)
                    .color(if is_err { theme::error() } else { theme::fg() })
            });
            let close = label(|| "✕".to_string())
                .style(|s| {
                    s.font_size(11.0)
                        .color(theme::fg_dim())
                        .cursor(floem::style::CursorStyle::Pointer)
                        .hover(|s| s.color(theme::fg()))
                })
                .on_click_stop(move |_| state.db_close_tab(i));
            let tab = stack((pin, name, close))
                .style(move |s| {
                    let active = state.db.active_tab.get() == Some(i);
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
                .on_click_stop(move |_| state.db_activate_tab(i));
            // Hovering a tab shows the statement it is the result of (DB-404).
            match sql.as_deref().map(tab_tooltip) {
                Some(text) => floem::views::tooltip(tab, move || {
                    let text = text.clone();
                    label(move || text.clone())
                })
                .style(|s| {
                    s.class(floem::views::TooltipClass, |s| {
                        s.font_size(12.0)
                            .font_family(theme::mono_family())
                            .color(theme::fg())
                            .padding_horiz(10.0)
                            .padding_vert(6.0)
                            .max_width(640.0)
                            .border(1.0)
                            .border_radius(5.0)
                            .border_color(theme::border())
                            .background(theme::bg_panel())
                    })
                })
                .into_any(),
                None => tab.into_any(),
            }
        },
    )
    .style(|s| s.flex_row().items_center());

    floem::views::scroll(stack((table_tab, tabs)).style(|s| s.flex_row().items_center())).style(
        move |s| {
            let s = s
                .width_full()
                .border_bottom(1.0)
                .border_color(theme::border())
                .background(theme::bg_hover());
            if state.db.result_tabs.with(|t| t.is_empty()) {
                s.hide()
            } else {
                s
            }
        },
    )
}

fn result_grid(state: AppState) -> impl IntoView {
    // Header row.
    let header = dyn_stack(
        move || state.db.result.get().map(|r| r.columns).unwrap_or_default(),
        |c| c.clone(),
        move |c| {
            let col = c.clone();
            let col2 = c.clone();
            label(move || {
                let arrow = match state.db.sort.get() {
                    Some((sc, true)) if sc == col2 => " ▲",
                    Some((sc, false)) if sc == col2 => " ▼",
                    _ => "",
                };
                let pk = state
                    .db
                    .columns
                    .with(|cols| cols.iter().any(|c| c.name == col2 && c.key == "PRI"));
                let key = if pk { "🔑 " } else { "" };
                format!("{key}{col2}{arrow}")
            })
            .style(move |s| {
                let s = s
                    .width(180.0)
                    .flex_shrink(0.0_f32)
                    .padding_horiz(8.0)
                    .padding_vert(5.0)
                    .font_size(11.0)
                    .font_bold()
                    .text_ellipsis()
                    .color(theme::fg_dim())
                    .border_right(1.0)
                    .border_color(theme::border());
                if state.db.result_table.get().is_some() {
                    s.cursor(floem::style::CursorStyle::Pointer)
                        .hover(|s| s.color(theme::fg()))
                } else {
                    s
                }
            })
            .on_click_stop(move |_| {
                if state.db.result_table.get_untracked().is_some() {
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
                .db
                .result
                .get()
                .map(|r| r.rows.into_iter().enumerate().collect::<Vec<_>>())
                .unwrap_or_default()
        },
        // By content too: a row's cells are drawn once, so a new result (a
        // tab, a page) with a row at the same place must make a new row.
        |(i, row)| (*i, row.clone()),
        move |(ri, row)| {
            let cells = dyn_stack(
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
                            let selected = state.db.selected_cell.get() == Some((ri, ci));
                            let (pending_edit, pending_del) = state
                                .db
                                .pending_view
                                .with(|v| (v.cells.contains(&(ri, ci)), v.rows.contains(&ri)));
                            let s = s
                                .width(180.0)
                                .flex_shrink(0.0_f32)
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
                                s.color(theme::error())
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
                        .on_click_stop(move |_| state.db.selected_cell.set(Some((ri, ci))))
                        .on_double_click_stop(move |_| state.db_begin_edit(ri, ci))
                },
            )
            .style(|s| s.flex_row().items_center().height(26.0));
            // A row staged for deletion is struck through, across all its
            // cells (DB-303); clicks go through to the cells beneath.
            let strike = empty().style(move |s| {
                let s = s
                    .absolute()
                    .inset_left(4.0)
                    .inset_right(4.0)
                    .inset_top(12.5)
                    .height(1.0)
                    .background(theme::error())
                    .pointer_events_none();
                if state.db.pending_view.with(|v| v.rows.contains(&ri)) {
                    s
                } else {
                    s.hide()
                }
            });
            stack((cells, strike)).style(|s| {
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

    // Rows staged with "+ Row" (DB-303), above the table's own, until
    // Submit inserts them or Revert drops them. A column left out shows the
    // database's default.
    let new_rows = dyn_stack(
        move || {
            let showing = state.db.active_tab.get().is_none()
                && state.db.result_key.get().zip(state.db.result_table.get())
                    == state.db.pending_target.get();
            if !showing {
                return Vec::new();
            }
            let columns = state
                .db
                .result
                .with(|r| r.as_ref().map(|r| r.columns.clone()).unwrap_or_default());
            state.db.pending_inserts.with(|rows| {
                rows.iter()
                    .enumerate()
                    .map(|(i, row)| (i, new_row_cells(&columns, &row.values)))
                    .collect::<Vec<_>>()
            })
        },
        |(i, cells)| (*i, cells.clone()),
        move |(_, cells)| {
            dyn_stack(
                move || cells.clone().into_iter().enumerate().collect::<Vec<_>>(),
                |(i, _)| *i,
                move |(_, cell)| {
                    let (text, given) = match cell {
                        NewCell::Value(v) => (sanitize_cell(&v), true),
                        NewCell::Null => ("NULL".to_string(), false),
                        NewCell::Default => ("default".to_string(), false),
                    };
                    label(move || text.clone()).style(move |s| {
                        let s = s
                            .width(180.0)
                            .flex_shrink(0.0_f32)
                            .height(26.0)
                            .padding_horiz(8.0)
                            .padding_vert(4.0)
                            .font_size(12.0)
                            .text_ellipsis()
                            .border_right(1.0)
                            .border_color(theme::border());
                        if given {
                            s.color(theme::fg())
                        } else {
                            s.color(theme::fg_dim())
                                .font_style(floem::text::Style::Italic)
                        }
                    })
                },
            )
            .style(|s| {
                s.flex_row()
                    .items_center()
                    .height(26.0)
                    .border_bottom(1.0)
                    .border_color(theme::border())
                    .background(Color::from_rgba8(0x98, 0xc3, 0x79, 36))
            })
        },
    )
    .style(|s| s.flex_col());

    stack((header, new_rows, rows)).style(move |s| {
        let n = state
            .db
            .result
            .with(|r| r.as_ref().map(|r| r.columns.len()).unwrap_or(0));
        s.flex_col().width((n.max(1) as f64) * 180.0)
    })
}

/// The structure (columns) view of the browsed table.
fn structure_grid(state: AppState) -> impl IntoView {
    let head_cell = |t: &'static str, w: f64| {
        label(move || t.to_string()).style(move |s| {
            s.width(w)
                .flex_shrink(0.0_f32)
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
                .db
                .columns
                .get()
                .into_iter()
                .enumerate()
                .collect::<Vec<_>>()
        },
        // By content too: another table's column at the same place is a new row.
        |(i, c)| {
            (
                *i,
                c.name.clone(),
                c.data_type.clone(),
                c.nullable,
                c.key.clone(),
            )
        },
        move |(_, c)| {
            let cell = |t: String, w: f64, dim: bool| {
                label(move || t.clone()).style(move |s| {
                    let s = s
                        .width(w)
                        .flex_shrink(0.0_f32)
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

    // The table's DDL, or a view's definition (DB-202/204).
    let def_title = label(move || {
        if state.db.is_view.get() {
            "Definition".to_string()
        } else {
            "DDL".to_string()
        }
    })
    .style(move |s| {
        let s = s
            .font_size(11.0)
            .font_bold()
            .color(theme::fg_dim())
            .padding_horiz(8.0)
            .padding_top(12.0)
            .padding_bottom(4.0);
        if state.db.ddl.with(Option::is_some) {
            s
        } else {
            s.hide()
        }
    });
    let def_text = label(move || match state.db.ddl.get() {
        Some(d) if !d.trim().is_empty() => d.trim().to_string(),
        _ => "(the database gave none)".to_string(),
    })
    .style(move |s| {
        let s = s
            .font_size(12.0)
            .font_family(theme::mono_family())
            .color(theme::fg())
            .padding(8.0)
            .margin_horiz(8.0)
            .border(1.0)
            .border_radius(4.0)
            .border_color(theme::border())
            .background(theme::bg_panel())
            .width(760.0);
        if state.db.ddl.with(Option::is_some) {
            s
        } else {
            s.hide()
        }
    });

    // Indexes section (below the columns grid).
    let idx_title = label(|| "Indexes".to_string()).style(move |s| {
        let s = s
            .font_size(11.0)
            .font_bold()
            .color(theme::fg_dim())
            .padding_horiz(8.0)
            .padding_top(12.0)
            .padding_bottom(4.0);
        if state.db.indexes.with(|i| i.is_empty()) {
            s.hide()
        } else {
            s
        }
    });
    let idx_rows = dyn_stack(
        move || {
            state
                .db
                .indexes
                .get()
                .into_iter()
                .enumerate()
                .collect::<Vec<_>>()
        },
        // By content too: another table's index at the same place is a new row.
        |(i, ix)| (*i, ix.name.clone(), ix.columns.clone(), ix.unique),
        move |(_, ix)| {
            let cols = ix.columns.join(", ");
            let name = ix.name.clone();
            let badge = if ix.unique { "UNIQUE" } else { "INDEX" };
            let copyable = !crate::db_state::is_key_index(&ix.name);
            let to_copy = ix.clone();
            let copy = label(|| "⧉".to_string())
                .style(move |s| {
                    let s = s
                        .padding_horiz(8.0)
                        .padding_vert(4.0)
                        .font_size(11.0)
                        .color(theme::fg_dim())
                        .cursor(floem::style::CursorStyle::Pointer)
                        .hover(|s| s.color(theme::fg()));
                    if copyable {
                        s
                    } else {
                        s.hide()
                    }
                })
                .on_click_stop(move |_| state.db_copy_index(&to_copy));
            stack((
                label(move || badge.to_string()).style(move |s| {
                    s.width(70.0)
                        .flex_shrink(0.0_f32)
                        .padding_horiz(8.0)
                        .padding_vert(4.0)
                        .font_size(10.5)
                        .color(if ix.unique {
                            theme::warning()
                        } else {
                            theme::fg_dim()
                        })
                }),
                label(move || name.clone()).style(|s| {
                    s.width(200.0)
                        .flex_shrink(0.0_f32)
                        .padding_horiz(8.0)
                        .padding_vert(4.0)
                        .font_size(12.0)
                        .color(theme::fg())
                        .text_ellipsis()
                }),
                label(move || cols.clone()).style(|s| {
                    s.flex_grow(1.0_f32)
                        .padding_horiz(8.0)
                        .padding_vert(4.0)
                        .font_size(12.0)
                        .font_family(theme::mono_family())
                        .color(theme::fg_dim())
                        .text_ellipsis()
                }),
                copy,
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

    let copy_ddl = label(move || {
        if state.db.is_view.get() {
            "⧉ Copy definition".to_string()
        } else {
            "⧉ Copy DDL".to_string()
        }
    })
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
    let new_migration = label(|| "+ Migration".to_string())
        .style(move |s| {
            // A migration that creates the table means nothing for a view.
            let s = if state.db.is_view.get() { s.hide() } else { s };
            s.font_size(11.0)
                .padding_horiz(8.0)
                .padding_vert(3.0)
                .margin_vert(6.0)
                .margin_left(6.0)
                .border_radius(4.0)
                .border(1.0)
                .border_color(theme::border())
                .color(theme::fg_dim())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::bg_hover()).color(theme::fg()))
        })
        .on_click_stop(move |_| state.db_new_migration_for_table());
    let ddl_row = stack((copy_ddl, new_migration)).style(|s| s.flex_row().items_center());

    stack((
        header, rows, idx_title, idx_rows, def_title, def_text, ddl_row,
    ))
    .style(|s| s.flex_col().width(520.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_staged_row_shows_its_values_nulls_and_defaults() {
        let cols = vec!["id".to_string(), "name".to_string(), "note".to_string()];
        let values = vec![
            ("name".to_string(), Some("ada".to_string())),
            ("note".to_string(), None),
        ];
        assert_eq!(
            new_row_cells(&cols, &values),
            vec![
                NewCell::Default,
                NewCell::Value("ada".into()),
                NewCell::Null
            ]
        );
    }

    #[test]
    fn the_confirmation_says_how_many_rows_it_touches() {
        use crate::state::RowEstimate::*;
        use e_db::Counted::*;
        assert_eq!(rows_summary(&[]), "");
        assert_eq!(rows_summary(&[Unknown]), "");
        assert_eq!(
            rows_summary(&[Known(Exactly(1)), Counting]),
            ", counting the rows it touches…"
        );
        assert_eq!(rows_summary(&[Known(Exactly(1))]), ", touching 1 row");
        assert_eq!(
            rows_summary(&[Known(Exactly(1200)), Known(Exactly(3))]),
            ", touching 1,203 rows"
        );
        assert_eq!(
            rows_summary(&[Known(Exactly(2)), Unknown]),
            ", touching at least 2 rows (some not counted)"
        );
        assert_eq!(
            rows_summary(&[Known(Over), Known(Exactly(5))]),
            ", touching over 10,005 rows"
        );
        assert_eq!(estimate_label(Known(Over)), "10,000+ rows");
        assert_eq!(estimate_label(Known(Exactly(0))), "0 rows");
        assert_eq!(group_digits(1234567), "1,234,567");
    }

    #[test]
    fn pretty_value_formats_json_objects_and_arrays() {
        assert_eq!(pretty_value("{\"a\":1}"), "{\n  \"a\": 1\n}");
        assert_eq!(pretty_value("[1,2]"), "[\n  1,\n  2\n]");
    }

    #[test]
    fn db_form_keeps_the_label_of_an_edited_connection() {
        // The label is one of the hints that a connection is production;
        // saving the edit form used to blank it.
        let form = crate::state::DbForm {
            label: "acme-production".into(),
            database: "app".into(),
            ..Default::default()
        };
        let cfg = form.to_config();
        assert_eq!(cfg.label, "acme-production");
        assert_eq!(cfg.environment(), e_db::Environment::Production);
        let unlabelled = crate::state::DbForm {
            label: String::new(),
            ..form
        };
        assert_eq!(
            unlabelled.to_config().environment(),
            e_db::Environment::Local
        );
    }

    #[test]
    fn db_form_keeps_an_environment_set_by_hand() {
        let form = crate::state::DbForm {
            host: "127.0.0.1".into(),
            env_label: "production".into(),
            ..Default::default()
        };
        let cfg = form.to_config();
        assert_eq!(cfg.environment(), e_db::Environment::Production);
        // A local host set to production starts read-only, like a guessed one.
        assert!(cfg.looks_like_prod());
    }

    #[test]
    fn pretty_value_leaves_scalars_and_invalid_json_untouched() {
        assert_eq!(pretty_value("hello"), "hello");
        assert_eq!(pretty_value("123"), "123");
        assert_eq!(pretty_value("{not json"), "{not json");
        assert_eq!(pretty_value(""), "");
    }
}
