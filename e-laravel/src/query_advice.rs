//! What an Eloquent query costs, and the index that would help: the
//! database's own plan for the SQL a chain runs (`EXPLAIN`), and — where the
//! plan scans a whole table — the composite index its `where` and `order by`
//! ask for, as a migration.
//!
//! The index follows the usual rule: the columns compared with `=` (or
//! `in`, `is null`) first, then the one compared by range — or, without
//! one, the columns it sorts by.

use e_db::QueryResult;

/// One query's plan, as the app's own connection gave it.
#[derive(Clone, Debug, PartialEq)]
pub struct Explained {
    pub sql: String,
    /// `mysql`, `postgres` or `sqlite`.
    pub engine: String,
    /// The plan's columns and rows.
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
    /// Why there is no plan.
    pub error: Option<String>,
    /// Indexes the tables the query reads already have: `(table, columns)`.
    pub indexes: Vec<(String, Vec<String>)>,
}

/// The index to add.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Advice {
    pub table: String,
    pub columns: Vec<String>,
}

impl Advice {
    /// `orders (status, created_at)`.
    pub fn describe(&self) -> String {
        format!("{} ({})", self.table, self.columns.join(", "))
    }

    /// The migration's file name, after `timestamp`.
    pub fn file_name(&self, timestamp: &str) -> String {
        format!(
            "{timestamp}_add_{}_index_to_{}_table.php",
            self.columns.join("_"),
            self.table
        )
    }

    /// The migration that adds the index, and drops it again.
    pub fn migration(&self) -> String {
        let cols = self
            .columns
            .iter()
            .map(|c| format!("'{c}'"))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "<?php\n\nuse Illuminate\\Database\\Migrations\\Migration;\nuse Illuminate\\Database\\Schema\\Blueprint;\nuse Illuminate\\Support\\Facades\\Schema;\n\nreturn new class extends Migration\n{{\n    public function up(): void\n    {{\n        Schema::table('{table}', function (Blueprint $table) {{\n            $table->index([{cols}]);\n        }});\n    }}\n\n    public function down(): void\n    {{\n        Schema::table('{table}', function (Blueprint $table) {{\n            $table->dropIndex([{cols}]);\n        }});\n    }}\n}};\n",
            table = self.table,
        )
    }
}

/// What the plans say: the problems, and the index for the first one an
/// index would fix.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Verdict {
    pub issues: Vec<String>,
    pub advice: Option<Advice>,
}

/// Judge `explained`, every query a chain ran.
pub fn verdict(explained: &[Explained]) -> Verdict {
    let mut out = Verdict::default();
    for e in explained {
        let plan = QueryResult {
            columns: e.columns.clone(),
            rows: e.rows.clone(),
            rows_affected: None,
            elapsed_ms: 0,
            is_select: true,
            truncated: false,
        };
        let issues = e_db::analyze_explain(&e.engine, &plan);
        if issues.is_empty() {
            continue;
        }
        if out.advice.is_none() {
            out.advice = index_for(&e.sql).filter(|a| {
                // Not when an index already starts with these columns.
                !e.indexes.iter().any(|(table, cols)| {
                    table == &a.table
                        && cols.len() >= a.columns.len()
                        && cols[..a.columns.len()] == a.columns[..]
                })
            });
        }
        out.issues.extend(issues);
    }
    out
}

/// The plans in full, for the Tinker panel.
pub fn report(explained: &[Explained], verdict: &Verdict) -> String {
    let mut out = String::from("\nEXPLAIN\n");
    if explained.is_empty() {
        out.push_str("  It ran no select to explain.\n");
    }
    for e in explained {
        out.push_str(&format!("\n  {}\n", e.sql));
        if let Some(err) = &e.error {
            out.push_str(&format!("    couldn't explain it: {err}\n"));
            continue;
        }
        out.push_str(&format!("    {}\n", e.columns.join(" | ")));
        for row in &e.rows {
            let cells: Vec<&str> = row.iter().map(|c| c.as_deref().unwrap_or("NULL")).collect();
            out.push_str(&format!("    {}\n", cells.join(" | ")));
        }
    }
    for issue in &verdict.issues {
        out.push_str(&format!("\n  ✖ {issue}"));
    }
    if let Some(a) = &verdict.advice {
        out.push_str(&format!(
            "\n  → an index on {} would cover it (⌘. on the line adds the migration)\n",
            a.describe()
        ));
    }
    out
}

