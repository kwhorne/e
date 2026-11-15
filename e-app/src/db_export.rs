//! Pure formatters that turn a query result grid into export text: CSV, TSV,
//! JSON, a Markdown table, or `INSERT` statements. Used by the export buttons
//! and copy-to-clipboard.

use e_db::QueryResult;

/// The available export formats.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Csv,
    Tsv,
    Json,
    Markdown,
    SqlInserts,
}

impl Format {
    /// Every format, in the order the menus list them.
    pub const ALL: [Format; 5] = [
        Format::Csv,
        Format::Tsv,
        Format::Json,
        Format::Markdown,
        Format::SqlInserts,
    ];

    /// What the menus call it.
    pub fn name(self) -> &'static str {
        match self {
            Format::Csv => "CSV",
            Format::Tsv => "TSV",
            Format::Json => "JSON",
            Format::Markdown => "Markdown table",
            Format::SqlInserts => "SQL INSERT statements",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Format::Csv => "csv",
            Format::Tsv => "tsv",
            Format::Json => "json",
            Format::Markdown => "md",
            Format::SqlInserts => "sql",
        }
    }
}

/// Format a result in `fmt`. `table` names the table for `INSERT` output,
/// quoted and escaped for `engine`.
pub fn format(result: &QueryResult, fmt: Format, table: &str, engine: &str) -> String {
    match fmt {
        Format::Csv => delimited(result, ','),
        Format::Tsv => delimited(result, '\t'),
        Format::Json => json(result),
        Format::Markdown => markdown(result),
        Format::SqlInserts => sql_inserts(result, table, engine),
    }
}

fn csv_escape(s: &str, delim: char) -> String {
    if s.contains([delim, '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn delimited(result: &QueryResult, delim: char) -> String {
    let d = delim.to_string();
    let mut out = String::new();
    let header: Vec<String> = result
        .columns
        .iter()
        .map(|c| csv_escape(c, delim))
        .collect();
    out.push_str(&header.join(&d));
    out.push('\n');
    for row in &result.rows {
        let cells: Vec<String> = row
            .iter()
            .map(|c| csv_escape(c.as_deref().unwrap_or(""), delim))
            .collect();
        out.push_str(&cells.join(&d));
        out.push('\n');
    }
    out
}

/// An array of one object per row, written by hand so the keys keep the
/// columns' order (a `serde_json::Map` sorts them) and a name two columns
/// share (`id` from a join) stays twice.
fn json(result: &QueryResult) -> String {
    let quote = |s: &str| serde_json::to_string(s).unwrap_or_default();
    if result.rows.is_empty() {
        return "[]".into();
    }
    let rows: Vec<String> = result
        .rows
        .iter()
        .map(|row| {
            let fields: Vec<String> = result
                .columns
                .iter()
                .enumerate()
                .map(|(i, col)| {
                    let v = match row.get(i).and_then(|c| c.as_deref()) {
                        Some(s) => quote(s),
                        None => "null".into(),
                    };
                    format!("    {}: {v}", quote(col))
                })
                .collect();
            format!("  {{\n{}\n  }}", fields.join(",\n"))
        })
        .collect();
    format!("[\n{}\n]", rows.join(",\n"))
}

fn markdown(result: &QueryResult) -> String {
    let mut out = String::new();
    out.push_str("| ");
    out.push_str(&result.columns.join(" | "));
    out.push_str(" |\n| ");
    out.push_str(
        &result
            .columns
            .iter()
            .map(|_| "---")
            .collect::<Vec<_>>()
            .join(" | "),
    );
    out.push_str(" |\n");
    for row in &result.rows {
        out.push_str("| ");
        let cells: Vec<String> = row
            .iter()
            .map(|c| {
                c.as_deref()
                    .unwrap_or("")
                    .replace('|', "\\|")
                    .replace('\n', " ")
            })
            .collect();
        out.push_str(&cells.join(" | "));
        out.push_str(" |\n");
    }
    out
}

/// One `INSERT` per row, names and values quoted the way `engine` reads
/// them (a backslash in a MySQL value would otherwise end the string).
fn sql_inserts(result: &QueryResult, table: &str, engine: &str) -> String {
    let mut out = String::new();
    for row in &result.rows {
        let values: Vec<(String, Option<String>)> = result
            .columns
            .iter()
            .cloned()
            .zip(row.iter().cloned())
            .collect();
        out.push_str(&e_db::insert_sql(engine, table, &values));
        out.push_str(";\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> QueryResult {
        QueryResult {
            columns: vec!["id".into(), "name".into()],
            rows: vec![
                vec![Some("1".into()), Some("Alice".into())],
                vec![Some("2".into()), None],
            ],
            ..Default::default()
        }
    }

    #[test]
    fn csv_and_tsv() {
        assert_eq!(
            format(&sample(), Format::Csv, "t", "sqlite"),
            "id,name\n1,Alice\n2,\n"
        );
        assert_eq!(
            format(&sample(), Format::Tsv, "t", "sqlite"),
            "id\tname\n1\tAlice\n2\t\n"
        );
    }

    #[test]
    fn json_keeps_the_columns_order() {
        let r = QueryResult {
            columns: vec!["z".into(), "a".into(), "id".into(), "id".into()],
            rows: vec![vec![
                Some("1".into()),
                None,
                Some("say \"hi\"".into()),
                Some("2".into()),
            ]],
            ..Default::default()
        };
        assert_eq!(
            format(&r, Format::Json, "t", "sqlite"),
            "[\n  {\n    \"z\": \"1\",\n    \"a\": null,\n    \"id\": \"say \\\"hi\\\"\",\n    \"id\": \"2\"\n  }\n]"
        );
        let empty = QueryResult {
            columns: vec!["id".into()],
            ..Default::default()
        };
        assert_eq!(format(&empty, Format::Json, "t", "sqlite"), "[]");
    }

    #[test]
    fn markdown_table() {
        let m = format(&sample(), Format::Markdown, "t", "sqlite");
        assert!(m.starts_with("| id | name |\n| --- | --- |\n"));
        assert!(m.contains("| 1 | Alice |"));
    }

    #[test]
    fn sql_inserts_are_quoted_for_the_engine() {
        let mut r = sample();
        r.rows[0][1] = Some("O'Brien \\".into());
        let s = format(&r, Format::SqlInserts, "users", "mysql");
        assert!(s.contains("INSERT INTO `users` (`id`, `name`) VALUES ('1', 'O''Brien \\\\');"));
        assert!(s.contains("VALUES ('2', NULL);"));
        let pg = format(&sample(), Format::SqlInserts, "users", "postgres");
        assert!(pg.starts_with("INSERT INTO \"users\" (\"id\", \"name\")"));
    }
}
