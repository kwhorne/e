//! CSV import: a small RFC 4180-ish parser (quoted fields, `""` escapes,
//! embedded commas/newlines). The rows are mapped onto a table's columns by
//! header name and turned into `INSERT` statements run in one transaction.

/// Parse CSV `text` into rows of fields. The first row is the header. Handles
/// quoted fields containing commas, newlines and doubled quotes.
pub fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = text.chars().peekable();
    let mut any = false;
    while let Some(c) = chars.next() {
        any = true;
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
        } else {
            match c {
                '"' => in_quotes = true,
                ',' => {
                    row.push(std::mem::take(&mut field));
                }
                '\r' => {}
                '\n' => {
                    row.push(std::mem::take(&mut field));
                    rows.push(std::mem::take(&mut row));
                }
                _ => field.push(c),
            }
        }
    }
    // Flush the last field/row if the file didn't end with a newline.
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    let _ = any;
    // Drop a trailing empty row (from a final newline).
    rows.retain(|r| !(r.len() == 1 && r[0].is_empty()));
    rows
}

/// Build `INSERT` statements for `table` from parsed CSV rows, mapping fields to
/// columns by header name (only headers that exist in `table_columns` are used).
/// Returns the statements, or an error describing why none could be built.
pub fn build_inserts(
    engine: &str,
    table: &str,
    csv_rows: &[Vec<String>],
    table_columns: &[String],
) -> Result<Vec<String>, String> {
    let Some(header) = csv_rows.first() else {
        return Err("CSV is empty".into());
    };
    // Map each CSV column index to a real table column, by name.
    let mapping: Vec<(usize, String)> = header
        .iter()
        .enumerate()
        .filter_map(|(i, h)| {
            let h = h.trim();
            table_columns
                .iter()
                .find(|c| c.eq_ignore_ascii_case(h))
                .map(|c| (i, c.clone()))
        })
        .collect();
    if mapping.is_empty() {
        return Err("No CSV headers match the table's columns".into());
    }
    let mut stmts = Vec::new();
    for r in &csv_rows[1..] {
        let values: Vec<(String, Option<String>)> = mapping
            .iter()
            .map(|(i, col)| {
                let v = r.get(*i).map(|s| s.to_string());
                // Treat an empty field as NULL.
                let v = v.filter(|s| !s.is_empty());
                (col.clone(), v)
            })
            .collect();
        stmts.push(e_db::insert_sql(engine, table, &values));
    }
    if stmts.is_empty() {
        return Err("CSV has a header but no data rows".into());
    }
    Ok(stmts)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn build_inserts_maps_by_header_and_nulls_blanks() {
        let rows = parse_csv("name,missing,id\nAlice,,1\n,x,2");
        let cols = vec!["id".to_string(), "name".to_string()];
        let stmts = build_inserts("sqlite", "users", &rows, &cols).unwrap();
        assert_eq!(stmts.len(), 2);
        // Only matching headers (name, id) are used; blank -> NULL.
        assert_eq!(
            stmts[0],
            "INSERT INTO `users` (`name`, `id`) VALUES ('Alice', '1')"
        );
        assert_eq!(
            stmts[1],
            "INSERT INTO `users` (`name`, `id`) VALUES (NULL, '2')"
        );
    }

    #[test]
    fn build_inserts_errors_when_no_headers_match() {
        let rows = parse_csv("foo,bar\n1,2");
        let cols = vec!["id".to_string()];
        assert!(build_inserts("sqlite", "t", &rows, &cols).is_err());
    }
}
