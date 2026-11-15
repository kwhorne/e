//! CSV import (DB-503): a small RFC 4180-ish parser (quoted fields, `""`
//! escapes, embedded delimiters and newlines), the delimiter sniffed from
//! the header, CSV columns mapped onto a table's columns (by name to start
//! with, then as the import dialog sets them), the values checked against
//! the columns' types, and the rows turned into `INSERT`s run in one
//! transaction.

/// Parse CSV `text` with `,` between fields. The first row is the header.
#[cfg(test)]
pub fn parse_csv(text: &str) -> Vec<Vec<String>> {
    parse_csv_with(text, ',')
}

/// Parse CSV `text` with `delim` between fields: quoted fields may hold
/// the delimiter, newlines and doubled quotes. A byte-order mark (Excel
/// writes one) is dropped, and so is a blank last line.
pub fn parse_csv_with(text: &str, delim: char) -> Vec<Vec<String>> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if in_quotes {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    in_quotes = false;
                }
            } else {
                field.push(c);
            }
        } else if c == '"' {
            in_quotes = true;
        } else if c == delim {
            row.push(std::mem::take(&mut field));
        } else if c == '\n' {
            row.push(std::mem::take(&mut field));
            rows.push(std::mem::take(&mut row));
        } else if c != '\r' {
            field.push(c);
        }
    }
    // The last field/row, if the file didn't end with a newline.
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows.retain(|r| !(r.len() == 1 && r[0].is_empty()));
    rows
}

/// The delimiter the header line uses: whichever of `,`, `;` and tab it has
/// most of outside quotes (`;` is what Excel writes where `,` is the
/// decimal mark), `,` when it has none.
pub fn sniff_delimiter(text: &str) -> char {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut counts = [(',', 0usize), (';', 0), ('\t', 0)];
    let mut in_quotes = false;
    for c in text.chars() {
        match c {
            '"' => in_quotes = !in_quotes,
            '\n' if !in_quotes => break,
            _ if !in_quotes => {
                if let Some(n) = counts.iter_mut().find(|(d, _)| *d == c) {
                    n.1 += 1;
                }
            }
            _ => {}
        }
    }
    counts
        .iter()
        .filter(|(_, n)| *n > 0)
        .max_by_key(|(_, n)| *n)
        .map_or(',', |(d, _)| *d)
}

/// A name with case, spaces, `_` and `-` set aside, so `First Name` finds
/// `first_name`.
fn loose(name: &str) -> String {
    name.chars()
        .filter(|c| !matches!(c, ' ' | '_' | '-'))
        .flat_map(char::to_lowercase)
        .collect()
}

/// The table column each CSV column goes into, found by name (`None`:
/// skipped). A table column is used once.
pub fn auto_map(header: &[String], columns: &[String]) -> Vec<Option<String>> {
    let mut used = std::collections::HashSet::new();
    header
        .iter()
        .map(|h| {
            let h = loose(h.trim());
            let col = columns
                .iter()
                .find(|c| loose(c) == h && !used.contains(*c))?;
            used.insert(col.clone());
            Some(col.clone())
        })
        .collect()
}

/// The values of one row that go in: `(column, value)` for each mapped CSV
/// column. An empty field is NULL, or, with `empty_default`, left out so
/// the column's default applies.
fn row_values(
    row: &[String],
    mapping: &[Option<String>],
    empty_default: bool,
) -> Vec<(String, Option<String>)> {
    mapping
        .iter()
        .enumerate()
        .filter_map(|(i, col)| {
            let col = col.clone()?;
            let v = row.get(i).filter(|v| !v.is_empty()).cloned();
            if v.is_none() && empty_default {
                return None;
            }
            Some((col, v))
        })
        .collect()
}

