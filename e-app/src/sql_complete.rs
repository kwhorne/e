//! What SQL completion offers at the cursor, from the schema of the database
//! the SQL runs against (DB-402):
//!
//! - table names after `FROM`, `JOIN`, `INTO`, `UPDATE`, `TABLE`, and after a
//!   comma in a `FROM` list;
//! - after `alias.` or `table.`, the columns of that one table, the alias
//!   read from anywhere in the statement (`SELECT u.| FROM users u` works);
//! - elsewhere, the columns of the tables the statement names, ahead of the
//!   keywords that fit the clause the cursor is in, then the rest.
//!
//! Strings and comments are skipped, so neither a `;` nor a `FROM` inside
//! one is taken for SQL, and nothing is offered while typing in one.

use std::collections::HashMap;

use lsp_types::{CompletionItem, CompletionItemKind};

pub(crate) type Schema = HashMap<String, Vec<e_db::ColumnInfo>>;

/// Completion at byte `cursor` in `sql`: the length of the word being typed
/// (which an accepted item replaces) and the items. `None` when there is
/// nothing to offer.
pub(crate) fn complete(
    sql: &str,
    cursor: usize,
    schema: &Schema,
) -> Option<(usize, Vec<CompletionItem>)> {
    if cursor > sql.len() || !sql.is_char_boundary(cursor) {
        return None;
    }
    let toks = tokenize(sql);
    if toks.iter().any(|t| t.hides(cursor)) {
        return None;
    }
    let prefix_start = sql[..cursor]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_word_char(*c))
        .last()
        .map_or(cursor, |(i, _)| i);
    let prefix = &sql[prefix_start..cursor];

    // The statement the cursor is in, without comments.
    let start = toks
        .iter()
        .rev()
        .find(|t| t.kind == Kind::Semi && t.end <= prefix_start)
        .map_or(0, |t| t.end);
    let end = toks
        .iter()
        .find(|t| t.kind == Kind::Semi && t.start >= cursor)
        .map_or(sql.len(), |t| t.start);
    let stmt: Vec<&Tok> = toks
        .iter()
        .filter(|t| t.start >= start && t.end <= end && t.kind != Kind::Comment)
        .collect();
    let before: Vec<&Tok> = stmt
        .iter()
        .copied()
        .filter(|t| t.end <= prefix_start)
        .collect();
    let refs = table_refs(&stmt, schema);

    // `alias.` / `table.`: that table's columns.
    if let [.., name, dot] = before.as_slice() {
        if dot.kind == Kind::Dot && dot.end == prefix_start {
            let table = name.ident().and_then(|q| resolve(q, &refs, schema))?;
            let items = rank(
                prefix,
                columns_of(schema, &[table.as_str()], false),
                Vec::new(),
            );
            return (!items.is_empty()).then_some((prefix.len(), items));
        }
    }

    let clause = clause_of(&before);
    let wants_tables = match before.last() {
        Some(t) if t.kind == Kind::Word => matches!(
            t.upper().as_str(),
            "FROM" | "JOIN" | "INTO" | "UPDATE" | "TABLE"
        ),
        Some(t) if t.kind == Kind::Comma => clause == Some("FROM"),
        _ => false,
    };
    if wants_tables {
        let mut names: Vec<&String> = schema.keys().collect();
        names.sort();
        let tables = names
            .into_iter()
            .map(|t| item(t, CompletionItemKind::CLASS, "table".into()))
            .collect();
        let items = rank(prefix, tables, Vec::new());
        return (!items.is_empty()).then_some((prefix.len(), items));
    }
    // Nothing is offered on a bare space outside a table position.
    if prefix.is_empty() {
        return None;
    }
    let keywords = keywords_for(clause, prefix);
    // The first word of a statement is a keyword.
    let columns = if before.is_empty() {
        Vec::new()
    } else {
        let named: Vec<&str> = refs.iter().map(|(_, t)| t.as_str()).collect();
        if named.is_empty() {
            let all: Vec<&str> = schema.keys().map(String::as_str).collect();
            columns_of(schema, &all, false)
        } else {
            columns_of(schema, &named, named.len() > 1)
        }
    };
    let items = rank(prefix, columns, keywords);
    (!items.is_empty()).then_some((prefix.len(), items))
}

