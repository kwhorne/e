//! What a pending migration will do, before it does it.
//!
//! `php artisan migrate --pretend` prints the SQL a migration would run
//! without running it. That SQL is read back into operations — add, change or
//! drop a column, add a unique index or a foreign key, drop a table — and each
//! is checked against the data already in the (local) database: a `NOT NULL`
//! column without a default on a table that has rows, a unique index over
//! values that repeat, a foreign key that rows would break, a column or table
//! dropped with its data in it. The editor shows the SQL beside the line that
//! produces it and the risks as diagnostics on that line.
//!
//! Parsing and the rules are pure and tested; [`DbFacts`] asks the database.

use std::collections::BTreeMap;

/// The database dialect, as far as the rules care.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Engine {
    Mysql,
    Postgres,
    Sqlite,
}

impl Engine {
    /// From an `e-db` engine name or a Laravel driver name.
    pub fn from_name(name: &str) -> Option<Engine> {
        match name.to_ascii_lowercase().as_str() {
            "mysql" | "mariadb" => Some(Engine::Mysql),
            "postgres" | "postgresql" | "pgsql" => Some(Engine::Postgres),
            "sqlite" => Some(Engine::Sqlite),
            _ => None,
        }
    }
}

/// A migration's statements, as `migrate --pretend` printed them, by
/// migration name.
pub fn parse_pretend(output: &str) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut current: Option<String> = None;
    for line in output.lines() {
        let t = line.trim();
        if let Some(sql) = t.strip_prefix('⇂') {
            if let Some(name) = &current {
                out.entry(name.clone())
                    .or_default()
                    .push(sql.trim().to_string());
            }
            continue;
        }
        // `  2024_05_01_000000_add_phone_to_users ........ ` (dots, or a time).
        let name = t.split_whitespace().next().unwrap_or("");
        if name.len() >= 18
            && name.as_bytes()[..4].iter().all(u8::is_ascii_digit)
            && name.as_bytes()[4] == b'_'
        {
            current = Some(name.to_string());
            out.entry(name.to_string()).or_default();
        } else if !t.is_empty() && !t.starts_with("INFO") {
            // A statement broken over lines continues the last one.
            if let Some(last) = current
                .as_ref()
                .and_then(|n| out.get_mut(n))
                .and_then(|v| v.last_mut())
            {
                last.push(' ');
                last.push_str(t);
            }
        }
    }
    out
}

/// What one statement does, as far as the rules care.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    CreateTable {
        table: String,
    },
    AddColumn {
        table: String,
        column: String,
        /// The type's name, lower case (`varchar`, `timestamp`, `bigint`).
        ty: String,
        not_null: bool,
        /// A default, or a value the database makes up (auto-increment,
        /// serial, generated).
        default: bool,
    },
    /// `->change()`: a new type, length or nullability.
    ChangeColumn {
        table: String,
        column: String,
        ty: Option<String>,
        /// The new maximum length (`varchar(100)`).
        length: Option<u32>,
        /// `Some(true)` when the column becomes (or stays) `NOT NULL`.
        not_null: Option<bool>,
    },
    AddUnique {
        table: String,
        columns: Vec<String>,
    },
    AddForeign {
        table: String,
        column: String,
        parent: String,
        parent_column: String,
    },
    DropColumn {
        table: String,
        column: String,
    },
    DropTable {
        table: String,
    },
}

impl Op {
    pub fn table(&self) -> &str {
        match self {
            Op::CreateTable { table }
            | Op::AddColumn { table, .. }
            | Op::ChangeColumn { table, .. }
            | Op::AddUnique { table, .. }
            | Op::AddForeign { table, .. }
            | Op::DropColumn { table, .. }
            | Op::DropTable { table } => table,
        }
    }