/// What the end of the line says.
pub fn label(verdict: &Verdict, queries: usize) -> String {
    let what = match queries {
        1 => "1 query".to_string(),
        n => format!("{n} queries"),
    };
    match (&verdict.advice, verdict.issues.first()) {
        (_, None) => format!("EXPLAIN ✓ {what}, every one on an index"),
        (Some(a), Some(issue)) => format!(
            "EXPLAIN ✖ {issue} — an index on {} would cover it",
            a.describe()
        ),
        (None, Some(issue)) => format!("EXPLAIN ✖ {issue}"),
    }
}

/// An identifier without its quotes: `"orders"`, `` `orders` ``, `[orders]`.
fn unquote(s: &str) -> &str {
    s.trim_matches(|c| c == '"' || c == '`' || c == '[' || c == ']')
}

/// `sql` lowercased outside its string literals, which become `''`, so
/// keywords inside a value don't count.
fn shape(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len());
    let mut chars = sql.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\'' {
            out.push_str("''");
            while let Some(d) = chars.next() {
                if d == '\'' {
                    if chars.peek() == Some(&'\'') {
                        chars.next();
                        continue;
                    }
                    break;
                }
            }
        } else {
            out.extend(c.to_lowercase());
        }
    }
    out
}

/// Split `text` at `sep` where it is outside parentheses.
fn split_top<'a>(text: &'a str, sep: &str) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    let mut i = 0;
    let b = text.as_bytes();
    while i < b.len() {
        match b[i] {
            b'(' => depth += 1,
            b')' => depth -= 1,
            _ if depth == 0 && text[i..].starts_with(sep) => {
                out.push(&text[start..i]);
                i += sep.len();
                start = i;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    out.push(&text[start..]);
    out
}

/// The column a `where` term or `order by` item names, if it is one of
/// `table`'s: `"orders"."status"` or plain `"status"`.
fn column_of(term: &str, table: &str) -> Option<String> {
    let first = term.split_whitespace().next()?;
    let ident: String = first
        .chars()
        .take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '"' | '`' | '[' | ']'))
        .collect();
    let parts: Vec<&str> = ident.split('.').map(unquote).collect();
    let column = match parts.as_slice() {
        [col] => *col,
        [t, col] if *t == table => *col,
        _ => return None,
    };
    (!column.is_empty()
        && column.chars().all(|c| c.is_alphanumeric() || c == '_')
        && !column.chars().next()?.is_ascii_digit())
    .then(|| column.to_string())
}