/// Order `columns` and `keywords` for `prefix`: names that start with it
/// first (columns, then keywords), then columns that only contain it.
/// Keywords only count when they start with it.
fn rank(
    prefix: &str,
    columns: Vec<CompletionItem>,
    keywords: Vec<CompletionItem>,
) -> Vec<CompletionItem> {
    let lower = prefix.to_lowercase();
    let mut seen = std::collections::HashSet::new();
    let (starts, contains): (Vec<_>, Vec<_>) = columns
        .into_iter()
        .filter(|i| i.label.to_lowercase().contains(&lower))
        .partition(|i| i.label.to_lowercase().starts_with(&lower));
    starts
        .into_iter()
        .chain(keywords)
        .chain(contains)
        .filter(|i| seen.insert(i.label.clone()))
        .collect()
}

fn item(label: &str, kind: CompletionItemKind, detail: String) -> CompletionItem {
    CompletionItem {
        label: label.to_string(),
        insert_text: Some(label.to_string()),
        kind: Some(kind),
        detail: Some(detail),
        ..Default::default()
    }
}

/// The columns of `tables`, by name; `with_table` says which table each is
/// from, for a statement naming several.
fn columns_of(schema: &Schema, tables: &[&str], with_table: bool) -> Vec<CompletionItem> {
    let mut cols: Vec<(&str, &e_db::ColumnInfo)> = tables
        .iter()
        .filter_map(|t| schema.get(*t).map(|cs| (*t, cs)))
        .flat_map(|(t, cs)| cs.iter().map(move |c| (t, c)))
        .collect();
    cols.sort_by(|a, b| a.1.name.cmp(&b.1.name).then(a.0.cmp(b.0)));
    cols.into_iter()
        .map(|(t, c)| {
            let detail = if with_table {
                format!("{t} · {}", c.data_type)
            } else {
                format!("column · {}", c.data_type)
            };
            item(&c.name, CompletionItemKind::FIELD, detail)
        })
        .collect()
}

/// The keywords that start with `prefix`: those that fit `clause` first, in
/// the order they're most often wanted there, then the rest. Written in
/// the case `prefix` is typed in.
fn keywords_for(clause: Option<&str>, prefix: &str) -> Vec<CompletionItem> {
    let upper = prefix.to_uppercase();
    let lowercase = !prefix.chars().any(char::is_uppercase);
    let likely: &[&str] = match clause {
        None => &[
            "SELECT",
            "INSERT INTO",
            "UPDATE",
            "DELETE FROM",
            "WITH",
            "EXPLAIN",
            "SHOW",
        ],
        Some("SELECT") => &["FROM", "AS", "DISTINCT", "CASE", "COUNT", "AND"],
        Some("FROM") | Some("JOIN") => &[
            "WHERE",
            "JOIN",
            "LEFT JOIN",
            "INNER JOIN",
            "ON",
            "AS",
            "GROUP BY",
            "ORDER BY",
            "LIMIT",
        ],
        Some("WHERE") | Some("ON") | Some("HAVING") => &[
            "AND",
            "OR",
            "NOT",
            "IN",
            "IS NULL",
            "IS NOT NULL",
            "LIKE",
            "BETWEEN",
            "EXISTS",
            "GROUP BY",
            "ORDER BY",
            "LIMIT",
        ],
        Some("GROUP") => &["HAVING", "ORDER BY", "LIMIT"],
        Some("ORDER") => &["ASC", "DESC", "LIMIT"],
        Some("LIMIT") => &["OFFSET"],
        Some("SET") => &["WHERE"],
        Some("INTO") => &["VALUES", "SELECT"],
        Some("VALUES") => &["ON DUPLICATE KEY UPDATE", "ON CONFLICT", "RETURNING"],
        _ => &[],
    };
    let rest = KEYWORDS.iter().filter(|k| !likely.contains(k));
    likely
        .iter()
        .chain(rest)
        .filter(|k| k.starts_with(&upper) && **k != upper)
        .map(|k| {
            let text = if lowercase {
                k.to_lowercase()
            } else {
                k.to_string()
            };
            item(&text, CompletionItemKind::KEYWORD, "keyword".into())
        })
        .collect()
}

