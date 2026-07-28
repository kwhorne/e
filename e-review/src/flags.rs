//! Automated review flags — a second pair of eyes over an agent's diff.
//!
//! These are deliberately *diff-scoped* inspections: they only look at what the
//! session actually added (or removed), so you get the "wait, why is there a
//! `dd()` in here" signal without running a whole static-analysis pass. Pure and
//! unit-tested.

use crate::{Changeset, FileChange, Hunk};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// Worth knowing, not worth blocking.
    Info,
    /// Probably wrong — look at it.
    Warn,
    /// Don't ship this without a deliberate decision.
    Danger,
}

/// One finding in the changeset.
#[derive(Debug, Clone, PartialEq)]
pub struct Flag {
    pub path: String,
    /// 1-based line in the new file (best effort; 0 for whole-file findings).
    pub line: usize,
    pub severity: Severity,
    /// Stable id, e.g. `"debug-leftover"`.
    pub code: &'static str,
    pub message: String,
}

/// Added lines in a hunk, paired with their line number in the new file.
fn added_lines(h: &Hunk) -> Vec<(usize, &str)> {
    let mut n = h.new_start;
    let mut out = Vec::new();
    for l in &h.lines {
        match l.as_bytes().first() {
            Some(b'+') => {
                out.push((n, &l[1..]));
                n += 1;
            }
            Some(b'-') => {}
            _ => n += 1,
        }
    }
    out
}

fn removed_lines(h: &Hunk) -> Vec<&str> {
    h.lines
        .iter()
        .filter(|l| l.starts_with('-'))
        .map(|l| &l[1..])
        .collect()
}

fn contains_any(hay: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| hay.contains(n))
}

/// True when `line` contains a call to `name(` that isn't the tail of a longer
/// identifier — so `dd(` doesn't match `add(`, and `ray(` doesn't match `array(`.
fn calls(line: &str, name: &str) -> bool {
    let pat = format!("{name}(");
    let mut from = 0;
    while let Some(i) = line[from..].find(&pat) {
        let at = from + i;
        let standalone = at == 0
            || line[..at]
                .chars()
                .next_back()
                .map(|p| !(p.is_alphanumeric() || p == '_'))
                .unwrap_or(true);
        if standalone {
            return true;
        }
        from = at + 1;
    }
    false
}

fn calls_any(line: &str, names: &[&str]) -> bool {
    names.iter().any(|n| calls(line, n))
}

fn contains_any_ci(hay: &str, needles: &[&str]) -> bool {
    let low = hay.to_ascii_lowercase();
    needles.iter().any(|n| low.contains(*n))
}

/// A short, safe excerpt of a line for the message (no secrets echoed in full).
fn excerpt(line: &str) -> String {
    let t = line.trim();
    if t.chars().count() <= 80 {
        t.to_string()
    } else {
        let s: String = t.chars().take(77).collect();
        format!("{s}…")
    }
}

/// True when a line looks like it assigns a long literal to a secret-ish name.
fn looks_like_secret(line: &str) -> bool {
    const NAMES: &[&str] = &[
        "api_key",
        "apikey",
        "secret",
        "password",
        "passwd",
        "token",
        "private_key",
        "client_secret",
    ];
    let low = line.to_ascii_lowercase();
    if !NAMES.iter().any(|n| low.contains(*n)) {
        return false;
    }
    // Require an assignment to a quoted literal of some length.
    let Some(eq) = line.find(['=', ':']) else {
        return false;
    };
    let rhs = &line[eq + 1..];
    let quoted = rhs
        .split(['\'', '"'])
        .nth(1)
        .map(|v| v.len() >= 12 && !v.contains("${") && !v.starts_with("env("))
        .unwrap_or(false);
    quoted
}

