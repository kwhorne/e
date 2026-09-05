//! The Migrations panel: what `php artisan migrate:status` knows, as a list you
//! can act on — run what's pending (snapshotting first when Grove can), roll
//! the last batch back, and open any migration's file.

use std::path::{Path, PathBuf};

use floem::peniko::Color;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use floem::views::{dyn_stack, label, scroll, stack, Decorators};
use floem::IntoView;

use crate::state::AppState;
use crate::theme;

const GREEN: Color = Color::from_rgb8(0x9e, 0xce, 0x6a);
const AMBER: Color = Color::from_rgb8(0xe5, 0xc0, 0x7b);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Migration {
    pub name: String,
    /// `Some(batch)` when it has run.
    pub batch: Option<u32>,
}

impl Migration {
    pub fn pending(&self) -> bool {
        self.batch.is_none()
    }
}

/// Parse `php artisan migrate:status` (Laravel 9+):
/// `  2024_05_01_000000_create_orders_table ..... [3] Ran` / `... Pending`.
pub fn parse_status(text: &str) -> Vec<Migration> {
    let mut out = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        // The name is the first token; it starts with a migration timestamp.
        let Some(name) = t.split_whitespace().next() else {
            continue;
        };
        if name.len() < 18 || !name.as_bytes()[..4].iter().all(u8::is_ascii_digit) {
            continue;
        }
        let rest = t[name.len()..].trim_start_matches(['.', ' ']);
        let batch = if rest.ends_with("Ran") {
            rest.trim_start_matches('[')
                .split(']')
                .next()
                .and_then(|b| b.trim().parse::<u32>().ok())
                .or(Some(0))
        } else if rest.ends_with("Pending") {
            None
        } else {
            continue;
        };
        out.push(Migration {
            name: name.to_string(),
            batch,
        });
    }
    out
}

/// Run `migrate:status` in `root`. Blocking (boots the app); off the UI thread.
pub fn status(root: &Path) -> Result<Vec<Migration>, String> {
    let out = std::process::Command::new("php")
        .args(["-d", "error_reporting=0", "-d", "display_errors=0"])
        .args(["artisan", "migrate:status", "--no-ansi"])
        .current_dir(root)
        .output()
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let first = text
            .lines()
            .chain(err.lines())
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("migrate:status failed");
        return Err(first.to_string());
    }
    Ok(parse_status(&text))
}

/// The migration file for a status entry, if it exists.
pub fn file_for(root: &Path, name: &str) -> Option<PathBuf> {
    let p = root.join("database/migrations").join(format!("{name}.php"));
    p.is_file().then_some(p)
}

impl AppState {
    pub fn toggle_migrations(&self) {
        let open = !self.migrations_open.get_untracked();
        self.migrations_open.set(open);
        if open {
            self.refresh_migrations();
        }
    }

    pub fn refresh_migrations(&self) {
        if self.migrations_loading.get_untracked() {
            return;
        }
        self.migrations_loading.set(true);
        let root = self.root.get_untracked();
        let list = self.migrations;
        let err = self.migrations_error;
        let loading = self.migrations_loading;
        self.spawn_bg(
            move || status(&root),
            move |res: Result<Vec<Migration>, String>| {
                loading.set(false);
                match res {
                    Ok(m) => {
                        err.set(String::new());
                        list.set(m);
                    }
                    Err(e) => err.set(e),
                }
            },
        );
    }

    /// Run a migration command in a terminal tab and re-read the status after
    /// it has had a moment to finish.
    fn run_migration_command(&self, label: &str, command: &str) {
        self.run_task(label, command);
        let app = *self;
        floem::action::exec_after(std::time::Duration::from_secs(6), move |_| {
            app.refresh_migrations();
        });
    }

    /// `php artisan migrate`, behind a `grove db snapshot` when Grove can take
    /// one for this database — so a bad migration is one restore from undone.
    pub fn migrations_run(&self) {
        let root = self.root.get_untracked();
        let command = match crate::tasks::grove_snapshot_engine(&root)
            .filter(|_| crate::grove::available())
        {
            Some(engine) => format!(
                "grove db snapshot --engine {engine} --note 'before migrate' && php artisan migrate"
            ),
            None => "php artisan migrate".to_string(),
        };
        self.run_migration_command("artisan: migrate", &command);
    }

    pub fn migrations_rollback(&self) {
        self.run_migration_command(
            "artisan: migrate:rollback",
            "php artisan migrate:rollback --step=1",
        );
    }

    pub fn open_migration(&self, name: &str) {
        if let Some(p) = file_for(&self.root.get_untracked(), name) {
            self.open_path(p);
        }
    }
}