/// Keywords offered anywhere, after those that fit the clause.
const KEYWORDS: &[&str] = &[
    "SELECT",
    "FROM",
    "WHERE",
    "AND",
    "OR",
    "NOT",
    "NULL",
    "IS NULL",
    "IS NOT NULL",
    "IN",
    "LIKE",
    "BETWEEN",
    "EXISTS",
    "AS",
    "ON",
    "JOIN",
    "LEFT JOIN",
    "RIGHT JOIN",
    "INNER JOIN",
    "CROSS JOIN",
    "GROUP BY",
    "HAVING",
    "ORDER BY",
    "ASC",
    "DESC",
    "LIMIT",
    "OFFSET",
    "DISTINCT",
    "UNION",
    "UNION ALL",
    "CASE",
    "WHEN",
    "THEN",
    "ELSE",
    "END",
    "COUNT",
    "SUM",
    "AVG",
    "MIN",
    "MAX",
    "COALESCE",
    "INSERT INTO",
    "VALUES",
    "UPDATE",
    "SET",
    "DELETE FROM",
    "RETURNING",
    "WITH",
    "EXPLAIN",
    "SHOW",
    "DESCRIBE",
    "CREATE TABLE",
    "CREATE INDEX",
    "ALTER TABLE",
    "DROP TABLE",
    "TRUNCATE",
    "BEGIN",
    "COMMIT",
    "ROLLBACK",
    "TRUE",
    "FALSE",
];

/// The clause the cursor is in: the last clause keyword before it.
fn clause_of(before: &[&Tok]) -> Option<&'static str> {
    const CLAUSES: &[&str] = &[
        "SELECT", "FROM", "JOIN", "WHERE", "ON", "GROUP", "ORDER", "HAVING", "LIMIT", "SET",
        "INTO", "VALUES", "UPDATE",
    ];
    before
        .iter()
        .rev()
        .filter(|t| t.kind == Kind::Word)
        .find_map(|t| CLAUSES.iter().find(|c| t.upper() == **c).copied())
        .map(|c| if c == "UPDATE" { "FROM" } else { c })
}

/// The tables the statement names after `FROM`, `JOIN`, `UPDATE` and
/// `INTO`, each with its alias (or `None`), as named in `schema`.
fn table_refs(stmt: &[&Tok], schema: &Schema) -> Vec<(Option<String>, String)> {
    let mut refs = Vec::new();
    let mut i = 0;
    while i < stmt.len() {
        let t = stmt[i];
        i += 1;
        if t.kind != Kind::Word {
            continue;
        }
        let kw = t.upper();
        if !matches!(kw.as_str(), "FROM" | "JOIN" | "UPDATE" | "INTO") {
            continue;
        }
        // `schema.table` names the table by its last part.
        while let Some(mut name) = stmt.get(i).and_then(|t| t.ident()) {
            i += 1;
            while stmt.get(i).is_some_and(|t| t.kind == Kind::Dot) {
                match stmt.get(i + 1).and_then(|t| t.ident()) {
                    Some(n) => {
                        name = n;
                        i += 2;
                    }
                    None => break,
                }
            }
            if stmt.get(i).is_some_and(|t| t.is_word("AS")) {
                i += 1;
            }
            let alias = match stmt.get(i) {
                Some(t) if t.kind == Kind::Quoted => Some(t.text.clone()),
                Some(t) if t.kind == Kind::Word && !RESERVED.contains(&t.upper().as_str()) => {
                    Some(t.text.clone())
                }
                _ => None,
            };
            if alias.is_some() {
                i += 1;
            }
            if let Some(table) = schema_table(name, schema) {
                refs.push((alias, table));
            }
            // `FROM a, b`: another table.
            if kw == "FROM" && stmt.get(i).is_some_and(|t| t.kind == Kind::Comma) {
                i += 1;
                continue;
            }
            break;
        }
    }
    refs
}

/// Words that end a table reference rather than alias it.
const RESERVED: &[&str] = &[
    "WHERE",
    "JOIN",
    "LEFT",
    "RIGHT",
    "INNER",
    "OUTER",
    "FULL",
    "CROSS",
    "NATURAL",
    "ON",
    "USING",
    "GROUP",
    "ORDER",
    "HAVING",
    "LIMIT",
    "OFFSET",
    "UNION",
    "EXCEPT",
    "INTERSECT",
    "SET",
    "VALUES",
    "SELECT",
    "WINDOW",
    "FOR",
    "RETURNING",
    "STRAIGHT_JOIN",
    "LATERAL",
    "PARTITION",
    "FINAL",
    "SAMPLE",
    "PREWHERE",
    "FORMAT",
    "SETTINGS",
    "AS",
    "DEFAULT",
];

