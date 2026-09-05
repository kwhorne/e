//! Detect runnable project tasks (build/test/scripts) from common manifests
//! and run them in the integrated terminal.

use std::path::Path;

use serde_json::Value;

#[derive(Clone, Debug)]
pub struct Task {
    /// Display label, e.g. `npm: dev` or `cargo test`.
    pub label: String,
    /// The shell command to run.
    pub command: String,
}

impl Task {
    fn new(label: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            command: command.into(),
        }
    }
}

/// The test the caret is in, as a `--filter` pattern: a Pest `it('…')` /
/// `test('…')` description, or a PHPUnit `test_x()` / `#[Test]` method. The
/// nearest declaration at or above the caret's line wins.
pub fn test_at_cursor(text: &str, offset: usize) -> Option<String> {
    let upto = offset.min(text.len());
    let line_end = text[upto..]
        .find('\n')
        .map(|i| upto + i)
        .unwrap_or(text.len());
    let mut found: Option<String> = None;
    let mut attr_test = false;
    for line in text[..line_end].lines() {
        let t = line.trim_start();
        // Pest: it('does x', …) / test('does x', …)
        for open in ["it(", "test(", "it (", "test ("] {
            if let Some(rest) = t.strip_prefix(open) {
                let rest = rest.trim_start();
                if let Some(q) = rest.chars().next().filter(|c| *c == '\'' || *c == '"') {
                    if let Some(end) = rest[1..].find(q) {
                        found = Some(rest[1..1 + end].to_string());
                    }
                }
            }
        }
        // PHPUnit: public function test_x() / #[Test] public function x()
        if let Some(pos) = t.find("function ") {
            let name: String = t[pos + "function ".len()..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() && (attr_test || name.starts_with("test")) {
                found = Some(name);
            }
        }
        attr_test = t.starts_with("#[Test") || t.contains("@test");
    }
    found
}

/// Quote a test name for `--filter`, which both Pest and PHPUnit read as a
/// regular expression.
pub fn filter_pattern(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 8);
    for c in name.chars() {
        if r"\.^$|?*+()[]{}/".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The arguments that narrow a PHP test run to `name` in `file` (a path
/// relative to the root): appended to `test_command`'s runner.
pub fn single_test_args(file: &str, name: &str) -> String {
    let pattern = filter_pattern(name).replace('"', "\\\"");
    format!("{file} --filter=\"{pattern}\"")
}

/// Tasks Grove adds to a Laravel project: its dev processes (Vite, queue
/// worker, whatever the app declares) and a snapshot-first migrate, so a
/// migration that goes wrong is one `grove db restore` from undone.
fn grove_tasks(root: &Path) -> Vec<Task> {
    let mut tasks = vec![
        Task::new("grove: dev start", "grove dev start"),
        Task::new("grove: dev stop", "grove dev stop"),
    ];
    if let Some(engine) = grove_snapshot_engine(root) {
        tasks.push(Task::new(
            "artisan: migrate (snapshot first)",
            format!(
                "grove db snapshot --engine {engine} --note 'before migrate' && php artisan migrate"
            ),
        ));
        tasks.push(Task::new(
            "grove: db snapshot",
            format!("grove db snapshot --engine {engine}"),
        ));
    }
    tasks
}

/// Grove's snapshot engine for the project's database, from `.env`; `None`
/// for SQLite and anything else Grove doesn't snapshot.
fn grove_snapshot_engine(root: &Path) -> Option<&'static str> {
    let env = std::fs::read_to_string(root.join(".env")).ok()?;
    let conn = env
        .lines()
        .find_map(|l| l.trim().strip_prefix("DB_CONNECTION="))?
        .trim()
        .trim_matches(|c| c == '"' || c == '\'');
    match conn {
        "mysql" | "mariadb" => Some("mysql"),
        "pgsql" => Some("postgres"),
        _ => None,
    }
}

fn scripts_from(path: &Path, prefix: &str, runner: &dyn Fn(&str) -> String, out: &mut Vec<Task>) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let Ok(v) = serde_json::from_str::<Value>(&text) else {
        return;
    };
    if let Some(scripts) = v.get("scripts").and_then(|s| s.as_object()) {
        for name in scripts.keys() {
            out.push(Task::new(format!("{prefix}: {name}"), runner(name)));
        }
    }
}

/// Discover tasks available in `root`.
pub fn detect(root: &Path) -> Vec<Task> {
    let mut tasks = Vec::new();

    // Rust / Cargo.
    if root.join("Cargo.toml").exists() {
        for c in ["test", "build", "run", "check", "clippy", "fmt"] {
            tasks.push(Task::new(format!("cargo {c}"), format!("cargo {c}")));
        }
    }

    // Node — pick the package manager from the lockfile.
    let pkg = root.join("package.json");
    if pkg.exists() {
        let runner: Box<dyn Fn(&str) -> String> = if root.join("pnpm-lock.yaml").exists() {
            Box::new(|n: &str| format!("pnpm {n}"))
        } else if root.join("yarn.lock").exists() {
            Box::new(|n: &str| format!("yarn {n}"))
        } else if root.join("bun.lockb").exists() {
            Box::new(|n: &str| format!("bun run {n}"))
        } else {
            Box::new(|n: &str| format!("npm run {n}"))
        };
        let label = if root.join("pnpm-lock.yaml").exists() {
            "pnpm"
        } else if root.join("yarn.lock").exists() {
            "yarn"
        } else if root.join("bun.lockb").exists() {
            "bun"
        } else {
            "npm"
        };
        scripts_from(&pkg, label, &runner, &mut tasks);
    }

    // PHP / Composer.
    scripts_from(
        &root.join("composer.json"),
        "composer",
        &|n: &str| format!("composer run {n}"),
        &mut tasks,
    );

    // Laravel.
    if root.join("artisan").exists() {
        tasks.push(Task::new("artisan: test", "php artisan test"));
        tasks.push(Task::new("artisan: serve", "php artisan serve"));
        tasks.push(Task::new("artisan: migrate", "php artisan migrate"));
        tasks.push(Task::new("artisan: tinker", "php artisan tinker"));
        if crate::grove::available() {
            tasks.extend(grove_tasks(root));
        }
    }
    if root.join("vendor/bin/pest").exists() {
        tasks.push(Task::new("pest", "vendor/bin/pest"));
    } else if root.join("vendor/bin/phpunit").exists() {
        tasks.push(Task::new("phpunit", "vendor/bin/phpunit"));
    }

    // Go.
    if root.join("go.mod").exists() {
        tasks.push(Task::new("go test", "go test ./..."));
        tasks.push(Task::new("go build", "go build ./..."));
    }

    // Makefile targets.
    if let Ok(text) = std::fs::read_to_string(root.join("Makefile")) {
        for line in text.lines() {
            let Some(colon) = line.find(':') else {
                continue;
            };
            let target = &line[..colon];
            let valid = !target.is_empty()
                && !line.starts_with('\t')
                && !line.starts_with(' ')
                && !target.starts_with('.')
                && target
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '_' || c == '-');
            if valid {
                tasks.push(Task::new(
                    format!("make {target}"),
                    format!("make {target}"),
                ));
            }
        }
    }

    tasks
}