    /// The column the operation is about, if one.
    pub fn column(&self) -> Option<&str> {
        match self {
            Op::AddColumn { column, .. }
            | Op::ChangeColumn { column, .. }
            | Op::AddForeign { column, .. }
            | Op::DropColumn { column, .. } => Some(column),
            Op::AddUnique { columns, .. } => columns.first().map(String::as_str),
            Op::CreateTable { .. } | Op::DropTable { .. } => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Tok {
    /// An unquoted word, lower case: a keyword, a type, a number, or a bare
    /// identifier.
    Word(String),
    /// A quoted identifier, as written.
    Ident(String),
    Str,
    Punct(char),
}

fn tokenize(sql: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut chars = sql.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '`' | '"' => {
                let mut s = String::new();
                for n in chars.by_ref() {
                    if n == c {
                        break;
                    }
                    s.push(n);
                }
                out.push(Tok::Ident(s));
            }
            '\'' => {
                for n in chars.by_ref() {
                    if n == '\'' {
                        break;
                    }
                }
                out.push(Tok::Str);
            }
            c if c.is_whitespace() => {}
            c if c.is_alphanumeric() || c == '_' || c == '.' => {
                let mut s = c.to_ascii_lowercase().to_string();
                while let Some(&n) = chars.peek() {
                    if n.is_alphanumeric() || n == '_' || n == '.' {
                        s.push(n.to_ascii_lowercase());
                        chars.next();
                    } else {
                        break;
                    }
                }
                out.push(Tok::Word(s));
            }
            c => out.push(Tok::Punct(c)),
        }
    }
    out
}

fn is_word(t: Option<&Tok>, w: &str) -> bool {
    matches!(t, Some(Tok::Word(x)) if x == w)
}

/// An identifier, quoted or bare.
fn name(t: Option<&Tok>) -> Option<String> {
    match t? {
        Tok::Ident(s) | Tok::Word(s) => Some(s.clone()),
        _ => None,
    }
}

/// Split at commas outside parentheses.
fn clauses(toks: &[Tok]) -> Vec<&[Tok]> {
    let mut out = Vec::new();
    let mut depth = 0;
    let mut start = 0;
    for (i, t) in toks.iter().enumerate() {
        match t {
            Tok::Punct('(') => depth += 1,
            Tok::Punct(')') => depth -= 1,
            Tok::Punct(',') if depth == 0 => {
                out.push(&toks[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&toks[start..]);
    out
}

/// The identifiers in the parenthesised list starting at `toks[0]`.
fn paren_names(toks: &[Tok]) -> Vec<String> {
    if toks.first() != Some(&Tok::Punct('(')) {
        return Vec::new();
    }
    toks[1..]
        .iter()
        .take_while(|t| **t != Tok::Punct(')'))
        .filter_map(|t| name(Some(t)))
        .collect()
}

fn has_seq(toks: &[Tok], a: &str, b: &str) -> bool {
    toks.windows(2)
        .any(|w| is_word(Some(&w[0]), a) && is_word(Some(&w[1]), b))
}

/// A column definition's type, length, nullability and default:
/// `varchar(255) not null default 'x'`.
fn column_def(def: &[Tok]) -> (String, Option<u32>, bool, bool) {
    let ty = match def.first() {
        Some(Tok::Word(w)) => w.clone(),
        _ => String::new(),
    };
    let length = match (def.get(1), def.get(2)) {
        (Some(Tok::Punct('(')), Some(Tok::Word(n))) => n.parse().ok(),
        _ => None,
    };
    let not_null = has_seq(def, "not", "null");
    let made_up = def.iter().any(|t| {
        matches!(t, Tok::Word(w) if matches!(w.as_str(),
            "default" | "auto_increment" | "autoincrement" | "generated" | "identity"))
    }) || matches!(ty.as_str(), "serial" | "bigserial" | "smallserial");
    (ty, length, not_null, made_up)
}

/// The operations one statement performs.
pub fn parse_statement(sql: &str) -> Vec<Op> {
    let toks = tokenize(sql);
    let w = |i: usize, s: &str| is_word(toks.get(i), s);

    if w(0, "create") && w(1, "table") {
        let at = if w(2, "if") { 5 } else { 2 };
        return name(toks.get(at))
            .map(|table| vec![Op::CreateTable { table }])
            .unwrap_or_default();
    }
    if w(0, "drop") && w(1, "table") {
        let at = if w(2, "if") { 4 } else { 2 };
        return name(toks.get(at))
            .map(|table| vec![Op::DropTable { table }])
            .unwrap_or_default();
    }
    if w(0, "create") && w(1, "unique") && w(2, "index") {
        let Some(on) = toks.iter().position(|t| is_word(Some(t), "on")) else {
            return Vec::new();
        };
        let Some(table) = name(toks.get(on + 1)) else {
            return Vec::new();
        };
        let columns = paren_names(&toks[(on + 2).min(toks.len())..]);
        return vec![Op::AddUnique { table, columns }];
    }
    if !(w(0, "alter") && w(1, "table")) {
        return Vec::new();
    }
    let Some(table) = name(toks.get(2)) else {
        return Vec::new();
    };
    let mut ops = Vec::new();
    for c in clauses(&toks[3..]) {
        let cw = |i: usize, s: &str| is_word(c.get(i), s);
        if cw(0, "add") && cw(1, "constraint") {
            // `add constraint x foreign key (c) references p (pc)` / `… unique (c)`
            if cw(3, "foreign") && cw(4, "key") {
                let cols = paren_names(&c[5..]);
                let Some(r) = c.iter().position(|t| is_word(Some(t), "references")) else {
                    continue;
                };
                let (Some(column), Some(parent)) = (cols.first().cloned(), name(c.get(r + 1)))
                else {
                    continue;
                };
                let parent_column = paren_names(&c[(r + 2).min(c.len())..])
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "id".into());
                ops.push(Op::AddForeign {
                    table: table.clone(),
                    column,
                    parent,
                    parent_column,
                });
            } else if cw(3, "unique") {
                ops.push(Op::AddUnique {
                    table: table.clone(),
                    columns: paren_names(&c[4..]),
                });
            }
        } else if cw(0, "add") && cw(1, "unique") {
            // MySQL: `add unique \`idx\`(\`a\`, \`b\`)`
            let open = c
                .iter()
                .position(|t| *t == Tok::Punct('('))
                .unwrap_or(c.len());
            ops.push(Op::AddUnique {
                table: table.clone(),
                columns: paren_names(&c[open..]),
            });
        } else if cw(0, "add")
            && !matches!(c.get(1), Some(Tok::Word(k)) if matches!(k.as_str(),
                "index" | "primary" | "fulltext" | "spatial" | "key" | "foreign" | "check"))
        {
            let at = if cw(1, "column") { 2 } else { 1 };
            let Some(column) = name(c.get(at)) else {
                continue;
            };
            let (ty, _, not_null, default) = column_def(&c[at + 1..]);
            ops.push(Op::AddColumn {
                table: table.clone(),
                column,
                ty,
                not_null,
                default,
            });
        } else if cw(0, "modify") || cw(0, "change") {
            // MySQL: `modify \`c\` <def>`, or `change \`old\` \`new\` <def>` —
            // the data is under the old name.
            let at = if cw(1, "column") { 2 } else { 1 };
            let Some(column) = name(c.get(at)) else {
                continue;
            };
            let def_at = if cw(0, "change") { at + 2 } else { at + 1 };
            let (ty, length, not_null, _) = column_def(&c[def_at.min(c.len())..]);
            ops.push(Op::ChangeColumn {
                table: table.clone(),
                column,
                ty: Some(ty),
                length,
                not_null: Some(not_null),
            });
        } else if cw(0, "alter") && cw(1, "column") {
            // Postgres: `alter column "c" type varchar(100)` / `set not null`
            // / `drop not null`.
            let Some(column) = name(c.get(2)) else {
                continue;
            };
            if cw(3, "type") {
                let (ty, length, _, _) = column_def(&c[4..]);
                ops.push(Op::ChangeColumn {
                    table: table.clone(),
                    column,
                    ty: Some(ty),
                    length,
                    not_null: None,
                });
            } else if cw(3, "set") && cw(4, "not") {
                ops.push(Op::ChangeColumn {
                    table: table.clone(),
                    column,
                    ty: None,
                    length: None,
                    not_null: Some(true),
                });
            }
        } else if cw(0, "drop")
            && !matches!(c.get(1), Some(Tok::Word(k)) if matches!(k.as_str(),
                "index" | "primary" | "foreign" | "constraint" | "key" | "unique" | "check"))
        {
            let at = if cw(1, "column") { 2 } else { 1 };
            if let Some(column) = name(c.get(at)) {
                ops.push(Op::DropColumn {
                    table: table.clone(),
                    column,
                });
            }
        }
    }
    ops
}

/// What the database knows about the data a migration touches. `None` when
/// it can't say (no such table yet, no connection).
pub trait Facts {
    fn rows(&self, table: &str) -> Option<i64>;
    fn nulls(&self, table: &str, column: &str) -> Option<i64>;
    fn non_nulls(&self, table: &str, column: &str) -> Option<i64>;
    /// How many value combinations of `columns` occur more than once.
    fn duplicates(&self, table: &str, columns: &[String]) -> Option<i64>;
    /// How many values of `column` are longer than `len` characters.
    fn longer_than(&self, table: &str, column: &str, len: u32) -> Option<i64>;
    /// How many rows have a `column` that no `parent` row has as `parent_column`.
    fn orphans(&self, table: &str, column: &str, parent: &str, parent_column: &str) -> Option<i64>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    /// The migration will fail.
    Error,
    /// It will run, and do something you may not want.
    Warning,
    Info,
}

/// A problem one operation will have with the data that's there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Risk {
    pub op: Op,
    pub severity: Severity,
    pub message: String,
}

fn n(count: i64) -> String {
    // 14200 → "14 200": readable at a glance in a diagnostic.
    let s = count.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

fn rows_word(count: i64) -> &'static str {
    if count == 1 {
        "row"
    } else {
        "rows"
    }
}

fn is_date(ty: &str) -> bool {
    matches!(ty, "date" | "datetime" | "timestamp" | "time" | "year")
}

/// Check each operation against the data. Tables the migration creates
/// itself are empty, so they're not asked about.
pub fn risks(ops: &[Op], engine: Engine, facts: &dyn Facts) -> Vec<Risk> {
    let created: Vec<&str> = ops
        .iter()
        .filter_map(|o| match o {
            Op::CreateTable { table } => Some(table.as_str()),
            _ => None,
        })
        .collect();
    let mut out = Vec::new();
    let mut risk = |op: &Op, severity: Severity, message: String| {
        out.push(Risk {
            op: op.clone(),
            severity,
            message,
        })
    };
    for op in ops {
        let table = op.table();
        if created.contains(&table) {
            continue;
        }
        match op {
            Op::AddColumn {
                column,
                ty,
                not_null: true,
                default: false,
                ..
            } => {
                if engine == Engine::Sqlite {
                    risk(
                        op,
                        Severity::Error,
                        format!(
                            "SQLite can't add NOT NULL column `{column}` without a default — \
                             add ->nullable() or ->default(…)"
                        ),
                    );
                    continue;
                }
                let Some(rows) = facts.rows(table).filter(|r| *r > 0) else {
                    continue;
                };
                let have = format!("`{table}` has {} {}", n(rows), rows_word(rows));
                match engine {
                    Engine::Postgres => risk(
                        op,
                        Severity::Error,
                        format!(
                            "{have} and none has a `{column}`: Postgres refuses a NOT NULL \
                             column without a default — add ->nullable() or ->default(…)"
                        ),
                    ),
                    Engine::Mysql if is_date(ty) => risk(
                        op,
                        Severity::Error,
                        format!(
                            "{have}: MySQL would give each the zero date for `{column}`, \
                             which strict mode (Laravel's default) refuses — add ->nullable() \
                             or ->useCurrent()"
                        ),
                    ),
                    _ => {
                        let empty = if ty.contains("char") || ty.contains("text") {
                            "''"
                        } else if ty.contains("int")
                            || matches!(ty.as_str(), "decimal" | "double" | "float")
                        {
                            "0"
                        } else {
                            "the type's empty value"
                        };
                        risk(
                            op,
                            Severity::Warning,
                            format!(
                                "{have}: MySQL sets `{column}` to {empty} in each — add \
                                 ->default(…) or ->nullable() if that's not what you want"
                            ),
                        )
                    }
                }
            }
            Op::ChangeColumn {
                column,
                length,
                not_null,
                ty,
                ..
            } => {
                if *not_null == Some(true) {
                    if let Some(nulls) = facts.nulls(table, column).filter(|c| *c > 0) {
                        risk(
                            op,
                            Severity::Error,
                            format!(
                                "{} {} of `{table}` {} NULL in `{column}`, so it can't become \
                                 NOT NULL — fill them first, or keep ->nullable()",
                                n(nulls),
                                rows_word(nulls),
                                if nulls == 1 { "has" } else { "have" }
                            ),
                        );
                    }
                }
                if let Some(len) = length {
                    if let Some(long) = facts.longer_than(table, column, *len).filter(|c| *c > 0) {
                        risk(
                            op,
                            Severity::Error,
                            format!(
                                "`{column}` is longer than {len} characters in {} {} of \
                                 `{table}`: the change would fail on them",
                                n(long),
                                rows_word(long),
                            ),
                        );
                    }
                }
                if ty.is_some() && engine != Engine::Sqlite {
                    if let Some(rows) = facts.rows(table).filter(|r| *r > 0) {
                        risk(
                            op,
                            Severity::Info,
                            format!(
                                "Rewrites `{table}` ({} {}): quick here, but on a large \
                                 production table it holds a lock while it runs",
                                n(rows),
                                rows_word(rows)
                            ),
                        );
                    }
                }
            }
            Op::AddUnique { columns, .. } if !columns.is_empty() => {
                if let Some(d) = facts.duplicates(table, columns).filter(|c| *c > 0) {
                    let cols = columns
                        .iter()
                        .map(|c| format!("`{c}`"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    risk(
                        op,
                        Severity::Error,
                        format!(
                            "{} {} of {cols} in `{table}` occur more than once: the unique \
                             index can't be built",
                            n(d),
                            if d == 1 { "value" } else { "values" }
                        ),
                    );
                }
            }
            Op::AddForeign {
                column,
                parent,
                parent_column,
                ..
            } => {
                if let Some(o) = facts
                    .orphans(table, column, parent, parent_column)
                    .filter(|c| *c > 0)
                {
                    risk(
                        op,
                        Severity::Error,
                        format!(
                            "`{column}` matches no `{parent}` row in {} {} of `{table}`: the \
                             foreign key can't be added",
                            n(o),
                            rows_word(o),
                        ),
                    );
                }
            }
            Op::DropColumn { column, .. } => {
                if let Some(v) = facts.non_nulls(table, column).filter(|c| *c > 0) {
                    risk(
                        op,
                        Severity::Warning,
                        format!(
                            "Drops `{column}` with the values of {} {} — down() can add the \
                             column back, not what was in it",
                            n(v),
                            rows_word(v)
                        ),
                    );
                }
            }
            Op::DropTable { .. } => {
                if let Some(rows) = facts.rows(table).filter(|r| *r > 0) {
                    risk(
                        op,
                        Severity::Warning,
                        format!(
                            "Drops `{table}` with its {} {} — down() can create the table \
                             again, not its data",
                            n(rows),
                            rows_word(rows)
                        ),
                    );
                }
            }
            _ => {}
        }
    }
    out
}

/// [`Facts`] from a live database connection. Counts only: nothing is
/// changed, and a query that fails (the table isn't there yet) is "can't say".
pub struct DbFacts<'a> {
    pub conn: &'a e_db::Conn,
    /// The `e-db` engine name, for quoting.
    pub engine: &'a str,
}

impl DbFacts<'_> {
    fn q(&self, ident: &str) -> String {
        e_db::quote_ident(self.engine, ident)
    }

    fn count(&self, sql: &str) -> Option<i64> {
        let res = e_db::query(self.conn, sql, 1).ok()?;
        res.rows.first()?.first()?.as_ref()?.parse().ok()
    }
}

impl Facts for DbFacts<'_> {
    fn rows(&self, table: &str) -> Option<i64> {
        self.count(&format!("SELECT COUNT(*) FROM {}", self.q(table)))
    }

    fn nulls(&self, table: &str, column: &str) -> Option<i64> {
        self.count(&format!(
            "SELECT COUNT(*) FROM {} WHERE {} IS NULL",
            self.q(table),
            self.q(column)
        ))
    }

    fn non_nulls(&self, table: &str, column: &str) -> Option<i64> {
        self.count(&format!(
            "SELECT COUNT(*) FROM {} WHERE {} IS NOT NULL",
            self.q(table),
            self.q(column)
        ))
    }

    fn duplicates(&self, table: &str, columns: &[String]) -> Option<i64> {
        let cols: Vec<String> = columns.iter().map(|c| self.q(c)).collect();
        let not_null: Vec<String> = cols.iter().map(|c| format!("{c} IS NOT NULL")).collect();
        self.count(&format!(
            "SELECT COUNT(*) FROM (SELECT 1 AS d FROM {} WHERE {} GROUP BY {} HAVING COUNT(*) > 1) dup",
            self.q(table),
            not_null.join(" AND "),
            cols.join(", ")
        ))
    }

    fn longer_than(&self, table: &str, column: &str, len: u32) -> Option<i64> {
        let f = if self.engine == "sqlite" {
            "LENGTH"
        } else {
            "CHAR_LENGTH"
        };
        self.count(&format!(
            "SELECT COUNT(*) FROM {} WHERE {f}({}) > {len}",
            self.q(table),
            self.q(column)
        ))
    }

    fn orphans(&self, table: &str, column: &str, parent: &str, parent_column: &str) -> Option<i64> {
        let (t, c, p, pc) = (
            self.q(table),
            self.q(column),
            self.q(parent),
            self.q(parent_column),
        );
        self.count(&format!(
            "SELECT COUNT(*) FROM {t} child WHERE child.{c} IS NOT NULL \
             AND NOT EXISTS (SELECT 1 FROM {p} parent WHERE parent.{pc} = child.{c})"
        ))
    }
}

/// A pending migration, checked: the SQL it will run and the risks, each at
/// the line of the migration it comes from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Checked {
    /// `(line, sql)`: each statement at the line it comes from.
    pub statements: Vec<(usize, String)>,
    /// `(line, risk)`.
    pub risks: Vec<(usize, Risk)>,
    /// False when `migrate --pretend` had nothing to say about this file:
    /// it has already run.
    pub pending: bool,
}

/// `migrate --pretend` for the migration at `file`. Blocking: boots the app.
pub fn pretend(root: &std::path::Path, file: &std::path::Path) -> Result<Vec<String>, String> {
    let path = format!("--path={}", file.to_string_lossy());
    let out = crate::phptools::php(
        root,
        &[
            "artisan",
            "migrate",
            "--pretend",
            &path,
            "--realpath",
            "--no-ansi",
        ],
        std::time::Duration::from_secs(30),
    )?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let first = stdout
            .lines()
            .chain(err.lines())
            .map(str::trim)
            .find(|l| !l.is_empty() && !l.starts_with("INFO"))
            .unwrap_or("migrate --pretend failed");
        return Err(first.to_string());
    }
    let name = file.file_stem().map(|s| s.to_string_lossy().into_owned());
    Ok(parse_pretend(&stdout)
        .into_iter()
        .find(|(n, _)| Some(n) == name.as_ref())
        .map(|(_, sql)| sql)
        .unwrap_or_default())
}