/// Inspect one file's diff.
pub fn scan_file(f: &FileChange) -> Vec<Flag> {
    let mut out = Vec::new();
    let mut push = |line: usize, severity: Severity, code: &'static str, message: String| {
        out.push(Flag {
            path: f.path.clone(),
            line,
            severity,
            code,
            message,
        })
    };

    let is_migration = f.risk_reason == "migration";
    let is_env = f.risk_reason == "environment";
    let is_test = f.risk_reason == "test";

    for h in &f.hunks {
        for (n, line) in added_lines(h) {
            // Debug statements left behind.
            if calls_any(
                line,
                &[
                    "dd",
                    "dump",
                    "var_dump",
                    "print_r",
                    "console.log",
                    "dbg!",
                    "ray",
                ],
            ) || line.contains("xdebug_break")
            {
                push(
                    n,
                    Severity::Warn,
                    "debug-leftover",
                    format!("Debug statement left in: {}", excerpt(line)),
                );
            }
            // Hardcoded credentials.
            if looks_like_secret(line) {
                push(
                    n,
                    Severity::Danger,
                    "secret",
                    "Possible hardcoded credential — move it to the environment".to_string(),
                );
            }
            // Raw SQL built with interpolation.
            if calls_any(line, &["DB::raw", "whereRaw", "selectRaw", "DB::statement"])
                && contains_any(line, &["$", "\" +", "' +", ".concat("])
            {
                push(
                    n,
                    Severity::Danger,
                    "sql-injection",
                    format!("Raw SQL with interpolated input: {}", excerpt(line)),
                );
            }
            // Destructive schema changes.
            if is_migration
                && (contains_any(
                    line,
                    &["dropColumn", "dropIfExists", "->drop(", "dropAllTables"],
                ) || calls(line, "truncate"))
            {
                push(
                    n,
                    Severity::Danger,
                    "destructive-migration",
                    format!("Destructive migration step: {}", excerpt(line)),
                );
            }
            // Environment values.
            if is_env && line.contains('=') && !line.trim_start().starts_with('#') {
                push(
                    n,
                    Severity::Danger,
                    "env-changed",
                    format!(
                        "Environment value changed: {}",
                        line.split('=').next().unwrap_or("").trim()
                    ),
                );
            }
            // Focused/skipped tests sneaking in.
            if contains_any(
                line,
                &[
                    "it.only(",
                    "describe.only(",
                    "test.only(",
                    "->skip(",
                    "#[ignore]",
                    "markTestSkipped",
                ],
            ) {
                push(
                    n,
                    Severity::Warn,
                    "test-skipped",
                    format!("Test skipped or focused: {}", excerpt(line)),
                );
            }
            // Unfinished work.
            if contains_any(line, &["TODO", "FIXME", "HACK", "XXX"]) {
                push(
                    n,
                    Severity::Info,
                    "todo-added",
                    format!("Unfinished marker added: {}", excerpt(line)),
                );
            }
            // Shelling out / dynamic evaluation.
            if calls_any(line, &["shell_exec", "passthru", "eval", "proc_open"])
                || line.contains("unsafe {")
            {
                push(
                    n,
                    Severity::Danger,
                    "unsafe",
                    format!("Dynamic execution or unsafe block: {}", excerpt(line)),
                );
            }
            // Verification turned off.
            if contains_any_ci(
                line,
                &[
                    "rejectunauthorized: false",
                    "verify=false",
                    "verify_peer\" => false",
                    "--no-verify",
                    "ssl_verify_none",
                    "chmod 777",
                    "0777",
                ],
            ) {
                push(
                    n,
                    Severity::Warn,
                    "verification-disabled",
                    format!("Safety check weakened: {}", excerpt(line)),
                );
            }
            // Sleeps in production code.
            if !is_test && (calls_any(line, &["sleep", "usleep"]) || line.contains("thread::sleep"))
            {
                push(
                    n,
                    Severity::Info,
                    "sleep",
                    format!("Blocking sleep added: {}", excerpt(line)),
                );
            }
        }

        // Authorization checks that disappeared.
        for line in removed_lines(h) {
            if (calls_any(line, &["authorize", "middleware"])
                || contains_any(line, &["Gate::", "->can(", "can:"]))
                && contains_any_ci(line, &["auth", "can", "gate", "policy", "authorize"])
            {
                push(
                    h.new_start,
                    Severity::Danger,
                    "auth-removed",
                    format!("Authorization check removed: {}", excerpt(line)),
                );
            }
        }
    }

    // Whole-file shapes.
    if is_test && f.removed > f.added && f.removed.saturating_sub(f.added) >= 5 {
        out.push(Flag {
            path: f.path.clone(),
            line: 0,
            severity: Severity::Warn,
            code: "tests-removed",
            message: format!(
                "Net {} test lines removed — make sure coverage didn't drop",
                f.removed - f.added
            ),
        });
    }
    if f.removed >= 100 && f.removed > f.added.saturating_mul(3) {
        out.push(Flag {
            path: f.path.clone(),
            line: 0,
            severity: Severity::Warn,
            code: "large-deletion",
            message: format!("Large deletion: −{} vs +{}", f.removed, f.added),
        });
    }

    out
}

/// Inspect the whole changeset, most severe first.
pub fn scan_changeset(cs: &Changeset) -> Vec<Flag> {
    let mut flags: Vec<Flag> = cs.files.iter().flat_map(scan_file).collect();
    flags.sort_by(|a, b| {
        b.severity
            .cmp(&a.severity)
            .then(a.path.cmp(&b.path))
            .then(a.line.cmp(&b.line))
    });
    flags
}