/// The `INSERT`s for `rows` (the data, without the header) into `table`.
pub fn build_inserts(
    engine: &str,
    table: &str,
    rows: &[Vec<String>],
    mapping: &[Option<String>],
    empty_default: bool,
) -> Result<Vec<String>, String> {
    if mapping.iter().all(Option::is_none) {
        return Err("No CSV column is mapped to a table column".into());
    }
    if rows.is_empty() {
        return Err("The CSV has a header but no rows".into());
    }
    let table_q = e_db::quote_ident(engine, table);
    Ok(rows
        .iter()
        .map(|r| {
            let values = row_values(r, mapping, empty_default);
            if !values.is_empty() {
                return e_db::insert_sql(engine, table, &values);
            }
            // Every field empty, and all left to their defaults.
            match engine {
                "postgres" | "postgresql" | "pgsql" | "sqlite" => {
                    format!("INSERT INTO {table_q} DEFAULT VALUES")
                }
                _ => format!("INSERT INTO {table_q} () VALUES ()"),
            }
        })
        .collect())
}

/// Something the import dialog points out before importing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    /// Importing can't go on until it's fixed.
    pub blocking: bool,
    pub text: String,
}

/// What a column's type lets the CSV check its values against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Whole,
    Number,
    Bool,
    Json,
    /// Anything else is left to the database.
    Other,
}

fn kind_of(data_type: &str) -> Kind {
    let mut t = data_type.trim().to_ascii_lowercase();
    // ClickHouse wraps types: Nullable(Int32), LowCardinality(String).
    for wrap in ["nullable(", "lowcardinality("] {
        while let Some(rest) = t.strip_prefix(wrap) {
            t = rest.strip_suffix(')').unwrap_or(rest).to_string();
        }
    }
    let word = t
        .split(|c: char| c == '(' || c.is_whitespace())
        .next()
        .unwrap_or("");
    match word {
        "int" | "integer" | "bigint" | "smallint" | "tinyint" | "mediumint" | "serial"
        | "bigserial" | "smallserial" | "int2" | "int4" | "int8" | "int16" | "int32" | "int64"
        | "int128" | "int256" | "uint8" | "uint16" | "uint32" | "uint64" | "uint128"
        | "uint256" => Kind::Whole,
        "decimal" | "numeric" | "float" | "double" | "real" | "float4" | "float8" | "float32"
        | "float64" | "decimal32" | "decimal64" | "decimal128" | "decimal256" => Kind::Number,
        "boolean" | "bool" => Kind::Bool,
        "json" | "jsonb" => Kind::Json,
        _ => Kind::Other,
    }
}

/// Whether `v` reads as a value of `kind`. An unsigned ClickHouse column
/// still takes a `-`: the database says so, not this.
fn fits(kind: Kind, v: &str) -> bool {
    let v = v.trim();
    match kind {
        Kind::Whole => {
            let digits = v.strip_prefix(['-', '+']).unwrap_or(v);
            !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
        }
        Kind::Number => v.parse::<f64>().is_ok_and(f64::is_finite),
        Kind::Bool => matches!(
            v.to_ascii_lowercase().as_str(),
            "true" | "false" | "t" | "f" | "1" | "0" | "yes" | "no" | "y" | "n" | "on" | "off"
        ),
        Kind::Json => serde_json::from_str::<serde_json::Value>(v).is_ok(),
        Kind::Other => true,
    }
}

fn kind_words(kind: Kind) -> &'static str {
    match kind {
        Kind::Whole => "whole numbers",
        Kind::Number => "numbers",
        Kind::Bool => "true/false",
        Kind::Json => "JSON",
        Kind::Other => "",
    }
}