/// Check the migration whose source is `src` given the SQL it will run.
/// Without `facts` (no database to ask), only the SQL is placed.
pub fn check(src: &str, statements: &[String], facts: Option<(&dyn Facts, Engine)>) -> Checked {
    let mut out = Checked {
        pending: !statements.is_empty(),
        ..Default::default()
    };
    let mut ops = Vec::new();
    for sql in statements {
        let these = parse_statement(sql);
        let line = these
            .first()
            .and_then(|op| line_of(src, op))
            .or_else(|| {
                line_of(
                    src,
                    &Op::DropTable {
                        table: String::new(),
                    },
                )
            })
            .unwrap_or(0);
        out.statements.push((line, sql.clone()));
        ops.extend(these);
    }
    if let Some((facts, engine)) = facts {
        out.risks = risks(&ops, engine, facts)
            .into_iter()
            .map(|r| (line_of(src, &r.op).unwrap_or(0), r))
            .collect();
    }
    out
}

/// What running a migration's `up()` and `down()` inside a transaction that
/// is then rolled back showed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DryRun {
    /// Both ran; nothing was kept. `steps` in the order they ran: `up`, `down`
    /// for a pending migration, `down`, `up` for one that has run.
    Passed { driver: String, steps: Vec<String> },
    /// `step` threw; nothing was kept.
    Failed {
        driver: String,
        step: String,
        error: String,
    },
    /// Not tried: the database commits schema changes as it makes them
    /// (MySQL, MariaDB), so a rollback couldn't undo them.
    NotTransactional { driver: String },
}

