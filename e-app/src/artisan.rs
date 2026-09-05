//! Artisan commands, as the application itself lists them.
//!
//! `php artisan list --format=json` is the source of truth: it includes the
//! project's own commands and every package's, with their arguments and
//! usage. Parsed once per session (pure parser, tested); run through the
//! integrated terminal so the output is visible and interactive prompts work.

use std::path::Path;

use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtisanCmd {
    pub name: String,
    pub description: String,
    /// The first usage line, e.g. `make:model [-m|--migration] [--] <name>`.
    pub usage: String,
    /// Names of the arguments that must be given.
    pub required_args: Vec<String>,
}

impl ArtisanCmd {
    /// A hint for the argument line: the usage minus the command name.
    pub fn args_hint(&self) -> String {
        self.usage
            .strip_prefix(&self.name)
            .unwrap_or(&self.usage)
            .trim()
            .to_string()
    }
}

/// Commands nobody runs from an editor palette.
const HIDDEN: &[&str] = &["help", "list", "completion", "_complete"];

/// Parse `php artisan list --format=json`.
pub fn parse_list(json: &Value) -> Vec<ArtisanCmd> {
    json.get("commands")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|c| {
                    let name = c.get("name")?.as_str()?.to_string();
                    if HIDDEN.contains(&name.as_str()) || name.starts_with('_') {
                        return None;
                    }
                    if c.get("hidden").and_then(Value::as_bool) == Some(true) {
                        return None;
                    }
                    let usage = c
                        .get("usage")
                        .and_then(Value::as_array)
                        .and_then(|u| u.first())
                        .and_then(Value::as_str)
                        .unwrap_or(&name)
                        .to_string();
                    let required_args = c
                        .pointer("/definition/arguments")
                        .and_then(Value::as_object)
                        .map(|args| {
                            args.iter()
                                .filter(|(_, a)| {
                                    a.get("is_required").and_then(Value::as_bool) == Some(true)
                                })
                                .map(|(n, _)| n.clone())
                                .collect()
                        })
                        .unwrap_or_default();
                    Some(ArtisanCmd {
                        name,
                        description: c
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        usage,
                        required_args,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Ask the application for its commands. Blocking (boots the app); run off
/// the UI thread.
pub fn list(root: &Path) -> Result<Vec<ArtisanCmd>, String> {
    if !root.join("artisan").is_file() {
        return Err(format!(
            "{} has no `artisan` file — not a Laravel project",
            root.display()
        ));
    }
    let out = std::process::Command::new("php")
        .args(["-d", "error_reporting=0", "-d", "display_errors=0"])
        .args(["artisan", "list", "--format=json"])
        .current_dir(root)
        .output()
        .map_err(|e| {
            format!(
                "couldn't run php: {e} (PATH is {})",
                std::env::var("PATH").unwrap_or_default()
            )
        })?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stdout = String::from_utf8_lossy(&out.stdout);
        let why = stderr
            .lines()
            .chain(stdout.lines())
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("no output");
        return Err(format!("php artisan list failed: {why}"));
    }
    serde_json::from_slice::<Value>(&out.stdout)
        .map(|v| parse_list(&v))
        .map_err(|e| format!("couldn't read artisan's command list: {e}"))
}

/// Rank commands for a query: name prefix first, then name contains, then
/// description contains. Empty query keeps the app's order with `make:` first.
pub fn filter(cmds: &[ArtisanCmd], query: &str) -> Vec<ArtisanCmd> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        let (mut make, rest): (Vec<_>, Vec<_>) = cmds
            .iter()
            .cloned()
            .partition(|c| c.name.starts_with("make:"));
        make.extend(rest);
        return make;
    }
    let mut scored: Vec<(u8, &ArtisanCmd)> = cmds
        .iter()
        .filter_map(|c| {
            let name = c.name.to_lowercase();
            if name.starts_with(&q) {
                Some((0, c))
            } else if name.contains(&q) {
                Some((1, c))
            } else if c.description.to_lowercase().contains(&q) {
                Some((2, c))
            } else {
                None
            }
        })
        .collect();
    scored.sort_by_key(|(score, _)| *score);
    scored.into_iter().map(|(_, c)| c.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn listing() -> Value {
        json!({ "commands": [
            { "name": "help", "description": "Display help", "usage": ["help [<command_name>]"] },
            { "name": "make:model", "description": "Create a new Eloquent model class",
              "usage": ["make:model [-a|--all] [-m|--migration] [--] <name>"],
              "definition": { "arguments": { "name": { "is_required": true } }, "options": {} } },
            { "name": "migrate", "description": "Run the database migrations",
              "usage": ["migrate [--seed] [--force]"],
              "definition": { "arguments": {}, "options": {} } },
            { "name": "_complete", "description": "Internal", "usage": ["_complete"] },
            { "name": "secret:thing", "description": "x", "usage": ["secret:thing"], "hidden": true }
        ]})
    }

    #[test]
    fn parses_and_hides_the_noise() {
        let cmds = parse_list(&listing());
        let names: Vec<&str> = cmds.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["make:model", "migrate"]);
        assert_eq!(cmds[0].required_args, vec!["name"]);
        assert_eq!(
            cmds[0].args_hint(),
            "[-a|--all] [-m|--migration] [--] <name>"
        );
        assert!(cmds[1].required_args.is_empty());
    }

    #[test]
    fn filters_by_name_then_description() {
        let cmds = parse_list(&listing());
        let f = filter(&cmds, "mig");
        assert_eq!(f[0].name, "migrate"); // prefix beats `make:model`'s --migration
        let f = filter(&cmds, "eloquent");
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].name, "make:model");
        // Empty query: make: commands first.
        assert_eq!(filter(&cmds, "")[0].name, "make:model");
    }
}
