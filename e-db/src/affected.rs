//! How many rows a write is about to touch, for the confirmation dialog
//! (DB-703).
//!
//! A count can only be had before the write runs by asking the database
//! which rows the write's `WHERE` picks. That is done for the plain forms,
//! read from the statement's tokens: `UPDATE t … SET … WHERE …`,
//! `DELETE FROM t … WHERE …`, `TRUNCATE t` and `DROP TABLE t`. Joins,
//! `USING`, `UPDATE … FROM` and anything else it can't vouch for go
//! uncounted rather than miscounted. `INSERT … VALUES` counts its own rows
//! and asks nothing.
//!
//! The count runs before the user has said yes, so it must only read: the
//! query built is put to [`crate::is_read_only`] like any other, and a
//! `WHERE` that calls `nextval()` (say) leaves the statement uncounted. It
//! stops at [`COUNT_CAP`] rows, so a count of a huge table is no heavier
//! than a page of it.

use std::ops::Range;

use crate::classify::{spans, Dialect, Tok, MYSQLISH, STANDARD};

/// Counting stops past this many rows ("10000+").
pub const COUNT_CAP: u64 = 10_000;

/// What a write statement is known to touch before it runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Affects {
    /// Read from the statement itself (`INSERT … VALUES`).
    Rows(u64),
    /// Run this read to count them; it returns one number, capped at
    /// `COUNT_CAP + 1`.
    Count(String),
    /// Can't tell.
    Unknown,
}

/// The rows a count query found, as the dialog says it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Counted {
    Exactly(u64),
    /// More than [`COUNT_CAP`].
    Over,
}

impl Counted {
    pub fn from_count(n: u64) -> Counted {
        if n > COUNT_CAP {
            Counted::Over
        } else {
            Counted::Exactly(n)
        }
    }
}

/// Work out how to learn the rows `sql` (one statement) will touch on
/// `engine`.
pub fn affects(engine: &str, sql: &str) -> Affects {
    let d = if matches!(engine, "mysql" | "mariadb") {
        MYSQLISH
    } else {
        STANDARD
    };
    match read(sql, d) {
        Some(Shape::Rows(n)) => Affects::Rows(n),
        Some(Shape::Pick { from, cond, limit }) => {
            let cap = limit.unwrap_or(u64::MAX).min(COUNT_CAP + 1);
            let filter = cond.map(|c| format!(" WHERE {c}")).unwrap_or_default();
            let count = format!(
                "SELECT COUNT(*) FROM (SELECT 1 FROM {from}{filter} LIMIT {cap}) e_affected"
            );
            if crate::is_read_only(&count) {
                Affects::Count(count)
            } else {
                Affects::Unknown
            }
        }
        None => Affects::Unknown,
    }
}

enum Shape {
    Rows(u64),
    /// `SELECT 1 FROM {from} WHERE {cond} LIMIT {limit}` finds the rows.
    Pick {
        from: String,
        cond: Option<String>,
        limit: Option<u64>,
    },
}

/// Words that can't be a table's alias: they start the next clause.
const CLAUSE_WORDS: &[&str] = &[
    "set",
    "where",
    "using",
    "from",
    "join",
    "inner",
    "left",
    "right",
    "full",
    "cross",
    "natural",
    "straight_join",
    "partition",
    "order",
    "limit",
    "returning",
    "on",
    "values",
];

/// Words that can come before the table and would be taken for it.
const MODIFIERS: &[&str] = &["low_priority", "ignore", "quick", "only", "delayed"];

struct Reader<'a> {
    sql: &'a str,
    toks: Vec<(Tok, Range<usize>)>,
}