/// The most appropriate test command for the project, if any.
pub fn test_command(root: &Path) -> Option<String> {
    if root.join("artisan").exists() {
        Some("php artisan test".to_string())
    } else if root.join("vendor/bin/pest").exists() {
        Some("vendor/bin/pest".to_string())
    } else if root.join("vendor/bin/phpunit").exists() {
        Some("vendor/bin/phpunit".to_string())
    } else if root.join("Cargo.toml").exists() {
        Some("cargo test".to_string())
    } else if root.join("go.mod").exists() {
        Some("go test ./...".to_string())
    } else if root.join("package.json").exists() {
        Some("npm test".to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod test_at_cursor_tests {
    use super::*;

    #[test]
    fn finds_pest_descriptions_and_phpunit_methods() {
        let pest = "<?php\n\nit('creates an order', function () {\n    expect(1)->toBe(1);\n});\n\ntest('sends a receipt (once)', function () {\n    // caret here\n});\n";
        let at = pest.find("caret").unwrap();
        assert_eq!(
            test_at_cursor(pest, at).as_deref(),
            Some("sends a receipt (once)")
        );
        assert_eq!(
            test_at_cursor(pest, pest.find("expect").unwrap()).as_deref(),
            Some("creates an order")
        );

        let unit = "class OrderTest extends TestCase\n{\n    public function test_totals(): void\n    {\n    }\n\n    #[Test]\n    public function it_sends_mail(): void\n    {\n        // caret\n    }\n    public function helper() {}\n}\n";
        assert_eq!(
            test_at_cursor(unit, unit.find("// caret").unwrap()).as_deref(),
            Some("it_sends_mail")
        );
        assert_eq!(
            test_at_cursor(unit, unit.find("test_totals").unwrap()).as_deref(),
            Some("test_totals")
        );
        // A plain helper isn't a test.
        assert_eq!(
            test_at_cursor(unit, unit.find("helper").unwrap()).as_deref(),
            Some("it_sends_mail")
        );
    }

    #[test]
    fn filter_patterns_escape_regex_characters() {
        assert_eq!(
            filter_pattern("sends a receipt (once)"),
            "sends a receipt \\(once\\)"
        );
        assert_eq!(filter_pattern("test_totals"), "test_totals");
    }
}