/// The table `name` in `schema`, matched without regard to case.
fn schema_table(name: &str, schema: &Schema) -> Option<String> {
    if schema.contains_key(name) {
        return Some(name.to_string());
    }
    schema
        .keys()
        .find(|k| k.eq_ignore_ascii_case(name))
        .cloned()
}

/// The table `qualifier` stands for: an alias the statement defines, or
/// else a table of that name (`SELECT users.|`, before any `FROM`).
fn resolve(qualifier: &str, refs: &[(Option<String>, String)], schema: &Schema) -> Option<String> {
    refs.iter()
        .find(|(a, _)| {
            a.as_deref()
                .is_some_and(|a| a.eq_ignore_ascii_case(qualifier))
        })
        .map(|(_, t)| t.clone())
        .or_else(|| schema_table(qualifier, schema))
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Word,
    /// `` `name` `` or `"name"`; `text` is the name without its quotes.
    Quoted,
    Dot,
    Comma,
    Semi,
    Str,
    Comment,
    Other,
}

#[derive(Debug)]
struct Tok {
    kind: Kind,
    start: usize,
    end: usize,
    text: String,
    /// A string or comment that runs to the end of the text.
    open: bool,
}

impl Tok {
    fn upper(&self) -> String {
        self.text.to_ascii_uppercase()
    }

    fn is_word(&self, w: &str) -> bool {
        self.kind == Kind::Word && self.text.eq_ignore_ascii_case(w)
    }

    /// The identifier this is, if it is one.
    fn ident(&self) -> Option<&str> {
        matches!(self.kind, Kind::Word | Kind::Quoted).then_some(self.text.as_str())
    }

    /// Whether a cursor at `at` is inside this string or comment.
    fn hides(&self, at: usize) -> bool {
        matches!(self.kind, Kind::Str | Kind::Comment)
            && self.start < at
            && (at < self.end || self.open)
    }
}

