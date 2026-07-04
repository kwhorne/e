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

/// Format a result in `fmt`. `table` names the table for `INSERT` output.
pub fn format(result: &QueryResult, fmt: Format, table: &str) -> String {
    match fmt {
        Format::Csv => delimited(result, ','),
        Format::Tsv => delimited(result, '\t'),
        Format::Json => json(result),
        Format::Markdown => markdown(result),
        Format::SqlInserts => sql_inserts(result, table),
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

fn json(result: &QueryResult) -> String {
    let arr: Vec<serde_json::Value> = result
        .rows
        .iter()
        .map(|row| {
            let mut obj = serde_json::Map::new();
            for (i, col) in result.columns.iter().enumerate() {
                let v = match row.get(i).and_then(|c| c.as_ref()) {
                    Some(s) => serde_json::Value::String(s.clone()),
                    None => serde_json::Value::Null,
                };
                obj.insert(col.clone(), v);
            }
            serde_json::Value::Object(obj)
        })
        .collect();
    serde_json::to_string_pretty(&serde_json::Value::Array(arr)).unwrap_or_default()
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

fn sql_inserts(result: &QueryResult, table: &str) -> String {
    let cols = result.columns.join(", ");
    let mut out = String::new();
    for row in &result.rows {
        let vals: Vec<String> = row
            .iter()
            .map(|c| match c {
                Some(v) => format!("'{}'", v.replace('\'', "''")),
                None => "NULL".to_string(),
            })
            .collect();
        out.push_str(&format!(
            "INSERT INTO {table} ({cols}) VALUES ({});\n",
            vals.join(", ")
        ));
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
            format(&sample(), Format::Csv, "t"),
            "id,name\n1,Alice\n2,\n"
        );
        assert_eq!(
            format(&sample(), Format::Tsv, "t"),
            "id\tname\n1\tAlice\n2\t\n"
        );
    }

    #[test]
    fn json_nulls_and_objects() {
        let j = format(&sample(), Format::Json, "t");
        assert!(j.contains("\"name\": \"Alice\""));
        assert!(j.contains("\"name\": null"));
    }

    #[test]
    fn markdown_table() {
        let m = format(&sample(), Format::Markdown, "t");
        assert!(m.starts_with("| id | name |\n| --- | --- |\n"));
        assert!(m.contains("| 1 | Alice |"));
    }

    #[test]
    fn sql_inserts_escape_and_null() {
        let mut r = sample();
        r.rows[0][1] = Some("O'Brien".into());
        let s = format(&r, Format::SqlInserts, "users");
        assert!(s.contains("INSERT INTO users (id, name) VALUES ('1', 'O''Brien');"));
        assert!(s.contains("VALUES ('2', NULL);"));
    }
}
