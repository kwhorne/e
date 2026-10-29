//! The Migrations panel: what `php artisan migrate:status` knows, as a list you
//! can act on — run what's pending (snapshotting first when Grove can), roll
//! the last batch back, and open any migration's file.
//!
//! Also what a pending migration will do before it runs: opened or saved, its
//! SQL (from `migrate --pretend`) shows beside the lines that produce it, and
//! what would go wrong against the data already there shows as diagnostics
//! (see [`e_laravel::migration_check`]). ⌘. offers a dry run of `up()` and
//! `down()` in a transaction that is rolled back, on Postgres and SQLite.

use std::path::{Path, PathBuf};

use floem::peniko::Color;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use floem::views::editor::text::Document;
use floem::views::{dyn_stack, label, scroll, stack, Decorators};
use floem::IntoView;

use crate::state::AppState;
use crate::theme;
fn green() -> Color {
    theme::success()
}
fn amber() -> Color {
    theme::warning()
}

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
    let out = crate::phptools::php(
        root,
        &["artisan", "migrate:status", "--no-ansi"],
        std::time::Duration::from_secs(30),
    )?;
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

/// Whether `path` is one of the project's migrations.
pub fn is_migration(root: &Path, path: &Path) -> bool {
    path.starts_with(root.join("database/migrations"))
        && path.extension().is_some_and(|e| e == "php")
}

/// SQL beside a line is cut to this many characters.
const ANNOTATION_MAX: usize = 140;

fn annotation(sql: &str) -> String {
    let mut s: String = sql.chars().take(ANNOTATION_MAX).collect();
    if s.len() < sql.len() {
        s.push('…');
    }
    format!("⇂ {s}")
}

/// A diagnostic over the code on `line` of `src`.
fn diagnostic_on(
    src: &str,
    line: usize,
    severity: lsp_types::DiagnosticSeverity,
    message: String,
) -> lsp_types::Diagnostic {
    let text = src.lines().nth(line).unwrap_or("");
    let indent = text.len() - text.trim_start().len();
    let at = |character: usize| lsp_types::Position {
        line: line as u32,
        character: character as u32,
    };
    lsp_types::Diagnostic {
        range: lsp_types::Range {
            start: at(indent),
            end: at(text.encode_utf16().count()),
        },
        severity: Some(severity),
        source: Some("migration".to_string()),
        message,
        ..Default::default()
    }
}

/// The project's database, if it can be asked without risk: local, reachable.
pub(crate) fn local_database(root: &Path) -> Option<(e_db::Conn, e_db::DbConfig)> {
    let cfg = e_db::from_env(root)?;
    if cfg.looks_like_prod() {
        return None;
    }
    let conn = e_db::connect(&cfg).ok()?;
    Some((conn, cfg))
}

impl AppState {
    /// Check a pending migration's buffer: place the SQL it will run beside
    /// its lines, and mark what would go wrong against the data. Reads the
    /// file on disk, so only for a buffer without unsaved edits — on open and
    /// on save. A migration that has already run is left alone.
    pub fn check_migration(&self, buffer_id: u64) {
        let Some(buf) = self.buffer_by_id(buffer_id) else {
            return;
        };
        let root = self.root.get_untracked();
        let Some(path) = buf.file.path.clone().filter(|p| is_migration(&root, p)) else {
            return;
        };
        if buf.dirty.get_untracked() || !root.join("artisan").is_file() {
            return;
        }
        let app = *self;
        self.spawn_bg(
            move || -> Result<(String, e_laravel::migration_check::Checked), String> {
                use e_laravel::migration_check as mc;
                let src = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
                let sql = mc::pretend(&root, &path)?;
                if sql.is_empty() {
                    return Ok((src, mc::Checked::default()));
                }
                let db = local_database(&root);
                let checked = match &db {
                    Some((conn, cfg)) => {
                        let facts = mc::DbFacts {
                            conn,
                            engine: &cfg.engine,
                        };
                        let engine = mc::Engine::from_name(&cfg.engine);
                        mc::check(&src, &sql, engine.map(|e| (&facts as &dyn mc::Facts, e)))
                    }
                    None => mc::check(&src, &sql, None),
                };
                Ok((src, checked))
            },
            move |res| {
                let Some(buf) = app.buffer_by_id(buffer_id) else {
                    return;
                };
                let (src, checked) = match res {
                    Ok(r) => r,
                    Err(e) => {
                        eprintln!("e: migrate --pretend: {e}");
                        (String::new(), Default::default())
                    }
                };
                use e_laravel::migration_check::Severity;
                let diags: Vec<lsp_types::Diagnostic> = checked
                    .risks
                    .iter()
                    .map(|(line, r)| {
                        let severity = match r.severity {
                            Severity::Error => lsp_types::DiagnosticSeverity::ERROR,
                            Severity::Warning => lsp_types::DiagnosticSeverity::WARNING,
                            Severity::Info => lsp_types::DiagnosticSeverity::INFORMATION,
                        };
                        diagnostic_on(&src, *line, severity, r.message.clone())
                    })
                    .collect();
                let notes: Vec<(usize, String)> = checked
                    .statements
                    .iter()
                    .map(|(line, sql)| (*line, annotation(sql)))
                    .collect();
                app.show_migration_check(&buf, diags, notes);
            },
        );
    }