fn pill(text: &'static str, color: Color, on_click: impl Fn() + 'static) -> impl IntoView {
    label(move || text.to_string())
        .style(move |s| {
            s.padding_horiz(10.0)
                .padding_vert(3.0)
                .border_radius(4.0)
                .font_size(11.0)
                .color(color)
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::bg_hover()))
        })
        .on_click_stop(move |_| on_click())
}

pub fn migrations_panel(state: AppState) -> impl IntoView {
    let title = label(|| "Migrations".to_string())
        .style(|s| s.font_size(13.0).font_bold().color(theme::fg()));
    let summary = label(move || {
        let (ran, pending) = state.migrations.with(|m| {
            let p = m.iter().filter(|x| x.pending()).count();
            (m.len() - p, p)
        });
        if state.migrations_loading.get() {
            "reading migrate:status…".to_string()
        } else if pending > 0 {
            format!("{ran} ran · {pending} pending")
        } else {
            format!("{ran} ran · up to date")
        }
    })
    .style(|s| {
        s.flex_grow(1.0_f32)
            .margin_left(10.0)
            .font_size(11.0)
            .color(theme::fg_dim())
    });
    let run = pill("▶ Migrate", GREEN, move || state.migrations_run());
    let rollback = pill("↶ Rollback last batch", AMBER, move || {
        state.migrations_rollback()
    });
    let refresh = pill("↻", theme::fg_dim(), move || state.refresh_migrations());
    let close = label(|| "✕".to_string())
        .style(|s| {
            s.padding_horiz(8.0)
                .color(theme::fg_dim())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.color(theme::fg()))
        })
        .on_click_stop(move |_| state.migrations_open.set(false));
    let header = stack((title, summary, run, rollback, refresh, close)).style(|s| {
        s.flex_row()
            .items_center()
            .gap(8.0)
            .padding_horiz(12.0)
            .padding_vert(8.0)
            .width_full()
            .border_bottom(1.0)
            .border_color(theme::border())
    });

    let error = label(move || state.migrations_error.get()).style(move |s| {
        let s = s.padding(14.0).color(AMBER).font_size(12.0);
        if state.migrations_error.with(|e| e.is_empty()) {
            s.hide()
        } else {
            s
        }
    });

    let rows = dyn_stack(
        move || state.migrations.get(),
        |m| m.name.clone(),
        move |m| {
            let name = m.name.clone();
            let open_name = m.name.clone();
            let status = match m.batch {
                Some(0) => "Ran".to_string(),
                Some(b) => format!("[{b}] Ran"),
                None => "Pending".to_string(),
            };
            let pending = m.pending();
            stack((
                label(move || name.clone()).style(|s| {
                    s.font_size(12.0)
                        .font_family("monospace".to_string())
                        .color(theme::fg())
                        .flex_grow(1.0_f32)
                        .min_width(0.0)
                        .text_ellipsis()
                }),
                label(move || status.clone()).style(move |s| {
                    s.font_size(11.0)
                        .color(if pending { AMBER } else { GREEN })
                        .flex_shrink(0.0_f32)
                }),
            ))
            .style(|s| {
                s.flex_row()
                    .items_center()
                    .gap(10.0)
                    .width_full()
                    .padding_horiz(12.0)
                    .padding_vert(4.0)
                    .cursor(floem::style::CursorStyle::Pointer)
                    .hover(|s| s.background(theme::bg_hover()))
            })
            .on_click_stop(move |_| state.open_migration(&open_name))
        },
    )
    .style(|s| s.flex_col().width_full());

    let card = stack((
        header,
        error,
        scroll(rows).style(|s| s.flex_grow(1.0_f32).width_full()),
    ))
    .style(|s| {
        s.flex_col()
            .width(820.0)
            .height(560.0)
            .border(1.0)
            .border_color(theme::border())
            .border_radius(10.0)
            .background(theme::bg())
    });

    floem::views::container(card).style(move |s| {
        let s = s
            .absolute()
            .inset(0.0)
            .size_full()
            .items_center()
            .justify_center()
            .background(Color::from_rgba8(0, 0, 0, 96));
        if state.migrations_open.get() {
            s
        } else {
            s.hide()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ran_and_pending_rows() {
        let text = "\n  Migration name ............................................. Batch / Status\n  2014_10_12_000000_create_users_table .......................... [1] Ran\n  2019_08_19_000000_create_failed_jobs_table .................... [1] Ran\n  2025_09_05_101010_create_bestillinger_table ................... Pending\n\n";
        let m = parse_status(text);
        assert_eq!(m.len(), 3);
        assert_eq!(m[0].name, "2014_10_12_000000_create_users_table");
        assert_eq!(m[0].batch, Some(1));
        assert!(m[2].pending());
        assert_eq!(m[2].name, "2025_09_05_101010_create_bestillinger_table");
        // Nothing to migrate: the header alone parses to nothing.
        assert!(parse_status("  Migration name .... Batch / Status\n").is_empty());
    }
}