impl Reader<'_> {
    fn word(&self, i: usize) -> Option<&str> {
        match self.toks.get(i) {
            Some((Tok::Word(w), _)) => Some(w.as_str()),
            _ => None,
        }
    }

    fn is_name(&self, i: usize) -> bool {
        matches!(self.toks.get(i), Some((Tok::Word(_) | Tok::Quoted(_), _)))
    }

    /// Whether only whitespace lies between tokens `a` and `b` (`a < b`).
    fn adjacent(&self, a: usize, b: usize) -> bool {
        match (self.toks.get(a), self.toks.get(b)) {
            (Some((_, x)), Some((_, y))) => self.sql[x.end..y.start].trim().is_empty(),
            _ => false,
        }
    }

    /// A table name (`t`, `s.t`, `` `s`.`t` ``) starting at token `i`: the
    /// token after it, and its text.
    fn name(&self, i: usize) -> Option<(usize, Range<usize>)> {
        if !self.is_name(i) || self.word(i).is_some_and(|w| MODIFIERS.contains(&w)) {
            return None;
        }
        let start = self.toks[i].1.start;
        let mut end = self.toks[i].1.end;
        let mut j = i + 1;
        while self.is_name(j) && self.sql[end..self.toks[j].1.start].trim() == "." {
            end = self.toks[j].1.end;
            j += 1;
        }
        Some((j, start..end))
    }

    /// An optional alias (`[AS] a`) at token `i`, ending where `next` (a
    /// clause word) or the statement does: the token after it.
    fn alias(&self, i: usize) -> Option<usize> {
        let mut j = i;
        if self.word(j) == Some("as") {
            if !self.adjacent(j - 1, j) || !self.is_name(j + 1) {
                return None;
            }
            j += 1;
        }
        if j < self.toks.len() && !self.word(j).is_some_and(|w| CLAUSE_WORDS.contains(&w)) {
            // One name, right after the table (no comma between).
            if !self.is_name(j) || !self.adjacent(j - 1, j) {
                return None;
            }
            j += 1;
        } else if j != i {
            return None; // `AS` and nothing after it
        }
        Some(j)
    }

    /// The first token at or after `from`, outside parentheses, that is
    /// one of `words` (`ORDER` only when `BY` follows).
    fn top_level(&self, from: usize, words: &[&str]) -> Option<usize> {
        let mut depth = 0i32;
        for i in from..self.toks.len() {
            match &self.toks[i].0 {
                Tok::Open => depth += 1,
                Tok::Close => depth -= 1,
                Tok::Word(w)
                    if depth == 0
                        && words.contains(&w.as_str())
                        && (w != "order" || self.word(i + 1) == Some("by")) =>
                {
                    return Some(i);
                }
                _ => {}
            }
        }
        None
    }

    /// The text from token `a` to just before token `b` (or the end).
    fn text(&self, a: usize, b: Option<usize>) -> String {
        let start = self.toks[a].1.start;
        let end = b
            .and_then(|b| self.toks.get(b))
            .map(|t| t.1.start)
            .unwrap_or_else(|| self.toks.last().map(|t| t.1.end).unwrap_or(start));
        self.sql[start..end].trim().to_string()
    }

    /// `WHERE …` from token `i` (if there): the condition, and a `LIMIT n`
    /// after it.
    fn filter(&self, i: usize) -> Option<(Option<String>, Option<u64>)> {
        let ends = ["order", "limit", "returning"];
        let (cond, rest) = match self.word(i) {
            Some("where") => {
                let end = self.top_level(i + 1, &ends);
                if end == Some(i + 1) || i + 1 >= self.toks.len() {
                    return None;
                }
                (Some(self.text(i + 1, end)), end)
            }
            None if i >= self.toks.len() => (None, None),
            Some(w) if ends.contains(&w) => (None, Some(i)),
            _ => return None,
        };
        let limit = match rest {
            Some(r) => match self.top_level(r, &["limit"]) {
                Some(l) => Some(self.word(l + 1)?.parse().ok()?),
                None => None,
            },
            None => None,
        };
        Some((cond, limit))
    }
}