    fn show_migration_check(
        &self,
        buf: &crate::state::Buffer,
        diags: Vec<lsp_types::Diagnostic>,
        notes: Vec<(usize, String)>,
    ) {
        let changed = *buf.migration_risks.borrow() != diags;
        if changed {
            *buf.migration_risks.borrow_mut() = diags;
            if let Some(uri) = buf.uri.clone() {
                let lsp = self
                    .diagnostics
                    .with_untracked(|m| m.get(&uri).cloned().unwrap_or_default());
                self.apply_diagnostics_to_buffer(&uri, &lsp);
            }
        }
        if buf.annotations.get_untracked() != notes {
            buf.annotations.set(notes);
            buf.doc.cache_rev().update(|r| *r += 1);
        }
    }

    /// Whether ⌘. should offer a dry run for this file: a migration, on a
    /// database whose schema changes can be rolled back.
    pub fn migration_dry_run_offered(&self, path: &Path) -> bool {
        let root = self.root.get_untracked();
        is_migration(&root, path)
            && e_db::from_env(&root).is_some_and(|c| {
                matches!(
                    e_laravel::migration_check::Engine::from_name(&c.engine),
                    Some(
                        e_laravel::migration_check::Engine::Postgres
                            | e_laravel::migration_check::Engine::Sqlite
                    )
                )
            })
    }

    /// Run the active migration's `up()` and `down()` in a transaction that
    /// is rolled back, and say how it went: a failure lands on the line of the
    /// method that threw.
    pub fn migration_dry_run(&self) {
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let root = self.root.get_untracked();
        let Some(path) = buf.file.path.clone().filter(|p| is_migration(&root, p)) else {
            return;
        };
        if buf.dirty.get_untracked() {
            Self::notify("Save the migration first: the dry run runs the file on disk");
            return;
        }
        let app = *self;
        let id = buf.id;
        let file = path.clone();
        self.spawn_bg(
            move || e_laravel::migration_check::dry_run(&root, &file),
            move |res| {
                use e_laravel::migration_check::DryRun;
                let Some(buf) = app.buffer_by_id(id) else {
                    return;
                };
                match res {
                    Ok(DryRun::Passed { steps, .. }) => Self::notify(&format!(
                        "Dry run passed: {} ran, and the transaction was rolled back",
                        steps
                            .iter()
                            .map(|s| format!("{s}()"))
                            .collect::<Vec<_>>()
                            .join(" then ")
                    )),
                    Ok(DryRun::Failed { step, error, .. }) => {
                        let src = buf.doc.text().to_string();
                        let line = src
                            .lines()
                            .position(|l| l.contains(&format!("function {step}")))
                            .unwrap_or(0);
                        let mut diags = buf.migration_risks.borrow().clone();
                        diags.push(diagnostic_on(
                            &src,
                            line,
                            lsp_types::DiagnosticSeverity::ERROR,
                            format!("{step}() fails (dry run, rolled back): {error}"),
                        ));
                        let notes = buf.annotations.get_untracked();
                        app.show_migration_check(&buf, diags, notes);
                        Self::notify(&format!("Dry run: {step}() fails — {error}"));
                    }
                    Ok(DryRun::NotTransactional { driver }) => Self::notify(&format!(
                        "No dry run on {driver}: it commits schema changes as it makes them, \
                         so a rollback couldn't undo them"
                    )),
                    Err(e) => Self::notify(&format!("Dry run couldn't start: {e}")),
                }
            },
        );
    }

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
    let run = pill("▶ Migrate", green(), move || state.migrations_run());
    let rollback = pill("↶ Rollback last batch", amber(), move || {
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
        let s = s.padding(14.0).color(amber()).font_size(12.0);
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
                        .font_family(theme::mono_family())
                        .color(theme::fg())
                        .flex_grow(1.0_f32)
                        .min_width(0.0)
                        .text_ellipsis()
                }),
                label(move || status.clone()).style(move |s| {
                    s.font_size(11.0)
                        .color(if pending { amber() } else { green() })
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