/// The PHP behind [`dry_run`]. The migration's path is `$argv[1]`.
const DRY_RUN_PHP: &str = r#"
require 'vendor/autoload.php';
$app = require 'bootstrap/app.php';
$app->make(Illuminate\Contracts\Console\Kernel::class)->bootstrap();
$out = ['driver' => '', 'ran' => false, 'steps' => [], 'step' => null, 'error' => null];
try {
    $m = require $argv[1];
    $name = $m->getConnection();
    if ($name) { Illuminate\Support\Facades\DB::setDefaultConnection($name); }
    $db = Illuminate\Support\Facades\DB::connection();
    $out['driver'] = $db->getDriverName();
    if (in_array($out['driver'], ['pgsql', 'sqlite'], true)) {
        $table = config('database.migrations');
        $table = is_array($table) ? ($table['table'] ?? 'migrations') : ($table ?: 'migrations');
        $out['ran'] = $db->table($table)
            ->where('migration', pathinfo($argv[1], PATHINFO_FILENAME))->exists();
        $db->beginTransaction();
        try {
            foreach ($out['ran'] ? ['down', 'up'] : ['up', 'down'] as $step) {
                $out['step'] = $step;
                $m->{$step}();
                $out['steps'][] = $step;
            }
            $out['step'] = null;
        } catch (\Throwable $e) {
            $out['error'] = $e->getMessage();
        } finally {
            $db->rollBack();
        }
    }
} catch (\Throwable $e) {
    $out['error'] = $e->getMessage();
}
echo "
__E_DRY_RUN__" . json_encode($out);
"#;

