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
            let Some(colon) = line.find(':') else { continue };
            let target = &line[..colon];
            let valid = !target.is_empty()
                && !line.starts_with('\t')
                && !line.starts_with(' ')
                && !target.starts_with('.')
                && target
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '_' || c == '-');
            if valid {
                tasks.push(Task::new(format!("make {target}"), format!("make {target}")));
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
