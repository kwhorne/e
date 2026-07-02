//! Heuristic diff between Laravel migrations and the live database schema:
//! "column exists in the DB but no migration creates it", and vice versa.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;

/// One discrepancy between migrations and the database.
#[derive(Clone)]
pub struct DiffRow {
    pub table: String,
    pub column: String,
    pub in_db: bool,
    pub in_migration: bool,
}

/// Columns a `$table->…` call introduces, keyed on the method name.
fn columns_for_call(method: &str, first_arg: Option<&str>) -> Vec<String> {
    match method {
        "id" | "increments" | "bigIncrements" => vec!["id".into()],
        "timestamps" | "nullableTimestamps" | "timestampsTz" => {
            vec!["created_at".into(), "updated_at".into()]
        }
        "softDeletes" | "softDeletesTz" => vec!["deleted_at".into()],
        "rememberToken" => vec!["remember_token".into()],
        // Methods whose first string argument is the column name.
        _ => first_arg.map(|a| vec![a.to_string()]).unwrap_or_default(),
    }
}

/// Parse `database/migrations/*.php` into `table -> expected columns`.
pub fn parse_migrations(dir: &Path) -> HashMap<String, BTreeSet<String>> {
    let mut tables: HashMap<String, BTreeSet<String>> = HashMap::new();
    let Ok(read) = std::fs::read_dir(dir) else {
        return tables;
    };
    let mut files: Vec<_> = read
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("php"))
        .collect();
    files.sort(); // timestamp-prefixed → chronological

    for file in files {
        let Ok(src) = std::fs::read_to_string(&file) else {
            continue;
        };
        apply_migration(&src, &mut tables);
    }
    tables
}

fn apply_migration(src: &str, tables: &mut HashMap<String, BTreeSet<String>>) {
    // Walk each `Schema::<op>('table', ...)` block.
    let bytes = src.as_bytes();
    let mut i = 0;
    while let Some(rel) = src[i..].find("Schema::") {
        let start = i + rel;
        i = start + 8;
        let rest = &src[i..];
        let (op, after) = if let Some(a) = rest.strip_prefix("create(") {
            ("create", a)
        } else if let Some(a) = rest.strip_prefix("table(") {
            ("table", a)
        } else if let Some(a) = rest.strip_prefix("dropIfExists(") {
            ("drop", a)
        } else if let Some(a) = rest.strip_prefix("drop(") {
            ("drop", a)
        } else {
            continue;
        };
        let Some(table) = first_string(after) else {
            continue;
        };
        if op == "drop" {
            tables.remove(&table);
            continue;
        }
        // Body = up to the next `Schema::` or end of file.
        let body_start = i;
        let body_end = src[body_start..]
            .find("Schema::")
            .map(|r| body_start + r)
            .unwrap_or(bytes.len());
        let body = &src[body_start..body_end];
        let entry = tables.entry(table).or_default();
        parse_table_body(body, entry);
        i = body_end;
    }
}

fn parse_table_body(body: &str, cols: &mut BTreeSet<String>) {
    let mut i = 0;
    while let Some(rel) = body[i..].find("$table->") {
        let start = i + rel + "$table->".len();
        i = start;
        let rest = &body[start..];
        let method: String = rest.chars().take_while(|c| c.is_alphanumeric()).collect();
        if method.is_empty() {
            continue;
        }
        let after_method = &rest[method.len()..];
        let first = after_method.strip_prefix('(').and_then(first_string_or_arg);
        if method == "dropColumn" {
            for c in first_args(after_method) {
                cols.remove(&c);
            }
            continue;
        }
        for c in columns_for_call(&method, first.as_deref()) {
            cols.insert(c);
        }
    }
}

/// The first `'...'` / `"..."` argument inside `(...)`, if any.
fn first_string(after_paren: &str) -> Option<String> {
    let s = after_paren.trim_start();
    let q = s.chars().next()?;
    if q != '\'' && q != '"' {
        return None;
    }
    let inner = &s[1..];
    let end = inner.find(q)?;
    Some(inner[..end].to_string())
}

fn first_string_or_arg(after_paren: &str) -> Option<String> {
    first_string(after_paren)
}

/// All string args at the head of `(...)` — handles `dropColumn(['a','b'])`.
fn first_args(after_method: &str) -> Vec<String> {
    let mut out = Vec::new();
    let Some(open) = after_method.find('(') else {
        return out;
    };
    let seg = &after_method[open + 1..];
    let end = seg.find(')').unwrap_or(seg.len());
    let seg = &seg[..end];
    let mut chars = seg.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c == '\'' || c == '"' {
            chars.next();
            let mut s = String::new();
            for x in chars.by_ref() {
                if x == c {
                    break;
                }
                s.push(x);
            }
            if !s.is_empty() {
                out.push(s);
            }
        } else {
            chars.next();
        }
    }
    out
}

/// Framework tables not created by a user migration.
fn is_internal(table: &str) -> bool {
    matches!(table, "migrations" | "sqlite_sequence")
}