/// Run the migration at `file` — `up()` then `down()` if it is pending,
/// `down()` then `up()` if it has run — inside a transaction that is rolled
/// back whatever happens, so the database is left as it was. Only on
/// Postgres and SQLite, whose schema changes are transactional; on MySQL the
/// first `CREATE` or `ALTER` would commit, so nothing is run there. Code in
/// the migration that reaches outside the database (files, queues, HTTP) is
/// not undone. Blocking: boots the app.
pub fn dry_run(root: &std::path::Path, file: &std::path::Path) -> Result<DryRun, String> {
    let file = file.to_string_lossy();
    let out = crate::phptools::php(
        root,
        &["-r", DRY_RUN_PHP, "--", &file],
        std::time::Duration::from_secs(60),
    )?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let Some((_, json)) = stdout.rsplit_once("__E_DRY_RUN__") else {
        let err = String::from_utf8_lossy(&out.stderr);
        let first = stdout
            .lines()
            .chain(err.lines())
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("the app didn't boot");
        return Err(first.to_string());
    };
    let v: serde_json::Value = serde_json::from_str(json.trim()).map_err(|e| e.to_string())?;
    Ok(read_dry_run(&v))
}

fn read_dry_run(v: &serde_json::Value) -> DryRun {
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let driver = s("driver");
    if !matches!(driver.as_str(), "pgsql" | "sqlite") && !driver.is_empty() {
        return DryRun::NotTransactional { driver };
    }
    let error = s("error");
    if !error.is_empty() {
        return DryRun::Failed {
            driver,
            step: s("step"),
            error,
        };
    }
    let steps = v
        .get("steps")
        .and_then(|x| x.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    DryRun::Passed { driver, steps }
}

/// The line (0-based) in a migration's source that an operation comes from:
/// where `up()` names its column, or its table. `None` when it can't be found.
pub fn line_of(src: &str, op: &Op) -> Option<usize> {
    let lines: Vec<&str> = src.lines().collect();
    let up = lines.iter().position(|l| l.contains("function up"))?;
    let end = lines
        .iter()
        .skip(up + 1)
        .position(|l| l.contains("function down"))
        .map(|i| up + 1 + i)
        .unwrap_or(lines.len());
    let quoted = |s: &str| [format!("'{s}'"), format!("\"{s}\"")];
    let find = |needle: &[String; 2], also: Option<&str>| {
        (up..end).find(|&i| {
            let l = lines[i];
            needle.iter().any(|n| l.contains(n.as_str())) && also.is_none_or(|a| l.contains(a))
        })
    };
    let table = quoted(op.table());
    let by_column = |also: Option<&str>| op.column().and_then(|c| find(&quoted(c), also));
    match op {
        Op::CreateTable { .. } | Op::DropTable { .. } => find(&table, None),
        Op::AddUnique { .. } => by_column(Some("unique")).or_else(|| by_column(None)),
        Op::DropColumn { .. } => by_column(Some("drop")).or_else(|| by_column(None)),
        _ => by_column(None),
    }
    .or_else(|| find(&table, None))
    .or(Some(up))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_pretend_output() {
        // As Laravel 12 printed it for a probe migration.
        let out = "
   INFO  Running migrations.

  2099_01_01_000000_e_probe_add_phone ........................................
  ⇂ alter table `users` add `phone` varchar(255) not null
  ⇂ alter table `users` drop `remember_token`
  ⇂ create table `probe_things` (`id` bigint unsigned not null auto_increment primary key)
  2099_01_02_000000_second .......................................
  ⇂ drop table if exists `old`
";
        let m = parse_pretend(out);
        assert_eq!(
            m["2099_01_01_000000_e_probe_add_phone"],
            [
                "alter table `users` add `phone` varchar(255) not null",
                "alter table `users` drop `remember_token`",
                "create table `probe_things` (`id` bigint unsigned not null auto_increment primary key)",
            ]
        );
        assert_eq!(
            m["2099_01_02_000000_second"],
            ["drop table if exists `old`"]
        );
    }

    fn add(table: &str, column: &str, ty: &str, not_null: bool, default: bool) -> Op {
        Op::AddColumn {
            table: table.into(),
            column: column.into(),
            ty: ty.into(),
            not_null,
            default,
        }
    }

    #[test]
    fn parses_mysql_statements() {
        assert_eq!(
            parse_statement("alter table `users` add `phone` varchar(255) not null"),
            [add("users", "phone", "varchar", true, false)]
        );
        assert_eq!(
            parse_statement("alter table `users` add `active` tinyint(1) not null default '1'"),
            [add("users", "active", "tinyint", true, true)]
        );
        assert_eq!(
            parse_statement("alter table `users` add `bio` text null, add `age` int not null"),
            [
                add("users", "bio", "text", false, false),
                add("users", "age", "int", true, false)
            ]
        );
        assert_eq!(
            parse_statement("alter table `users` modify `email` varchar(100) not null"),
            [Op::ChangeColumn {
                table: "users".into(),
                column: "email".into(),
                ty: Some("varchar".into()),
                length: Some(100),
                not_null: Some(true),
            }]
        );
        assert_eq!(
            parse_statement("alter table `users` add unique `users_name_unique`(`name`)"),
            [Op::AddUnique {
                table: "users".into(),
                columns: vec!["name".into()]
            }]
        );
        assert_eq!(
            parse_statement("alter table `users` drop `remember_token`, drop `bio`"),
            [
                Op::DropColumn {
                    table: "users".into(),
                    column: "remember_token".into()
                },
                Op::DropColumn {
                    table: "users".into(),
                    column: "bio".into()
                }
            ]
        );
        assert_eq!(
            parse_statement(
                "alter table `orders` add constraint `orders_user_id_foreign` foreign key (`user_id`) references `users` (`id`)"
            ),
            [Op::AddForeign {
                table: "orders".into(),
                column: "user_id".into(),
                parent: "users".into(),
                parent_column: "id".into(),
            }]
        );
        // Index and key work isn't about columns' data.
        assert!(
            parse_statement("alter table `users` add index `users_email_index`(`email`)")
                .is_empty()
        );
        assert!(parse_statement("alter table `users` drop index `users_email_index`").is_empty());
        assert!(parse_statement("alter table `users` drop foreign key `x`").is_empty());
        assert_eq!(
            parse_statement("drop table if exists `old`"),
            [Op::DropTable {
                table: "old".into()
            }]
        );
        assert_eq!(
            parse_statement(
                "create table `t` (`id` bigint unsigned not null auto_increment primary key)"
            ),
            [Op::CreateTable { table: "t".into() }]
        );
    }

    #[test]
    fn parses_postgres_and_sqlite_statements() {
        assert_eq!(
            parse_statement(r#"alter table "users" add column "phone" varchar(255) not null"#),
            [add("users", "phone", "varchar", true, false)]
        );
        assert_eq!(
            parse_statement(r#"alter table "users" add column "id2" bigserial not null"#),
            [add("users", "id2", "bigserial", true, true)]
        );
        assert_eq!(
            parse_statement(
                r#"alter table "users" alter column "email" type varchar(100), alter column "email" set not null"#
            ),
            [
                Op::ChangeColumn {
                    table: "users".into(),
                    column: "email".into(),
                    ty: Some("varchar".into()),
                    length: Some(100),
                    not_null: None,
                },
                Op::ChangeColumn {
                    table: "users".into(),
                    column: "email".into(),
                    ty: None,
                    length: None,
                    not_null: Some(true),
                }
            ]
        );
        assert_eq!(
            parse_statement(
                r#"create unique index "users_name_unique" on "users" ("name", "team_id")"#
            ),
            [Op::AddUnique {
                table: "users".into(),
                columns: vec!["name".into(), "team_id".into()]
            }]
        );
        assert_eq!(
            parse_statement(
                r#"alter table "users" add constraint "users_name_unique" unique ("name")"#
            ),
            [Op::AddUnique {
                table: "users".into(),
                columns: vec!["name".into()]
            }]
        );
        assert_eq!(
            parse_statement(r#"alter table "users" drop column "bio""#),
            [Op::DropColumn {
                table: "users".into(),
                column: "bio".into()
            }]
        );
    }

    /// Facts from a table of answers.
    #[derive(Default)]
    struct Known {
        rows: Vec<(&'static str, i64)>,
        nulls: i64,
        non_nulls: i64,
        duplicates: i64,
        longer: i64,
        orphans: i64,
    }

    impl Facts for Known {
        fn rows(&self, table: &str) -> Option<i64> {
            self.rows.iter().find(|(t, _)| *t == table).map(|(_, n)| *n)
        }
        fn nulls(&self, _: &str, _: &str) -> Option<i64> {
            Some(self.nulls)
        }
        fn non_nulls(&self, _: &str, _: &str) -> Option<i64> {
            Some(self.non_nulls)
        }
        fn duplicates(&self, _: &str, _: &[String]) -> Option<i64> {
            Some(self.duplicates)
        }
        fn longer_than(&self, _: &str, _: &str, _: u32) -> Option<i64> {
            Some(self.longer)
        }
        fn orphans(&self, _: &str, _: &str, _: &str, _: &str) -> Option<i64> {
            Some(self.orphans)
        }
    }

    fn messages(ops: &[Op], engine: Engine, facts: &Known) -> Vec<(Severity, String)> {
        risks(ops, engine, facts)
            .into_iter()
            .map(|r| (r.severity, r.message))
            .collect()
    }

    #[test]
    fn a_not_null_column_without_a_default_on_a_table_with_rows() {
        let facts = Known {
            rows: vec![("orders", 14_200)],
            ..Default::default()
        };
        let phone = [add("orders", "phone", "varchar", true, false)];
        let pg = messages(&phone, Engine::Postgres, &facts);
        assert_eq!(pg[0].0, Severity::Error);
        assert!(pg[0]
            .1
            .starts_with("`orders` has 14 200 rows and none has a `phone`"));
        assert_eq!(
            messages(&phone, Engine::Mysql, &facts)[0].0,
            Severity::Warning
        );
        assert!(messages(&phone, Engine::Mysql, &facts)[0]
            .1
            .contains("sets `phone` to '' in each"));
        let paid = [add("orders", "paid_at", "timestamp", true, false)];
        assert_eq!(messages(&paid, Engine::Mysql, &facts)[0].0, Severity::Error);

        // SQLite refuses it even on an empty table.
        let empty = Known::default();
        assert_eq!(
            messages(&phone, Engine::Sqlite, &empty)[0].0,
            Severity::Error
        );
        // Fine: an empty table, a default, nullable, or a table made here.
        assert!(messages(&phone, Engine::Postgres, &empty).is_empty());
        assert!(messages(
            &[add("orders", "x", "int", true, true)],
            Engine::Postgres,
            &facts
        )
        .is_empty());
        assert!(messages(
            &[add("orders", "x", "int", false, false)],
            Engine::Postgres,
            &facts
        )
        .is_empty());
        let made_here = [
            Op::CreateTable {
                table: "orders".into(),
            },
            add("orders", "phone", "varchar", true, false),
        ];
        assert!(messages(&made_here, Engine::Postgres, &facts).is_empty());
    }

    #[test]
    fn changes_unique_indexes_foreign_keys_and_drops() {
        let change = [Op::ChangeColumn {
            table: "users".into(),
            column: "email".into(),
            ty: Some("varchar".into()),
            length: Some(100),
            not_null: Some(true),
        }];
        let facts = Known {
            rows: vec![("users", 40)],
            nulls: 3,
            longer: 1,
            duplicates: 2,
            orphans: 5,
            non_nulls: 38,
        };
        let m = messages(&change, Engine::Mysql, &facts);
        assert_eq!(m.len(), 3);
        assert!(m[0].1.starts_with("3 rows of `users` have NULL in `email`"));
        assert!(m[1]
            .1
            .starts_with("`email` is longer than 100 characters in 1 row of `users`"));
        assert_eq!(m[2].0, Severity::Info);

        let unique = [Op::AddUnique {
            table: "users".into(),
            columns: vec!["name".into()],
        }];
        assert!(messages(&unique, Engine::Mysql, &facts)[0]
            .1
            .starts_with("2 values of `name` in `users` occur more than once"));

        let fk = [Op::AddForeign {
            table: "users".into(),
            column: "team_id".into(),
            parent: "teams".into(),
            parent_column: "id".into(),
        }];
        assert!(messages(&fk, Engine::Postgres, &facts)[0]
            .1
            .starts_with("`team_id` matches no `teams` row in 5 rows of `users`"));

        let drops = [
            Op::DropColumn {
                table: "users".into(),
                column: "bio".into(),
            },
            Op::DropTable {
                table: "users".into(),
            },
        ];
        let m = messages(&drops, Engine::Mysql, &facts);
        assert_eq!(m[0].0, Severity::Warning);
        assert!(m[0].1.starts_with("Drops `bio` with the values of 38 rows"));
        assert!(m[1].1.starts_with("Drops `users` with its 40 rows"));
    }

    #[test]
    fn reads_the_dry_run_answer() {
        use serde_json::json;
        assert_eq!(
            read_dry_run(&json!({"driver": "mysql", "steps": []})),
            DryRun::NotTransactional {
                driver: "mysql".into()
            }
        );
        assert_eq!(
            read_dry_run(&json!({"driver": "sqlite", "steps": ["up", "down"], "error": null})),
            DryRun::Passed {
                driver: "sqlite".into(),
                steps: vec!["up".into(), "down".into()]
            }
        );
        assert_eq!(
            read_dry_run(&json!({"driver": "pgsql", "steps": ["up"], "step": "down",
                "error": "column \"bio\" does not exist"})),
            DryRun::Failed {
                driver: "pgsql".into(),
                step: "down".into(),
                error: "column \"bio\" does not exist".into()
            }
        );
    }

    #[test]
    fn finds_the_line_an_operation_comes_from() {
        let src = "<?php
return new class extends Migration
{
    public function up(): void
    {
        Schema::table('users', function (Blueprint $table) {
            $table->string('phone');
            $table->unique('name');
            $table->dropColumn('bio');
        });
    }

    public function down(): void
    {
        Schema::table('users', function (Blueprint $table) {
            $table->string('bio');
            $table->dropColumn('phone');
        });
    }
};
";
        assert_eq!(
            line_of(src, &add("users", "phone", "varchar", true, false)),
            Some(6)
        );
        let unique = Op::AddUnique {
            table: "users".into(),
            columns: vec!["name".into()],
        };
        assert_eq!(line_of(src, &unique), Some(7));
        // up()'s dropColumn, not down()'s re-add.
        let drop = Op::DropColumn {
            table: "users".into(),
            column: "bio".into(),
        };
        assert_eq!(line_of(src, &drop), Some(8));
        // Not named in up(): the table's line.
        assert_eq!(
            line_of(src, &add("users", "zzz", "int", true, false)),
            Some(5)
        );
    }
}
