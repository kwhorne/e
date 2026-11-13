//! Statement classification: does a statement hand back rows, and can it
//! write?
//!
//! Both questions are answered from the statement's words outside string
//! literals, quoted identifiers and comments. The read-only answer gates writes
//! to production and read-only connections, so it errs towards "writes": a
//! statement is read-only only when it starts with a reading keyword and no
//! writing keyword appears anywhere in it — which also catches writable CTEs
//! (`WITH d AS (DELETE …) SELECT …`), `SELECT … INTO` and `EXPLAIN ANALYZE
//! <write>`.
//!
//! Engines disagree on two lexical points that matter here: MySQL treats `\`
//! inside a string as an escape and `#` as a line comment, PostgreSQL (with
//! standard strings) does neither. A statement written to look harmless under
//! one reading could hide a write under the other, so it is tokenised both ways
//! and counts as read-only only if both readings agree.

/// A word or one of the punctuation marks the rules care about.
#[derive(Debug, PartialEq)]
enum Tok {
    Word(String),
    Open,
    Eq,
}

#[derive(Clone, Copy)]
struct Dialect {
    /// `\` escapes the next character inside `'…'` / `"…"`.
    backslash_escapes: bool,
    /// `#` starts a line comment.
    hash_comments: bool,
}

const MYSQLISH: Dialect = Dialect {
    backslash_escapes: true,
    hash_comments: true,
};
const STANDARD: Dialect = Dialect {
    backslash_escapes: false,
    hash_comments: false,
};

fn tokens(sql: &str, d: Dialect) -> Vec<Tok> {
    let chars: Vec<char> = sql.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        match c {
            '\'' | '"' | '`' => {
                // Skip to the matching quote; a doubled quote stays inside.
                let mut j = i + 1;
                while j < chars.len() {
                    if d.backslash_escapes && c != '`' && chars[j] == '\\' {
                        j += 2;
                        continue;
                    }
                    if chars[j] == c {
                        if chars.get(j + 1) == Some(&c) {
                            j += 2;
                            continue;
                        }
                        break;
                    }
                    j += 1;
                }
                i = j + 1;
            }
            '-' if next == Some('-') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '#' if d.hash_comments => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            // MySQL runs the body of `/*! … */` (and hints in `/*+ … */` are
            // still SQL), so only a plain block comment is skipped.
            '/' if next == Some('*') && !matches!(chars.get(i + 2), Some('!') | Some('+')) => {
                i += 2;
                while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                    i += 1;
                }
                i += 2;
            }
            '(' => {
                out.push(Tok::Open);
                i += 1;
            }
            '=' => {
                out.push(Tok::Eq);
                i += 1;
            }
            c if c.is_alphanumeric() || c == '_' => {
                let start = i;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                out.push(Tok::Word(
                    chars[start..i].iter().collect::<String>().to_lowercase(),
                ));
            }
            _ => i += 1,
        }
    }
    out
}

/// The statement's first keyword, skipping leading comments and `(`.
fn first_word(toks: &[Tok]) -> Option<&str> {
    toks.iter().find_map(|t| match t {
        Tok::Word(w) => Some(w.as_str()),
        _ => None,
    })
}

/// Whether the statement hands back a result set (and so should be run as a
/// query rather than an execute). Leading comments don't count.
pub(crate) fn returns_rows(sql: &str) -> bool {
    matches!(
        first_word(&tokens(sql, MYSQLISH)),
        Some("select" | "show" | "pragma" | "explain" | "describe" | "desc" | "with")
    )
}

/// Keywords that make a statement write, wherever they appear.
const WRITE_WORDS: &[&str] = &[
    "insert",
    "update",
    "delete",
    "merge",
    "upsert",
    "truncate",
    "drop",
    "alter",
    "create",
    "rename",
    "grant",
    "revoke",
    // `SELECT … INTO new_table` / `INTO OUTFILE`.
    "into",
    // PostgreSQL functions that change state from inside a SELECT.
    "setval",
    "nextval",
    "set_config",
    "pg_terminate_backend",
    "pg_cancel_backend",
    "pg_reload_conf",
    "pg_switch_wal",
    "pg_promote",
    "lo_import",
    "lo_export",
    "lo_unlink",
    "dblink_exec",
];

/// SQLite pragmas that take an argument yet only read.
const READING_PRAGMAS: &[&str] = &[
    "table_info",
    "table_xinfo",
    "index_list",
    "index_info",
    "index_xinfo",
    "foreign_key_list",
    "foreign_key_check",
    "integrity_check",
    "quick_check",
    "table_list",
];