/// What to point out about importing `rows` (the data; the header has
/// `width` fields) with `mapping` into `columns`: a table column mapped
/// twice (blocking), values that don't fit their column's type, empty
/// values headed for a NOT NULL column as NULL, and rows with another
/// number of fields. A few examples each, by line in the file.
pub fn problems(
    rows: &[Vec<String>],
    width: usize,
    mapping: &[Option<String>],
    columns: &[e_db::ColumnInfo],
    empty_default: bool,
) -> Vec<Problem> {
    const EXAMPLES: usize = 3;
    let mut out = Vec::new();
    // Line 1 is the header. (A quoted field with a newline in it puts a
    // row lower in the file than this says.)
    let line = |i: usize| i + 2;

    let mut seen = std::collections::HashSet::new();
    for col in mapping.iter().flatten() {
        if !seen.insert(col) {
            out.push(Problem {
                blocking: true,
                text: format!("{col} has two CSV columns mapped to it"),
            });
        }
    }

    for (ci, col) in mapping.iter().enumerate() {
        let Some(col) = col else { continue };
        let Some(info) = columns.iter().find(|c| &c.name == col) else {
            continue;
        };
        let kind = kind_of(&info.data_type);
        let mut bad = Vec::new();
        let mut empty = 0usize;
        for (ri, r) in rows.iter().enumerate() {
            match r.get(ci).map(String::as_str) {
                None | Some("") => empty += 1,
                Some(v) if !fits(kind, v) => bad.push((ri, v)),
                _ => {}
            }
        }
        if !bad.is_empty() {
            let examples = bad
                .iter()
                .take(EXAMPLES)
                .map(|(ri, v)| {
                    let v: String = v.chars().take(30).collect();
                    format!("line {} “{v}”", line(*ri))
                })
                .collect::<Vec<_>>()
                .join(", ");
            out.push(Problem {
                blocking: false,
                text: format!(
                    "{col} ({}) takes {}: {} value{} {} — {examples}",
                    info.data_type,
                    kind_words(kind),
                    bad.len(),
                    if bad.len() == 1 { "" } else { "s" },
                    if bad.len() == 1 { "isn't" } else { "aren't" },
                ),
            });
        }
        if empty > 0 && !empty_default && !info.nullable {
            out.push(Problem {
                blocking: false,
                text: format!(
                    "{col} is NOT NULL: {empty} empty value{} would go in as NULL",
                    if empty == 1 { "" } else { "s" },
                ),
            });
        }
    }

    let ragged: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, r)| r.len() != width)
        .map(|(i, _)| line(i))
        .collect();
    if !ragged.is_empty() {
        let examples = ragged
            .iter()
            .take(EXAMPLES)
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        out.push(Problem {
            blocking: false,
            text: format!(
                "{} row{} {} a different number of fields than the header (line {examples})",
                ragged.len(),
                if ragged.len() == 1 { "" } else { "s" },
                if ragged.len() == 1 { "has" } else { "have" },
            ),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(name: &str, ty: &str, nullable: bool) -> e_db::ColumnInfo {
        e_db::ColumnInfo {
            name: name.into(),
            data_type: ty.into(),
            nullable,
            key: String::new(),
        }
    }

    #[test]
    fn parse_quoted_and_embedded() {
        let csv = "id,name\n1,Alice\n2,\"Bob, Jr.\"\n3,\"line\nbreak\"\n4,\"say \"\"hi\"\"\"";
        let rows = parse_csv(csv);
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0], vec!["id", "name"]);
        assert_eq!(rows[2], vec!["2", "Bob, Jr."]);
        assert_eq!(rows[3], vec!["3", "line\nbreak"]);
        assert_eq!(rows[4], vec!["4", "say \"hi\""]);
    }

    #[test]
    fn excel_semicolons_and_a_bom() {
        let csv = "\u{feff}navn;beløp\r\n\"Ås; Ola\";12,50\r\n";
        assert_eq!(sniff_delimiter(csv), ';');
        let rows = parse_csv_with(csv, ';');
        assert_eq!(rows, vec![vec!["navn", "beløp"], vec!["Ås; Ola", "12,50"]]);
        assert_eq!(sniff_delimiter("a\tb\tc\n1,2\t3\t4"), '\t');
        assert_eq!(sniff_delimiter("\"a;b\",c\n"), ',');
        assert_eq!(sniff_delimiter("single\n"), ',');
    }

    #[test]
    fn columns_map_by_name_loosely_and_once() {
        let header: Vec<String> = ["First Name", "ID", "nope", "first-name"]
            .map(String::from)
            .to_vec();
        let cols: Vec<String> = ["id", "first_name"].map(String::from).to_vec();
        assert_eq!(
            auto_map(&header, &cols),
            vec![Some("first_name".into()), Some("id".into()), None, None]
        );
    }

    #[test]
    fn inserts_follow_the_mapping_and_empty_fields() {
        let rows = vec![
            vec!["Alice".to_string(), "".to_string(), "1".to_string()],
            vec!["".to_string(), "x".to_string(), "".to_string()],
        ];
        let mapping = vec![Some("name".to_string()), None, Some("id".to_string())];
        assert_eq!(
            build_inserts("sqlite", "users", &rows, &mapping, false).unwrap(),
            vec![
                "INSERT INTO `users` (`name`, `id`) VALUES ('Alice', '1')",
                "INSERT INTO `users` (`name`, `id`) VALUES (NULL, NULL)",
            ]
        );
        // Left to the defaults instead: an all-empty row inserts defaults.
        assert_eq!(
            build_inserts("sqlite", "users", &rows, &mapping, true).unwrap(),
            vec![
                "INSERT INTO `users` (`name`, `id`) VALUES ('Alice', '1')",
                "INSERT INTO `users` DEFAULT VALUES",
            ]
        );
        assert_eq!(
            build_inserts("mysql", "users", &rows[1..], &mapping, true).unwrap(),
            vec!["INSERT INTO `users` () VALUES ()"]
        );
        assert!(build_inserts("sqlite", "t", &rows, &[None, None, None], false).is_err());
        assert!(build_inserts("sqlite", "t", &[], &mapping, false).is_err());
    }

    #[test]
    fn values_are_checked_against_their_columns() {
        let cols = vec![
            col("id", "int(11)", false),
            col("price", "decimal(8,2)", true),
            col("active", "boolean", true),
            col("meta", "jsonb", true),
            col("email", "varchar(255)", false),
            col("n", "Nullable(UInt32)", true),
        ];
        let rows: Vec<Vec<String>> = [
            ["1", "9.50", "true", "{}", "a@x", "7"],
            ["x2", "1e3", "maybe", "{", "", "+1"],
            ["-3", "12,5", "0", "[1]", "b@x", "1.5"],
        ]
        .iter()
        .map(|r| r.iter().map(|s| s.to_string()).collect())
        .collect();
        let mapping: Vec<Option<String>> = cols.iter().map(|c| Some(c.name.clone())).collect();
        let found: Vec<String> = problems(&rows, 6, &mapping, &cols, false)
            .into_iter()
            .map(|p| p.text)
            .collect();
        assert_eq!(
            found,
            vec![
                "id (int(11)) takes whole numbers: 1 value isn't — line 3 “x2”",
                "price (decimal(8,2)) takes numbers: 1 value isn't — line 4 “12,5”",
                "active (boolean) takes true/false: 1 value isn't — line 3 “maybe”",
                "meta (jsonb) takes JSON: 1 value isn't — line 3 “{”",
                "email is NOT NULL: 1 empty value would go in as NULL",
                "n (Nullable(UInt32)) takes whole numbers: 1 value isn't — line 4 “1.5”",
            ]
        );
        // Left to the default, an empty value isn't NULL.
        assert!(!problems(&rows, 6, &mapping, &cols, true)
            .iter()
            .any(|p| p.text.contains("NOT NULL")));
    }

    #[test]
    fn a_column_mapped_twice_blocks_and_ragged_rows_are_counted() {
        let cols = vec![col("name", "text", true)];
        let rows = vec![
            vec!["a".to_string(), "b".to_string()],
            vec!["c".to_string()],
        ];
        let mapping = vec![Some("name".to_string()), Some("name".to_string())];
        let found = problems(&rows, 2, &mapping, &cols, false);
        assert_eq!(
            found,
            vec![
                Problem {
                    blocking: true,
                    text: "name has two CSV columns mapped to it".into(),
                },
                Problem {
                    blocking: false,
                    text: "1 row has a different number of fields than the header (line 3)".into(),
                },
            ]
        );
    }
}