/// The index `sql`'s `where` and `order by` ask for on the table it reads
/// from; `None` for a query no single index would serve — an `or` at the
/// top, nothing to filter or sort by.
pub fn index_for(sql: &str) -> Option<Advice> {
    let s = shape(sql);
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let from = s.find(" from ")? + 6;
    let table_token = s[from..].split([' ', ',']).next()?;
    let table = unquote(table_token.rsplit('.').next()?).to_string();
    if table.is_empty() || table.starts_with('(') {
        return None;
    }
    let rest = &s[from..];
    let ends = [
        " group by ",
        " order by ",
        " limit ",
        " offset ",
        " having ",
        " for update",
    ];
    let clause_end = |from: usize| {
        ends.iter()
            .filter_map(|e| rest[from..].find(e).map(|i| from + i))
            .min()
            .unwrap_or(rest.len())
    };
    let mut equal: Vec<String> = Vec::new();
    let mut range: Option<String> = None;
    if let Some(w) = rest.find(" where ") {
        let body = &rest[w + 7..clause_end(w + 7)];
        if split_top(body, " or ").len() > 1 {
            return None;
        }
        for term in split_top(body, " and ") {
            let term = term.trim().trim_start_matches('(').trim();
            let Some(col) = column_of(term, &table) else {
                continue;
            };
            let after = term[term.find(' ').unwrap_or(term.len())..].trim_start();
            let is_equal = after.starts_with("= ")
                || after.starts_with("in ")
                || after.starts_with("in(")
                || after.starts_with("is null")
                || after.starts_with("<=> ");
            let is_range = [">", "<", "between ", "like ''"]
                .iter()
                .any(|op| after.starts_with(op))
                && !after.starts_with("<>");
            if is_equal && !equal.contains(&col) {
                equal.push(col);
            } else if is_range && range.is_none() {
                range = Some(col);
            }
        }
    }
    let mut columns = equal;
    match range {
        Some(col) if !columns.contains(&col) => columns.push(col),
        Some(_) => {}
        None => {
            if let Some(o) = rest.find(" order by ") {
                let body = &rest[o + 10..];
                let body = &body[..[" limit ", " offset ", " for update"]
                    .iter()
                    .filter_map(|e| body.find(e))
                    .min()
                    .unwrap_or(body.len())];
                for item in split_top(body, ",") {
                    match column_of(item.trim(), &table) {
                        Some(col) if !columns.contains(&col) => columns.push(col),
                        _ => break,
                    }
                }
            }
        }
    }
    columns.truncate(4);
    (!columns.is_empty()).then_some(Advice { table, columns })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn advice(sql: &str) -> Option<String> {
        index_for(sql).map(|a| a.describe())
    }

    #[test]
    fn equality_first_then_the_range_or_the_sort() {
        assert_eq!(
            advice(r#"select * from "orders" where "status" = 'paid' and "created_at" > '2024-01-01 00:00:00' order by "total" desc limit 1"#).as_deref(),
            Some("orders (status, created_at)")
        );
        assert_eq!(
            advice("select * from `orders` where `orders`.`user_id` = 3 and `orders`.`deleted_at` is null order by `created_at` desc").as_deref(),
            Some("orders (user_id, deleted_at, created_at)")
        );
        assert_eq!(
            advice("select count(*) as aggregate from `orders` where `status` in ('paid', 'sent')")
                .as_deref(),
            Some("orders (status)")
        );
    }

    #[test]
    fn what_no_single_index_serves_gets_no_advice() {
        assert_eq!(
            advice("select * from orders where status = 'a' or total > 3"),
            None
        );
        assert_eq!(advice("select * from orders"), None);
        // A keyword inside a value doesn't end the clause.
        assert_eq!(
            advice("select * from orders where note = 'x or y order by z' and paid = 1").as_deref(),
            Some("orders (note, paid)")
        );
        // Another table's column isn't this table's index.
        assert_eq!(
            advice("select * from orders inner join users on users.id = orders.user_id where users.email = 'a'"),
            None
        );
    }

    #[test]
    fn a_full_scan_is_answered_with_an_index_unless_one_exists() {
        let mut e = Explained {
            sql: "select * from \"orders\" where \"status\" = 'paid'".into(),
            engine: "sqlite".into(),
            columns: vec![
                "id".into(),
                "parent".into(),
                "notused".into(),
                "detail".into(),
            ],
            rows: vec![vec![
                Some("2".into()),
                Some("0".into()),
                Some("0".into()),
                Some("SCAN orders".into()),
            ]],
            error: None,
            indexes: vec![],
        };
        let v = verdict(std::slice::from_ref(&e));
        assert_eq!(v.issues, vec!["Full scan: SCAN orders".to_string()]);
        assert_eq!(
            label(&v, 1),
            "EXPLAIN ✖ Full scan: SCAN orders — an index on orders (status) would cover it"
        );
        let a = v.advice.unwrap();
        assert_eq!(
            a.file_name("2026_10_30_120000"),
            "2026_10_30_120000_add_status_index_to_orders_table.php"
        );
        assert!(a
            .migration()
            .contains("            $table->index(['status']);\n"));
        assert!(a
            .migration()
            .contains("            $table->dropIndex(['status']);\n"));
        // An index starting with the column: no advice.
        e.indexes = vec![("orders".into(), vec!["status".into(), "total".into()])];
        assert_eq!(verdict(&[e]).advice, None);
    }
}