/// Compute discrepancies between expected (migrations) and actual (DB) columns.
pub fn diff(
    expected: &HashMap<String, BTreeSet<String>>,
    actual: &HashMap<String, HashSet<String>>,
) -> Vec<DiffRow> {
    let mut rows = Vec::new();
    let mut tables: BTreeSet<&String> = BTreeSet::new();
    tables.extend(expected.keys());
    tables.extend(actual.keys());

    for table in tables {
        if is_internal(table) {
            continue;
        }
        let exp = expected.get(table);
        let act = actual.get(table);
        let mut cols: BTreeSet<String> = BTreeSet::new();
        if let Some(e) = exp {
            cols.extend(e.iter().cloned());
        }
        if let Some(a) = act {
            cols.extend(a.iter().cloned());
        }
        for col in cols {
            let in_db = act.map(|a| a.contains(&col)).unwrap_or(false);
            let in_migration = exp.map(|e| e.contains(&col)).unwrap_or(false);
            if in_db != in_migration {
                rows.push(DiffRow {
                    table: table.clone(),
                    column: col,
                    in_db,
                    in_migration,
                });
            }
        }
    }
    rows
}

// ---- Panel view -----------------------------------------------------------

use crate::state::AppState;
use crate::theme;
use floem::peniko::Color;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use floem::views::{dyn_stack, label, scroll, stack, Decorators};
use floem::IntoView;

pub fn schema_diff_panel(state: AppState) -> impl IntoView {
    let title = label(|| "Schema Diff — migrations vs database".to_string()).style(|s| {
        s.flex_grow(1.0)
            .font_size(13.0)
            .font_bold()
            .color(theme::fg())
    });
    let refresh = label(|| "↻ Refresh".to_string())
        .style(|s| {
            s.padding_horiz(10.0)
                .padding_vert(3.0)
                .border_radius(4.0)
                .font_size(11.0)
                .color(theme::fg_dim())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::bg_hover()).color(theme::fg()))
        })
        .on_click_stop(move |_| state.compute_schema_diff());
    let close = label(|| "✕".to_string())
        .style(|s| {
            s.padding_horiz(8.0)
                .color(theme::fg_dim())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.color(theme::fg()))
        })
        .on_click_stop(move |_| state.schema_diff_open.set(false));
    let header = stack((title, refresh, close)).style(|s| {
        s.flex_row()
            .items_center()
            .gap(8.0)
            .padding_horiz(12.0)
            .padding_vert(8.0)
            .width_full()
            .border_bottom(1.0)
            .border_color(theme::border())
    });

    let rows = dyn_stack(
        move || {
            state
                .schema_diff
                .get()
                .into_iter()
                .enumerate()
                .collect::<Vec<_>>()
        },
        |(i, _)| *i,
        move |(_, row)| {
            let badge_text = match (row.in_db, row.in_migration) {
                (true, false) => "in DB, not in migrations",
                (false, true) => "in migration, not in DB (pending)",
                _ => "differs",
            };
            let color = if row.in_db {
                Color::from_rgb8(0xe5, 0xc0, 0x7b)
            } else {
                Color::from_rgb8(0x61, 0xaf, 0xef)
            };
            let name = format!("{}.{}", row.table, row.column);
            stack((
                label(move || name.clone()).style(|s| {
                    s.flex_grow(1.0)
                        .font_family("monospace".to_string())
                        .font_size(12.0)
                        .color(theme::fg())
                }),
                label(move || badge_text.to_string())
                    .style(move |s| s.font_size(11.0).color(color)),
            ))
            .style(|s| {
                s.flex_row()
                    .items_center()
                    .gap(10.0)
                    .width_full()
                    .padding_horiz(12.0)
                    .padding_vert(3.0)
            })
        },
    )
    .style(|s| s.flex_col().width_full());

    let empty_hint = label(|| "In sync — no differences.".to_string()).style(move |s| {
        let s = s.padding(16.0).color(theme::fg_dim()).font_size(12.0);
        if state.schema_diff.with(|d| d.is_empty()) {
            s
        } else {
            s.hide()
        }
    });

    let card = stack((
        header,
        empty_hint,
        scroll(rows).style(|s| s.flex_grow(1.0).width_full()),
    ))
    .style(|s| {
        s.flex_col()
            .width(720.0)
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
            .background(Color::from_rgba8(0, 0, 0, 0xCC));
        if state.schema_diff_open.get() {
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
    fn parses_and_diffs() {
        let src = r#"
            Schema::create('users', function (Blueprint $table) {
                $table->id();
                $table->string('name');
                $table->string('email')->unique();
                $table->timestamps();
            });
        "#;
        let mut tables = HashMap::new();
        apply_migration(src, &mut tables);
        let users = &tables["users"];
        assert!(users.contains("id"));
        assert!(users.contains("name"));
        assert!(users.contains("email"));
        assert!(users.contains("created_at"));

        let mut actual = HashMap::new();
        actual.insert(
            "users".to_string(),
            ["id", "name", "email", "created_at", "updated_at", "phone"]
                .iter()
                .map(|s| s.to_string())
                .collect::<HashSet<_>>(),
        );
        let rows = diff(&tables, &actual);
        // `phone` is in the DB but no migration creates it.
        assert!(rows
            .iter()
            .any(|r| r.column == "phone" && r.in_db && !r.in_migration));
    }
}