/// `(danger, warn, info)` counts.
pub fn counts(flags: &[Flag]) -> (usize, usize, usize) {
    let mut c = (0, 0, 0);
    for f in flags {
        match f.severity {
            Severity::Danger => c.0 += 1,
            Severity::Warn => c.1 += 1,
            Severity::Info => c.2 += 1,
        }
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::changeset_from_diff;

    fn cs_of(path: &str, added: &[&str]) -> Changeset {
        let mut d = format!("diff --git a/{path} b/{path}\nindex 1..2 100644\n--- a/{path}\n+++ b/{path}\n@@ -10,1 +10,{} @@\n", added.len());
        for l in added {
            d.push('+');
            d.push_str(l);
            d.push('\n');
        }
        changeset_from_diff(&d)
    }

    fn codes(cs: &Changeset) -> Vec<&'static str> {
        scan_changeset(cs).into_iter().map(|f| f.code).collect()
    }

    #[test]
    fn flags_debug_leftovers_with_line_numbers() {
        let cs = cs_of(
            "app/Http/Controllers/X.php",
            &["    $x = 1;", "    dd($x);"],
        );
        let flags = scan_changeset(&cs);
        assert_eq!(flags.len(), 1);
        assert_eq!(flags[0].code, "debug-leftover");
        assert_eq!(flags[0].severity, Severity::Warn);
        // Second added line of a hunk starting at 10.
        assert_eq!(flags[0].line, 11);
    }

    #[test]
    fn flags_hardcoded_secret_but_not_env_lookup() {
        let bad = cs_of(
            "app/Services/Api.php",
            &["$apiKey = 'sk-live-abcdef123456';"],
        );
        assert!(codes(&bad).contains(&"secret"));

        let ok = cs_of("app/Services/Api.php", &["$apiKey = env('API_KEY');"]);
        assert!(!codes(&ok).contains(&"secret"), "{:?}", codes(&ok));
    }

    #[test]
    fn flags_interpolated_raw_sql() {
        let cs = cs_of(
            "app/Repo.php",
            &["DB::raw(\"select * from t where id = $id\")"],
        );
        assert!(codes(&cs).contains(&"sql-injection"));
    }

    #[test]
    fn flags_destructive_migration_only_in_migrations() {
        let mig = cs_of(
            "database/migrations/2026_x.php",
            &["$table->dropColumn('email');"],
        );
        assert!(codes(&mig).contains(&"destructive-migration"));

        let other = cs_of("app/Models/User.php", &["$table->dropColumn('email');"]);
        assert!(!codes(&other).contains(&"destructive-migration"));
    }

    #[test]
    fn flags_env_and_skipped_tests_and_todos() {
        let env = cs_of(".env", &["DB_PASSWORD=hunter2hunter2"]);
        assert!(codes(&env).contains(&"env-changed"));

        let t = cs_of(
            "tests/Feature/XTest.php",
            &["it.only('works', fn () => 1);"],
        );
        assert!(codes(&t).contains(&"test-skipped"));

        let todo = cs_of("src/lib.rs", &["// TODO: handle errors"]);
        assert!(codes(&todo).contains(&"todo-added"));
    }

    #[test]
    fn flags_removed_authorization() {
        let d = "\
diff --git a/app/Http/Controllers/PostController.php b/app/Http/Controllers/PostController.php
index 1..2 100644
--- a/app/Http/Controllers/PostController.php
+++ b/app/Http/Controllers/PostController.php
@@ -20,3 +20,2 @@
-        $this->authorize('update', $post);
         return $post;
";
        let cs = changeset_from_diff(d);
        let flags = scan_changeset(&cs);
        assert_eq!(flags[0].code, "auth-removed");
        assert_eq!(flags[0].severity, Severity::Danger);
    }

    #[test]
    fn sorts_danger_first_and_counts() {
        let d = "\
diff --git a/.env b/.env
index 1..2 100644
--- a/.env
+++ b/.env
@@ -1,1 +1,2 @@
+APP_SECRET=abcdefghijklmnop
diff --git a/src/x.rs b/src/x.rs
index 1..2 100644
--- a/src/x.rs
+++ b/src/x.rs
@@ -1,1 +1,2 @@
+// TODO: later
";
        let cs = changeset_from_diff(d);
        let flags = scan_changeset(&cs);
        assert_eq!(flags[0].severity, Severity::Danger);
        let (danger, warn, info) = counts(&flags);
        assert!(danger >= 1);
        assert_eq!(info, 1);
        let _ = warn;
    }

    #[test]
    fn call_detection_respects_word_boundaries() {
        // `add(` must not trigger `dd(`, `array(` must not trigger `ray(`,
        // `usleep(` must not double-report as `sleep(`.
        let cs = cs_of(
            "src/lib.rs",
            &[
                "pub fn add(a: i32) -> i32 { a + 1 }",
                "$x = array(1, 2);",
                "let odd = is_odd(3);",
            ],
        );
        assert!(scan_changeset(&cs).is_empty(), "{:?}", codes(&cs));

        // A real call still flags, including through `$this->`.
        let real = cs_of("app/X.php", &["$this->dd($x);"]);
        assert!(codes(&real).contains(&"debug-leftover"));
    }

    #[test]
    fn clean_diff_has_no_flags() {
        let cs = cs_of("src/lib.rs", &["pub fn add(a: i32) -> i32 { a + 1 }"]);
        assert!(scan_changeset(&cs).is_empty());
    }
}