fn tokenize(sql: &str) -> Vec<Tok> {
    let mut toks = Vec::new();
    let mut it = sql.char_indices().peekable();
    while let Some((start, c)) = it.next() {
        let mut open = false;
        let mut text = String::new();
        let kind = match c {
            c if c.is_whitespace() => continue,
            c if is_word_char(c) => {
                text.push(c);
                while let Some(&(_, c)) = it.peek() {
                    if !is_word_char(c) {
                        break;
                    }
                    text.push(c);
                    it.next();
                }
                Kind::Word
            }
            '\'' => {
                open = true;
                while let Some((_, c)) = it.next() {
                    if c == '\\' {
                        it.next();
                    } else if c == '\'' {
                        if it.peek().is_some_and(|&(_, c)| c == '\'') {
                            it.next();
                        } else {
                            open = false;
                            break;
                        }
                    }
                }
                Kind::Str
            }
            '`' | '"' => {
                open = true;
                while let Some((_, d)) = it.next() {
                    if d == c {
                        if it.peek().is_some_and(|&(_, e)| e == c) {
                            text.push(c);
                            it.next();
                        } else {
                            open = false;
                            break;
                        }
                    } else {
                        text.push(d);
                    }
                }
                Kind::Quoted
            }
            '-' if it.peek().is_some_and(|&(_, c)| c == '-') => {
                open = true;
                for (_, c) in it.by_ref() {
                    if c == '\n' {
                        open = false;
                        break;
                    }
                }
                Kind::Comment
            }
            '/' if it.peek().is_some_and(|&(_, c)| c == '*') => {
                it.next();
                open = true;
                let mut star = false;
                for (_, c) in it.by_ref() {
                    if star && c == '/' {
                        open = false;
                        break;
                    }
                    star = c == '*';
                }
                Kind::Comment
            }
            '.' => Kind::Dot,
            ',' => Kind::Comma,
            ';' => Kind::Semi,
            _ => Kind::Other,
        };
        let end = it.peek().map_or(sql.len(), |&(i, _)| i);
        toks.push(Tok {
            kind,
            start,
            end,
            text,
            open,
        });
    }
    toks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> Schema {
        let col = |name: &str, ty: &str| e_db::ColumnInfo {
            name: name.into(),
            data_type: ty.into(),
            nullable: false,
            key: String::new(),
        };
        HashMap::from([
            (
                "users".to_string(),
                vec![
                    col("id", "int"),
                    col("name", "varchar"),
                    col("email", "varchar"),
                ],
            ),
            (
                "orders".to_string(),
                vec![
                    col("id", "int"),
                    col("user_id", "int"),
                    col("total", "decimal"),
                ],
            ),
            ("posts".to_string(), vec![col("title", "varchar")]),
        ])
    }

    /// The labels offered with the cursor at `|`.
    fn at(sql: &str) -> Vec<String> {
        let cursor = sql.find('|').unwrap();
        let sql = sql.replace('|', "");
        complete(&sql, cursor, &schema())
            .map(|(_, items)| items.into_iter().map(|i| i.label).collect())
            .unwrap_or_default()
    }

    #[test]
    fn tables_after_from_join_and_a_comma() {
        assert_eq!(at("SELECT * FROM |"), ["orders", "posts", "users"]);
        assert_eq!(at("SELECT * FROM us|"), ["users"]);
        // Names that start with what is typed come first.
        assert_eq!(at("select * from users join o|"), ["orders", "posts"]);
        assert_eq!(at("SELECT * FROM users u, p|"), ["posts"]);
        assert_eq!(at("UPDATE u|"), ["users"]);
    }

    #[test]
    fn an_alias_gives_its_tables_columns() {
        assert_eq!(at("SELECT u.| FROM users u"), ["email", "id", "name"]);
        assert_eq!(at("SELECT * FROM users AS u WHERE u.em|"), ["email"]);
        assert_eq!(
            at("select * from users u join orders o on o.| = u.id"),
            ["id", "total", "user_id"]
        );
        // A table's own name works as well, alias or not.
        assert_eq!(at("SELECT orders.t| FROM orders"), ["total"]);
        assert_eq!(at("SELECT `o`.us| FROM `orders` `o`"), ["user_id"]);
        // An alias the statement doesn't define offers nothing.
        assert!(at("SELECT x.| FROM users u").is_empty());
    }

    #[test]
    fn columns_come_from_the_tables_the_statement_names() {
        // `title` is in posts, which this statement doesn't name.
        assert!(!at("SELECT * FROM users WHERE ti|").contains(&"title".to_string()));
        assert_eq!(at("SELECT * FROM posts WHERE ti|"), ["title"]);
        // Two tables: both tables' columns, `id` once; a column that starts
        // with what is typed, then keywords, then those that contain it.
        let both = at("SELECT i| FROM users u JOIN orders o ON o.user_id = u.id");
        assert_eq!(
            both,
            [
                "id",
                "is null",
                "is not null",
                "in",
                "inner join",
                "insert into",
                "email",
                "user_id"
            ]
        );
        // No table named yet: every column.
        assert!(at("SELECT ti|").contains(&"title".to_string()));
    }

    #[test]
    fn keywords_fit_the_clause_and_the_case_typed() {
        assert_eq!(at("sel|"), ["select"]);
        assert_eq!(at("DEL|"), ["DELETE FROM"]);
        // In WHERE, AND before the other A-keywords and the columns that
        // only contain an `a`.
        let w = at("SELECT * FROM orders WHERE total > 1 a|");
        assert_eq!(w[0], "and");
        // After FROM's table: the next clause.
        assert_eq!(at("SELECT * FROM users wh|"), ["where", "when"]);
        assert_eq!(at("SELECT * FROM users ORDER BY name d|")[0], "desc");
        // A keyword typed out in full isn't offered again.
        assert!(!at("SELECT * FROM users WHERE|").contains(&"WHERE".to_string()));
    }

    #[test]
    fn statements_strings_and_comments_are_respected() {
        // The other statement's table doesn't count.
        assert!(!at("SELECT * FROM posts; SELECT * FROM users WHERE ti|")
            .contains(&"title".to_string()));
        assert_eq!(
            at("SELECT * FROM users WHERE name = 'a;b' AND e|")[0],
            "email"
        );
        // Nothing inside a string or a comment.
        assert!(at("SELECT 'FROM u|").is_empty());
        assert!(at("SELECT 1 -- from u|").is_empty());
        assert!(at("SELECT /* u.| */ 1 FROM users u").is_empty());
        // Nothing on a bare space outside a table position.
        assert!(at("SELECT * FROM users WHERE |").is_empty());
    }

    #[test]
    fn the_word_typed_is_what_an_item_replaces() {
        let (len, _) = complete("SELECT * FROM us", 16, &schema()).unwrap();
        assert_eq!(len, 2);
        let (len, _) = complete("SELECT u. FROM users u", 9, &schema()).unwrap();
        assert_eq!(len, 0);
    }
}