fn read_only_in(sql: &str, d: Dialect) -> bool {
    let toks = tokens(sql, d);
    let words = || {
        toks.iter().filter_map(|t| match t {
            Tok::Word(w) => Some(w.as_str()),
            _ => None,
        })
    };
    let writes = || words().any(|w| WRITE_WORDS.contains(&w));
    match first_word(&toks) {
        Some("select" | "with" | "values" | "table" | "show" | "describe" | "desc") => !writes(),
        // A plain EXPLAIN only plans; EXPLAIN ANALYZE runs the statement.
        Some("explain") => !(words().any(|w| w == "analyze" || w == "analyse") && writes()),
        Some("pragma") => {
            if toks.contains(&Tok::Eq) {
                return false;
            }
            if !toks.contains(&Tok::Open) {
                return true; // `PRAGMA user_version` reads
            }
            // `PRAGMA [schema.]name(arg)`: the name is the word before `(`.
            let open = toks.iter().position(|t| *t == Tok::Open).unwrap_or(0);
            matches!(
                toks[..open].last(),
                Some(Tok::Word(w)) if READING_PRAGMAS.contains(&w.as_str())
            )
        }
        _ => false,
    }
}

/// Whether the statement only reads. Conservative: anything it can't vouch for
/// counts as a write.
pub fn is_read_only(sql: &str) -> bool {
    read_only_in(sql, MYSQLISH) && read_only_in(sql, STANDARD)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_reads_are_read_only() {
        for sql in [
            "SELECT * FROM users",
            "  select 1",
            "(SELECT 1) UNION (SELECT 2)",
            "SHOW TABLES",
            "DESCRIBE users",
            "desc\nusers",
            "WITH t AS (SELECT 1) SELECT * FROM t",
            "EXPLAIN SELECT * FROM users",
            "EXPLAIN DELETE FROM users",
            "EXPLAIN ANALYZE SELECT * FROM users",
            "PRAGMA user_version",
            "PRAGMA table_info(\"users\")",
            "SELECT replace(name, 'a', 'b') FROM users",
            "SELECT 'delete from users' AS s",
            "SELECT \"update\" FROM t",
            "SELECT 1 /* drop table x */",
        ] {
            assert!(is_read_only(sql), "{sql}");
        }
    }

    #[test]
    fn writes_are_not_read_only() {
        for sql in [
            "DELETE FROM users",
            "update users set a = 1",
            "INSERT INTO t VALUES (1)",
            "DROP TABLE t",
            "SET SESSION TRANSACTION READ WRITE",
            "BEGIN",
            "CALL p()",
            "COPY t FROM '/tmp/x'",
            "WITH d AS (DELETE FROM users RETURNING *) SELECT * FROM d",
            "EXPLAIN ANALYZE DELETE FROM users",
            "EXPLAIN (ANALYZE, BUFFERS) UPDATE users SET a = 1",
            "SELECT * INTO backup FROM users",
            "SELECT * FROM users INTO OUTFILE '/tmp/u'",
            "SELECT setval('users_id_seq', 1)",
            "SELECT * FROM users FOR UPDATE",
            "PRAGMA user_version = 3",
            "PRAGMA journal_mode(WAL)",
            "SELECT 1 /*! , (SELECT 1 INTO @x) */",
        ] {
            assert!(!is_read_only(sql), "{sql}");
        }
    }

    #[test]
    fn leading_comments_dont_hide_the_verb() {
        assert!(is_read_only("-- list them\nSELECT * FROM users"));
        assert!(is_read_only("/* hi */ SELECT 1"));
        assert!(!is_read_only("-- harmless\nDELETE FROM users"));
        assert!(returns_rows("-- list them\nSELECT * FROM users"));
        assert!(returns_rows("/* hi */\nshow tables"));
        assert!(!returns_rows("-- x\nUPDATE t SET a = 1"));
    }

    #[test]
    fn a_dialect_trick_cannot_hide_a_write() {
        // PostgreSQL ends the string at `\'`; MySQL doesn't. Under the MySQL
        // reading the DELETE looks quoted, but Postgres would run it.
        let sql = "WITH a AS (SELECT 'x\\'), d AS (DELETE FROM t RETURNING 1) SELECT ''";
        assert!(!is_read_only(sql));
        // `#` is a comment to MySQL, an operator to Postgres.
        let sql = "WITH a AS (SELECT 1 # 1), d AS (DELETE FROM t RETURNING 1) SELECT 1";
        assert!(!is_read_only(sql));
    }
}
