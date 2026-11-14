//! Statement classification: does a statement hand back rows, and can it
//! write?
//!
//! Both questions are answered from the statement's words outside string
//! literals and comments. The read-only answer gates writes to production and
//! read-only connections, so it errs towards "writes": a statement is
//! read-only only when it starts with a reading keyword and no writing keyword
//! appears anywhere in it — which also catches writable CTEs (`WITH d AS
//! (DELETE …) SELECT …`), `SELECT … INTO` and `EXPLAIN ANALYZE <write>`. A
//! quoted identifier is never a keyword, but it can still name a function
//! that writes (`pg_catalog."set_config"(…)`).
//!
//! Engines disagree on several lexical points that matter here. MySQL treats
//! `\` inside a string as an escape and `#` as a line comment, and runs the
//! body of `/*! … */`. PostgreSQL (with
//! standard strings) does none of that, but has dollar-quoted strings
//! (`$$ … $$`, `$tag$ … $tag$`) and block comments that nest. A statement
//! written to look harmless under one reading could hide a write under the
//! other, so it is tokenised both ways and counts as read-only only if both
//! readings agree.

/// A word, a quoted identifier, or one of the punctuation marks the rules
/// care about.
#[derive(Debug, PartialEq)]
enum Tok {
    Word(String),
    /// A `"…"` or `` `…` `` name, lowercased. (To MySQL `"…"` is a string,
    /// but taking it for a name only ever finds more writes.)
    Quoted(String),
    Open,
    Close,
    Eq,
}

#[derive(Clone, Copy)]
struct Dialect {
    /// `\` escapes the next character inside `'…'` / `"…"`.
    backslash_escapes: bool,
    /// `#` starts a line comment.
    hash_comments: bool,
    /// MySQL's block comments: the body of `/*! … */` and `/*+ … */` is
    /// SQL, and the first `*/` ends a comment. Otherwise (PostgreSQL) every
    /// `/* … */` is a comment, and they nest.
    mysql_comments: bool,
    /// `$$ … $$` and `$tag$ … $tag$` are strings.
    dollar_quotes: bool,
}

const MYSQLISH: Dialect = Dialect {
    backslash_escapes: true,
    hash_comments: true,
    mysql_comments: true,
    dollar_quotes: false,
};
const STANDARD: Dialect = Dialect {
    backslash_escapes: false,
    hash_comments: false,
    mysql_comments: false,
    dollar_quotes: true,
};