fn read(sql: &str, d: Dialect) -> Option<Shape> {
    let r = Reader {
        sql,
        toks: spans(sql, d),
    };
    match r.word(0)? {
        "delete" => {
            if r.word(1) != Some("from") {
                return None; // `DELETE t1 FROM t1 JOIN …`, `DELETE QUICK …`
            }
            let (after, name) = r.name(2)?;
            let after_alias = r.alias(after)?;
            let (cond, limit) = r.filter(after_alias)?;
            Some(Shape::Pick {
                from: r.sql[name.start..r.toks[after_alias - 1].1.end].to_string(),
                cond,
                limit,
            })
        }
        "update" => {
            let (after, name) = r.name(1)?;
            let after_alias = r.alias(after)?;
            if r.word(after_alias) != Some("set") {
                return None; // `UPDATE a JOIN b …`, `UPDATE a, b …`
            }
            // PostgreSQL's `UPDATE … SET … FROM other` joins.
            let clause = r.top_level(
                after_alias + 1,
                &["where", "from", "order", "limit", "returning"],
            );
            if clause.is_some_and(|c| r.word(c) == Some("from")) {
                return None;
            }
            let (cond, limit) = match clause {
                Some(c) => r.filter(c)?,
                None => (None, None),
            };
            Some(Shape::Pick {
                from: r.sql[name.start..r.toks[after_alias - 1].1.end].to_string(),
                cond,
                limit,
            })
        }
        "truncate" => {
            let at = if r.word(1) == Some("table") { 2 } else { 1 };
            let (after, name) = r.name(at)?;
            let tail_ok = (after..r.toks.len()).all(|i| {
                matches!(
                    r.word(i),
                    Some("cascade" | "restrict" | "restart" | "continue" | "identity")
                )
            });
            (tail_ok && (after == r.toks.len() || r.adjacent(after - 1, after))).then(|| {
                Shape::Pick {
                    from: r.sql[name].to_string(),
                    cond: None,
                    limit: None,
                }
            })
        }
        "drop" => {
            if r.word(1) != Some("table") {
                return None;
            }
            let at = if r.word(2) == Some("if") && r.word(3) == Some("exists") {
                4
            } else {
                2
            };
            let (after, name) = r.name(at)?;
            let tail_ok =
                (after..r.toks.len()).all(|i| matches!(r.word(i), Some("cascade" | "restrict")));
            // One table: anything else between it and the end is a list.
            let one = r.sql[name.end..]
                .trim_start()
                .trim_end_matches(|c: char| c == ';' || c.is_whitespace())
                .chars()
                .all(|c| c.is_alphanumeric() || c.is_whitespace() || c == '_');
            (tail_ok && one).then(|| Shape::Pick {
                from: r.sql[name].to_string(),
                cond: None,
                limit: None,
            })
        }
        "insert" | "replace" => {
            // `INSERT … SET a = 1` (MySQL) is one row.
            if let Some(i) = r.top_level(1, &["values", "value", "select", "set", "with"]) {
                match r.word(i) {
                    Some("set") => return Some(Shape::Rows(1)),
                    Some("values" | "value") => {}
                    _ => return None,
                }
                let mut depth = 0i32;
                let mut rows = 0u64;
                for (t, _) in &r.toks[i + 1..] {
                    match t {
                        Tok::Open => {
                            if depth == 0 {
                                rows += 1;
                            }
                            depth += 1;
                        }
                        Tok::Close => depth -= 1,
                        // `ON DUPLICATE KEY …`, `ON CONFLICT …`, `RETURNING …`.
                        Tok::Word(_) if depth == 0 => break,
                        _ => {}
                    }
                }
                (rows > 0).then_some(Shape::Rows(rows))
            } else {
                None
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count(engine: &str, sql: &str) -> String {
        match affects(engine, sql) {
            Affects::Count(q) => q,
            other => panic!("{sql}: {other:?}"),
        }
    }

    #[test]
    fn an_update_or_delete_counts_the_rows_its_where_picks() {
        assert_eq!(
            count("postgres", "UPDATE users SET name = 'x' WHERE id > 10"),
            "SELECT COUNT(*) FROM (SELECT 1 FROM users WHERE id > 10 LIMIT 10001) e_affected"
        );
        assert_eq!(
            count("mysql", "DELETE FROM `shop`.`orders` WHERE status = 'old'"),
            "SELECT COUNT(*) FROM (SELECT 1 FROM `shop`.`orders` WHERE status = 'old' \
             LIMIT 10001) e_affected"
        );
        // An alias stays, for the WHERE that uses it.
        assert_eq!(
            count(
                "postgres",
                "DELETE FROM orders AS o WHERE o.total = 0 RETURNING o.id"
            ),
            "SELECT COUNT(*) FROM (SELECT 1 FROM orders AS o WHERE o.total = 0 LIMIT 10001) \
             e_affected"
        );
        // A subquery's WHERE and ORDER don't end the condition.
        assert_eq!(
            count(
                "mysql",
                "UPDATE t SET a = (SELECT max(b) FROM u WHERE u.k = t.k) \
                 WHERE t.id IN (SELECT id FROM v ORDER BY id)"
            ),
            "SELECT COUNT(*) FROM (SELECT 1 FROM t WHERE t.id IN (SELECT id FROM v ORDER BY id) \
             LIMIT 10001) e_affected"
        );
    }

    #[test]
    fn a_write_without_where_counts_the_whole_table() {
        assert_eq!(
            count("sqlite", "DELETE FROM logs"),
            "SELECT COUNT(*) FROM (SELECT 1 FROM logs LIMIT 10001) e_affected"
        );
        assert_eq!(
            count("postgres", "UPDATE users SET active = false"),
            "SELECT COUNT(*) FROM (SELECT 1 FROM users LIMIT 10001) e_affected"
        );
        assert_eq!(
            count("postgres", "TRUNCATE TABLE sessions RESTART IDENTITY"),
            "SELECT COUNT(*) FROM (SELECT 1 FROM sessions LIMIT 10001) e_affected"
        );
        assert_eq!(
            count("mysql", "DROP TABLE IF EXISTS `tmp_import`"),
            "SELECT COUNT(*) FROM (SELECT 1 FROM `tmp_import` LIMIT 10001) e_affected"
        );
    }

    #[test]
    fn a_limit_on_the_write_caps_the_count() {
        assert_eq!(
            count(
                "mysql",
                "DELETE FROM jobs WHERE done = 1 ORDER BY id LIMIT 500"
            ),
            "SELECT COUNT(*) FROM (SELECT 1 FROM jobs WHERE done = 1 LIMIT 500) e_affected"
        );
    }

    #[test]
    fn an_insert_counts_its_own_rows() {
        assert_eq!(
            affects(
                "mysql",
                "INSERT INTO t (a, b) VALUES (1, 'x'), (2, '(y)'), (3, NOW())"
            ),
            Affects::Rows(3)
        );
        assert_eq!(
            affects(
                "postgres",
                "INSERT INTO t (a) VALUES (1) ON CONFLICT (a) DO UPDATE SET a = 2"
            ),
            Affects::Rows(1)
        );
        assert_eq!(
            affects("mysql", "INSERT INTO t SET a = 1"),
            Affects::Rows(1)
        );
        assert_eq!(
            affects("postgres", "INSERT INTO t SELECT * FROM u"),
            Affects::Unknown
        );
    }

    #[test]
    fn what_it_cannot_read_is_not_counted() {
        for sql in [
            // Joins and lists of tables.
            "UPDATE a JOIN b ON a.id = b.a_id SET a.x = 1 WHERE b.y = 2",
            "UPDATE a, b SET a.x = b.x WHERE a.id = b.id",
            "UPDATE a SET x = b.x FROM b WHERE a.id = b.id",
            "DELETE a FROM a JOIN b ON a.id = b.id",
            "DELETE FROM a USING b WHERE a.id = b.id",
            "TRUNCATE a, b",
            "DROP TABLE a, b",
            // Modifiers that would pass for the table.
            "UPDATE LOW_PRIORITY t SET a = 1",
            "DELETE FROM ONLY t WHERE a = 1",
            // Not a row write.
            "ALTER TABLE t ADD COLUMN c int",
            "CREATE TABLE t (id int)",
        ] {
            assert_eq!(affects("mysql", sql), Affects::Unknown, "{sql}");
            assert_eq!(affects("postgres", sql), Affects::Unknown, "{sql}");
        }
    }

    #[test]
    fn a_condition_that_writes_is_never_run_to_count() {
        // The count runs before the user says yes.
        assert_eq!(
            affects(
                "postgres",
                "DELETE FROM t WHERE id = nextval('s') OR set_config('a', 'b', false) = 'b'"
            ),
            Affects::Unknown
        );
        assert_eq!(
            affects(
                "postgres",
                "UPDATE t SET a = 1 WHERE id IN (SELECT id FROM (DELETE FROM u RETURNING id) d)"
            ),
            Affects::Unknown
        );
    }

    #[test]
    fn counts_past_the_cap_say_so() {
        assert_eq!(Counted::from_count(3), Counted::Exactly(3));
        assert_eq!(Counted::from_count(COUNT_CAP), Counted::Exactly(COUNT_CAP));
        assert_eq!(Counted::from_count(COUNT_CAP + 1), Counted::Over);
    }
}