/// The length of the `$tag$` opening a dollar-quoted string at `i`, if one
/// does: `$`, an optional tag (a name not starting with a digit — `$1` is a
/// parameter), `$`. A `$` inside a name (`a$b`) never gets here: names
/// take it in.
fn dollar_tag(chars: &[char], i: usize) -> Option<usize> {
    let mut j = i + 1;
    if chars.get(j).is_some_and(|c| c.is_alphabetic() || *c == '_') {
        while chars
            .get(j)
            .is_some_and(|c| c.is_alphanumeric() || *c == '_')
        {
            j += 1;
        }
    }
    (chars.get(j) == Some(&'$')).then_some(j + 1 - i)
}

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
                if c != '\'' {
                    let name: String = chars[i + 1..j.min(chars.len())].iter().collect();
                    out.push(Tok::Quoted(name.to_lowercase()));
                }
                i = j + 1;
            }
            '$' if d.dollar_quotes && dollar_tag(&chars, i).is_some() => {
                let len = dollar_tag(&chars, i).unwrap_or(1);
                let tag = &chars[i..i + len];
                let mut j = i + len;
                while j < chars.len() && !chars[j..].starts_with(tag) {
                    j += 1;
                }
                i = j + len;
            }
            // (MySQL wants a space after `--`; both readings take `--x` for a
            // comment anyway, as every other engine does.)
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
            // still SQL), so there only a plain block comment is skipped.
            '/' if next == Some('*')
                && !(d.mysql_comments && matches!(chars.get(i + 2), Some('!') | Some('+'))) =>
            {
                i += 2;
                let mut depth = 1;
                while i < chars.len() {
                    if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                        i += 2;
                        depth -= 1;
                        if depth == 0 || d.mysql_comments {
                            break;
                        }
                    } else if !d.mysql_comments && chars[i] == '/' && chars.get(i + 1) == Some(&'*')
                    {
                        i += 2;
                        depth += 1;
                    } else {
                        i += 1;
                    }
                }
            }
            '(' => {
                out.push(Tok::Open);
                i += 1;
            }
            ')' => {
                out.push(Tok::Close);
                i += 1;
            }
            '=' => {
                out.push(Tok::Eq);
                i += 1;
            }
            c if c.is_alphanumeric() || c == '_' => {
                let start = i;
                while i < chars.len()
                    && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '$')
                {
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

/// The statement's first keyword, skipping leading comments and `(`. A
/// quoted name where the keyword should be is no keyword (`"select"` is a
/// column), so it gives a word that matches none.
fn first_word(toks: &[Tok]) -> Option<&str> {
    toks.iter().find_map(|t| match t {
        Tok::Word(w) => Some(w.as_str()),
        Tok::Quoted(_) => Some("\""),
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

/// Whether the statement is a read the server can be told to stop early: it
/// starts with SELECT, WITH, VALUES or TABLE and only reads, so it can be
/// wrapped as `SELECT * FROM (…) LIMIT n`.
pub(crate) fn limitable(sql: &str) -> bool {
    matches!(
        first_word(&tokens(sql, MYSQLISH)),
        Some("select" | "with" | "values" | "table")
    ) && is_read_only(sql)
}

/// Keywords that make a statement write, wherever they appear.
const WRITE_WORDS: &[&str] = &[
    "insert", "update", "delete", "merge", "upsert", "truncate", "drop", "alter", "create",
    "rename", "grant", "revoke", // `SELECT … INTO new_table` / `INTO OUTFILE`.
    "into",
];

/// PostgreSQL functions that change state from inside a SELECT. Unlike a
/// keyword, a function can be named in quotes (`"set_config"(…)`), so these
/// count quoted too.
const WRITE_FUNCTIONS: &[&str] = &[
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
    let writes = || {
        toks.iter().any(|t| match t {
            Tok::Word(w) => {
                WRITE_WORDS.contains(&w.as_str()) || WRITE_FUNCTIONS.contains(&w.as_str())
            }
            Tok::Quoted(name) => WRITE_FUNCTIONS.contains(&name.as_str()),
            _ => false,
        })
    };
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

fn rollback_in(sql: &str, d: Dialect) -> bool {
    let toks = tokens(sql, d);
    // Each token as a word; a quoted name as `"`, punctuation as nothing.
    let words: Vec<Option<&str>> = toks
        .iter()
        .map(|t| match t {
            Tok::Word(w) => Some(w.as_str()),
            Tok::Quoted(_) => Some("\""),
            _ => None,
        })
        .collect();
    let rest = match words.as_slice() {
        [Some("rollback" | "abort"), Some("work" | "transaction"), rest @ ..]
        | [Some("rollback" | "abort"), rest @ ..] => rest,
        _ => return false,
    };
    match rest {
        [] | [Some("and"), Some("chain")] | [Some("and"), Some("no"), Some("chain")] => true,
        // `TO [SAVEPOINT] name`.
        [Some("to"), Some("savepoint"), Some(_)] => true,
        [Some("to"), Some(name)] => *name != "savepoint",
        _ => false,
    }
}

/// Whether the statement only ends (or goes back within) the transaction:
/// `ROLLBACK`, `ABORT` or `ROLLBACK TO SAVEPOINT`, and nothing else. That
/// discards changes rather than making them, and a session left in a failed
/// transaction can do nothing else, so a read-only connection lets it
/// through. (`ROLLBACK PREPARED` throws away another transaction's work and
/// doesn't count.)
pub(crate) fn is_rollback(sql: &str) -> bool {
    rollback_in(sql, MYSQLISH) && rollback_in(sql, STANDARD)
}

fn destructive_in(sql: &str, d: Dialect) -> Option<&'static str> {
    let toks = tokens(sql, d);
    let word = |i: usize| match toks.get(i) {
        Some(Tok::Word(w)) => Some(w.as_str()),
        _ => None,
    };
    for (i, t) in toks.iter().enumerate() {
        let Tok::Word(w) = t else { continue };
        match w.as_str() {
            // `DROP TABLE`, and `ALTER TABLE … DROP COLUMN` with it.
            "drop" => return Some("DROP"),
            // MySQL also has a `TRUNCATE(x, d)` number function.
            "truncate" if toks.get(i + 1) != Some(&Tok::Open) => return Some("TRUNCATE"),
            "delete" | "update" => {
                // The statement verb only: at the start, after `(` or `)` (a
                // CTE's body, the statement after a CTE) or after EXPLAIN's
                // words — not `FOR UPDATE`, `ON DELETE CASCADE` or
                // `ON DUPLICATE KEY UPDATE`.
                let verb = i == 0
                    || matches!(toks.get(i - 1), Some(Tok::Open | Tok::Close))
                    || matches!(
                        word(i - 1),
                        Some("explain" | "analyze" | "analyse" | "verbose")
                    );
                if !verb {
                    continue;
                }
                // Its own WHERE: at its own depth, before the statement (or
                // the parenthesis it sits in) ends — not one in a subquery.
                let mut depth = 0i32;
                let mut has_where = false;
                for t in &toks[i + 1..] {
                    match t {
                        Tok::Open => depth += 1,
                        Tok::Close => {
                            depth -= 1;
                            if depth < 0 {
                                break;
                            }
                        }
                        Tok::Word(w) if depth == 0 && w == "where" => {
                            has_where = true;
                            break;
                        }
                        _ => {}
                    }
                }
                if !has_where {
                    return Some(if w == "delete" {
                        "DELETE without WHERE"
                    } else {
                        "UPDATE without WHERE"
                    });
                }
            }
            _ => {}
        }
    }
    None
}

/// If the statement destroys data wholesale — DROP (also `ALTER … DROP`),
/// TRUNCATE, or a DELETE or UPDATE with no WHERE of its own — a short reason
/// for the confirmation; otherwise `None`. Comments, strings and quoted names
/// don't count, and as with [`is_read_only`] both readings are taken: either
/// one finding a reason is enough.
pub fn is_destructive(sql: &str) -> Option<&'static str> {
    destructive_in(sql, MYSQLISH).or_else(|| destructive_in(sql, STANDARD))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destruction_is_found_past_comments_and_inside_statements() {
        assert_eq!(is_destructive("-- tidy up\nDROP TABLE users"), Some("DROP"));
        assert_eq!(
            is_destructive("/* nightly */ DELETE FROM orders"),
            Some("DELETE without WHERE")
        );
        assert_eq!(is_destructive("# mysql\ntruncate t"), Some("TRUNCATE"));
        assert_eq!(
            is_destructive("ALTER TABLE users DROP COLUMN email"),
            Some("DROP")
        );
        assert_eq!(
            is_destructive(
                "WITH gone AS (DELETE FROM orders RETURNING id) SELECT count(*) FROM gone"
            ),
            Some("DELETE without WHERE")
        );
        assert_eq!(
            is_destructive("WITH x AS (SELECT 1) UPDATE orders SET total = 0"),
            Some("UPDATE without WHERE")
        );
        assert_eq!(
            is_destructive("EXPLAIN ANALYZE DELETE FROM t"),
            Some("DELETE without WHERE")
        );
        // A WHERE in a subquery isn't the statement's own.
        assert_eq!(
            is_destructive("DELETE FROM t USING (SELECT id FROM u WHERE u.x = 1) s"),
            Some("DELETE without WHERE")
        );
        assert_eq!(
            is_destructive("UPDATE t SET note = 'where'"),
            Some("UPDATE without WHERE")
        );
        // MySQL runs `/*! … */`.
        assert_eq!(is_destructive("/*! DROP TABLE t */"), Some("DROP"));
    }

    #[test]
    fn what_isnt_destruction_stays_quiet() {
        assert_eq!(
            is_destructive("DELETE FROM t WHERE id IN (SELECT id FROM u)"),
            None
        );
        assert_eq!(
            is_destructive("UPDATE t SET a = (SELECT 1) WHERE id = 2"),
            None
        );
        assert_eq!(
            is_destructive("SELECT * FROM t WHERE id = 1 FOR UPDATE"),
            None
        );
        assert_eq!(
            is_destructive(
                "CREATE TABLE c (p_id INT REFERENCES p ON DELETE CASCADE ON UPDATE CASCADE)"
            ),
            None
        );
        assert_eq!(
            is_destructive("INSERT INTO t (id, n) VALUES (1, 2) ON DUPLICATE KEY UPDATE n = 2"),
            None
        );
        assert_eq!(
            is_destructive("INSERT INTO t (id) VALUES (1) ON CONFLICT (id) DO UPDATE SET n = 2"),
            None
        );
        assert_eq!(is_destructive("SELECT TRUNCATE(1.234, 1)"), None);
        assert_eq!(is_destructive("SELECT 'drop table x' AS note"), None);
        assert_eq!(is_destructive("-- DROP TABLE users\nSELECT 1"), None);
        assert_eq!(is_destructive("SELECT \"delete\" FROM t"), None);
    }

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

    #[test]
    fn a_quoted_function_name_still_counts() {
        // Turning PostgreSQL's read-only session off from inside a SELECT.
        for sql in [
            "SELECT pg_catalog.\"set_config\"('default_transaction_read_only', 'off', false)",
            "SELECT \"set_config\"('default_transaction_read_only', 'off', false)",
            "SELECT \"SetVal\"('s', 1)",
            "SELECT `nextval`('s')",
        ] {
            assert!(!is_read_only(sql), "{sql}");
        }
        // A quoted name isn't a keyword, though.
        assert!(is_read_only("SELECT \"into\", `drop` FROM t"));
        assert!(!is_read_only("\"select\" FROM t"));
    }

    #[test]
    fn dollar_quotes_are_strings_to_postgres() {
        // Read as `'…'` strings, the quotes hid the DELETE.
        for sql in [
            "WITH a AS (SELECT $$ ' $$), d AS (DELETE FROM t RETURNING 1) SELECT $$ ' $$",
            "WITH a AS (SELECT $q$ ' $q$), d AS (DELETE FROM t RETURNING 1) SELECT $q$ ' $q$",
        ] {
            assert!(!is_read_only(sql), "{sql}");
            assert_eq!(is_destructive(sql), Some("DELETE without WHERE"), "{sql}");
        }
        // To PostgreSQL what's inside one is a string. (MySQL has no dollar
        // quotes, so the statement as a whole still isn't vouched for.)
        let pg = |sql| read_only_in(sql, STANDARD);
        assert!(pg("SELECT $$delete from t$$"));
        assert!(pg("SELECT $body$ drop table x $body$ AS s"));
        assert!(!is_read_only("SELECT $$delete from t$$"));
        // A tag must match: `$a$` doesn't end at `$b$`.
        assert!(pg("SELECT $a$ $b$ drop $b$ $a$"));
        // `a$b$` is a name and `$1$` no tag, not quotes.
        assert!(!pg("SELECT a$b$ FROM t; DELETE FROM t; SELECT $b$"));
        assert!(!pg("SELECT $1$ FROM t; DELETE FROM t; SELECT $1$"));
    }

    #[test]
    fn postgres_block_comments_nest() {
        // PostgreSQL's comment ends at the second `*/`, so the quote after the
        // first one is inside it and the DELETE runs.
        let sql = "WITH a AS (SELECT 1 /* /* */ ' */), d AS (DELETE FROM t RETURNING 1) SELECT '1'";
        assert!(!is_read_only(sql));
        assert_eq!(is_destructive(sql), Some("DELETE without WHERE"));
        assert!(read_only_in(
            "/* outer /* inner */ still a comment */ SELECT 1",
            STANDARD
        ));
        // `/*! … */` is SQL only to MySQL; to PostgreSQL it is a comment.
        let sql = "WITH a AS (SELECT 1 /*! ' */), d AS (DELETE FROM t RETURNING 1) SELECT '1'";
        assert!(!is_read_only(sql));
        assert_eq!(is_destructive(sql), Some("DELETE without WHERE"));
    }

    #[test]
    fn rollbacks_are_recognised() {
        for sql in [
            "ROLLBACK",
            "rollback;",
            "ROLLBACK WORK",
            "ROLLBACK TRANSACTION AND NO CHAIN",
            "ABORT",
            "ROLLBACK TO SAVEPOINT sp1",
            "ROLLBACK TO sp1",
            "ROLLBACK TRANSACTION TO SAVEPOINT \"Sp 1\"",
            "-- give up\nROLLBACK",
        ] {
            assert!(is_rollback(sql), "{sql}");
        }
        for sql in [
            "ROLLBACK PREPARED 'tx1'",
            "ROLLBACK; DELETE FROM t",
            "ROLLBACK TO SAVEPOINT",
            "SELECT 1",
            "ROLLBACK /* x */ AND DROP",
        ] {
            assert!(!is_rollback(sql), "{sql}");
        }
    }
}
