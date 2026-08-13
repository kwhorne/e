//! Laravel Pint and PHPStan — the two tools a modern Laravel project's CI
//! enforces, and which the editor previously knew nothing about.
//!
//! Both are invoked from `vendor/bin`, so a project that doesn't use them gets
//! nothing: no configuration, no error, no behaviour change. That matters more
//! than it sounds — every one of these hooks runs on save.
//!
//! The parsing lives here and is pure, because a mis-parsed PHPStan report puts
//! a squiggle on the wrong line, which is worse than showing nothing.

use std::path::{Path, PathBuf};

use lsp_types::{Diagnostic, DiagnosticSeverity, Position, Range};

/// A tool installed in the project's `vendor/bin`.
fn vendor_bin(root: &Path, name: &str) -> Option<PathBuf> {
    let p = root.join("vendor").join("bin").join(name);
    p.is_file().then_some(p)
}

/// `vendor/bin/pint`, if this project uses Pint.
pub fn pint_binary(root: &Path) -> Option<PathBuf> {
    vendor_bin(root, "pint")
}

/// `vendor/bin/phpstan`, if this project uses PHPStan (or Larastan, which ships
/// the same binary).
pub fn phpstan_binary(root: &Path) -> Option<PathBuf> {
    vendor_bin(root, "phpstan")
}

/// Whether the project has a PHPStan config, which is what decides the rule
/// level and paths. Without one PHPStan refuses to run, so there's no point
/// spawning it.
pub fn has_phpstan_config(root: &Path) -> bool {
    ["phpstan.neon", "phpstan.neon.dist", "phpstan.dist.neon"]
        .iter()
        .any(|f| root.join(f).is_file())
}

/// One diagnostic PHPStan reported, before it is attached to a buffer.
#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    /// Absolute path as PHPStan reported it.
    pub file: String,
    /// 1-based, as in the report.
    pub line: u32,
    pub message: String,
    /// PHPStan's rule identifier, e.g. `variable.undefined`. Shown as the
    /// diagnostic's code so a reader can look it up or baseline it.
    pub identifier: Option<String>,
}

/// What a PHPStan run produced.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Report {
    pub findings: Vec<Finding>,
    /// Errors that aren't about a file — a broken config, an unreadable path.
    /// Surfaced rather than swallowed, since silence would look like "clean".
    pub errors: Vec<String>,
}

/// Parse `phpstan analyse --error-format=json`.
///
/// The shape is `{"files": {path: {"messages": [...]}}, "errors": [...]}`. Both
/// halves matter: a config error leaves `files` empty, and reporting that as a
/// clean run would be a lie of exactly the kind an editor should never tell.
pub fn parse_phpstan(stdout: &str) -> Option<Report> {
    // PHPStan can print warnings before the JSON; start at the object.
    let start = stdout.find('{')?;
    let v: serde_json::Value = serde_json::from_str(stdout[start..].trim()).ok()?;

    let mut report = Report::default();
    if let Some(files) = v.get("files").and_then(|f| f.as_object()) {
        for (file, entry) in files {
            let Some(messages) = entry.get("messages").and_then(|m| m.as_array()) else {
                continue;
            };
            for m in messages {
                let Some(message) = m.get("message").and_then(|s| s.as_str()) else {
                    continue;
                };
                report.findings.push(Finding {
                    file: file.clone(),
                    // A null line means the whole file; anchor it at the top
                    // rather than dropping the finding.
                    line: m.get("line").and_then(|l| l.as_u64()).unwrap_or(1).max(1) as u32,
                    message: message.to_string(),
                    identifier: m
                        .get("identifier")
                        .and_then(|s| s.as_str())
                        .filter(|s| !s.is_empty())
                        .map(str::to_string),
                });
            }
        }
    }
    if let Some(errors) = v.get("errors").and_then(|e| e.as_array()) {
        report.errors = errors
            .iter()
            .filter_map(|e| e.as_str().map(str::to_string))
            .collect();
    }
    // Stable order: PHPStan hands back a map, whose iteration order is not.
    report
        .findings
        .sort_by(|a, b| a.file.cmp(&b.file).then(a.line.cmp(&b.line)));
    Some(report)
}

/// The findings for one file, as editor diagnostics.
///
/// PHPStan reports a line but no column, so each one covers the whole line —
/// guessing a span would put the squiggle under the wrong token.
pub fn diagnostics_for(report: &Report, file: &Path) -> Vec<Diagnostic> {
    report
        .findings
        .iter()
        .filter(|f| Path::new(&f.file) == file)
        .map(|f| {
            let line = f.line.saturating_sub(1);
            Diagnostic {
                range: Range {
                    start: Position { line, character: 0 },
                    end: Position {
                        line,
                        character: u32::MAX,
                    },
                },
                severity: Some(DiagnosticSeverity::WARNING),
                code: f.identifier.clone().map(lsp_types::NumberOrString::String),
                source: Some("phpstan".into()),
                message: f.message.clone(),
                ..Default::default()
            }
        })
        .collect()
}

/// Format `text` with the project's Pint.
///
/// Pint rewrites files in place and resolves `pint.json` from the file's
/// project, so the text goes to a temporary file *beside the original* rather
/// than in the system temp directory — a preset configured for `app/` has to
/// see the file as being in `app/`.
///
/// Returns `None` when Pint isn't installed, fails, or changes nothing, so the
/// caller can fall through to the language server's formatter.
pub fn pint_format(root: &Path, file: &Path, text: &str) -> Option<String> {
    let pint = pint_binary(root)?;
    let dir = file.parent().unwrap_or(root);
    let tmp = dir.join(format!(".e-pint-{}.php", std::process::id()));
    std::fs::write(&tmp, text).ok()?;

    let status = std::process::Command::new(&pint)
        .arg("--quiet")
        .arg(&tmp)
        .current_dir(root)
        .output();

    let formatted = match status {
        Ok(o) if o.status.success() => std::fs::read_to_string(&tmp).ok(),
        Ok(o) => {
            eprintln!(
                "e: pint failed: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            );
            None
        }
        Err(e) => {
            eprintln!("e: could not run pint: {e}");
            None
        }
    };
    let _ = std::fs::remove_file(&tmp);
    formatted.filter(|f| f != text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_normal_report_is_parsed() {
        let json = r#"{
          "totals": {"errors": 0, "file_errors": 2},
          "files": {
            "/app/Http/Controllers/OrderController.php": {
              "errors": 2,
              "messages": [
                {"message": "Undefined variable: $order", "line": 42, "ignorable": true,
                 "identifier": "variable.undefined"},
                {"message": "Method has no return type", "line": 12, "ignorable": true,
                 "identifier": "missingType.return"}
              ]
            }
          },
          "errors": []
        }"#;
        let r = parse_phpstan(json).unwrap();
        assert_eq!(r.findings.len(), 2);
        // Sorted by line, so the output is stable between runs.
        assert_eq!(r.findings[0].line, 12);
        assert_eq!(r.findings[1].line, 42);
        assert_eq!(
            r.findings[1].identifier.as_deref(),
            Some("variable.undefined")
        );
        assert!(r.errors.is_empty());
    }

    #[test]
    fn a_config_error_is_not_reported_as_a_clean_run() {
        // `files` is empty and the real problem is in `errors`. Treating this as
        // "no findings" would tell the user their code is fine when PHPStan
        // never looked at it.
        let json = r#"{"totals":{"errors":1,"file_errors":0},"files":{},
          "errors":["Path /app/NotThere was not found"]}"#;
        let r = parse_phpstan(json).unwrap();
        assert!(r.findings.is_empty());
        assert_eq!(r.errors, ["Path /app/NotThere was not found"]);
    }

    #[test]
    fn a_file_level_finding_without_a_line_is_kept() {
        let json = r#"{"files":{"/app/X.php":{"messages":[
            {"message":"Ignored error pattern was not matched","line":null}]}},"errors":[]}"#;
        let r = parse_phpstan(json).unwrap();
        assert_eq!(r.findings.len(), 1, "dropping it would hide a real problem");
        assert_eq!(r.findings[0].line, 1);
        assert_eq!(r.findings[0].identifier, None);
    }

    #[test]
    fn noise_before_the_json_is_tolerated() {
        // PHPStan prints deprecation notices to stdout ahead of the report.
        let json = "PHP Deprecated: something\n{\"files\":{},\"errors\":[]}";
        assert_eq!(parse_phpstan(json), Some(Report::default()));
    }

    #[test]
    fn junk_is_rejected_rather_than_guessed_at() {
        assert_eq!(parse_phpstan(""), None);
        assert_eq!(parse_phpstan("command not found"), None);
        assert_eq!(parse_phpstan("{not json"), None);
    }

    #[test]
    fn diagnostics_cover_the_whole_line_and_carry_the_rule() {
        let r = parse_phpstan(
            r#"{"files":{"/app/X.php":{"messages":[
                {"message":"Undefined variable: $x","line":7,"identifier":"variable.undefined"}]}},
               "errors":[]}"#,
        )
        .unwrap();
        let d = diagnostics_for(&r, Path::new("/app/X.php"));
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].range.start.line, 6, "0-based for the editor");
        assert_eq!(d[0].range.start.character, 0);
        assert_eq!(d[0].source.as_deref(), Some("phpstan"));
        assert_eq!(
            d[0].code,
            Some(lsp_types::NumberOrString::String(
                "variable.undefined".into()
            ))
        );
    }

    #[test]
    fn only_the_asked_for_file_comes_back() {
        let r = parse_phpstan(
            r#"{"files":{
                "/app/A.php":{"messages":[{"message":"a","line":1}]},
                "/app/B.php":{"messages":[{"message":"b","line":1}]}},
               "errors":[]}"#,
        )
        .unwrap();
        assert_eq!(diagnostics_for(&r, Path::new("/app/A.php")).len(), 1);
        assert_eq!(diagnostics_for(&r, Path::new("/app/B.php"))[0].message, "b");
        assert!(diagnostics_for(&r, Path::new("/app/C.php")).is_empty());
    }

    #[test]
    fn a_project_without_the_tools_reports_nothing() {
        let dir = std::env::temp_dir().join(format!("e-phptools-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(pint_binary(&dir).is_none());
        assert!(phpstan_binary(&dir).is_none());
        assert!(!has_phpstan_config(&dir));
        // And formatting falls through rather than mangling the buffer.
        assert_eq!(pint_format(&dir, &dir.join("a.php"), "<?php\n"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_phpstan_config_is_detected_under_any_of_its_names() {
        for name in ["phpstan.neon", "phpstan.neon.dist", "phpstan.dist.neon"] {
            let dir = std::env::temp_dir().join(format!("e-neon-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(name), "parameters:\n  level: 5\n").unwrap();
            assert!(has_phpstan_config(&dir), "{name}");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

/// Against real binaries in a real project. Opt-in, because it needs a Laravel
/// project with `vendor/bin/pint` and `vendor/bin/phpstan` installed.
///
/// ```sh
/// E_PHP_PROJECT=/path/to/app cargo test -p e-app live_php -- --ignored --nocapture
/// ```
#[cfg(test)]
mod live_php {
    use super::*;

    fn project() -> Option<PathBuf> {
        std::env::var("E_PHP_PROJECT").ok().map(PathBuf::from)
    }

    #[test]
    #[ignore]
    fn phpstan_output_parses_and_lands_on_the_right_line() {
        let Some(root) = project() else { return };
        let bin = phpstan_binary(&root).expect("vendor/bin/phpstan");
        let file = root.join("app/Http/Controllers/BadController.php");
        assert!(file.is_file(), "fixture missing: {}", file.display());

        let out = std::process::Command::new(&bin)
            .args(["analyse", "--error-format=json", "--no-progress"])
            .arg(&file)
            .current_dir(&root)
            .output()
            .expect("run phpstan");
        let text = String::from_utf8_lossy(&out.stdout);
        let report = parse_phpstan(&text).expect("parse real phpstan output");
        println!("findings: {:#?}", report.findings);

        let diags = diagnostics_for(&report, &file);
        assert_eq!(diags.len(), 1, "expected the undefined variable");
        assert!(diags[0].message.contains("undefinedVariable"));
        assert_eq!(diags[0].range.start.line, 4, "line 5, zero-based");
        assert_eq!(
            diags[0].code,
            Some(lsp_types::NumberOrString::String(
                "variable.undefined".into()
            ))
        );
    }

    #[test]
    #[ignore]
    fn pint_formats_and_leaves_no_temporary_behind() {
        let Some(root) = project() else { return };
        assert!(pint_binary(&root).is_some(), "vendor/bin/pint");
        let target = root.join("app/Http/Controllers/BadController.php");

        let ugly = "<?php\nnamespace App;\nclass  Foo   {\npublic function bar(){return 1;}\n}\n";
        let formatted = pint_format(&root, &target, ugly).expect("pint should change this");
        println!("---\n{formatted}---");
        assert!(formatted.contains("class Foo"), "{formatted}");
        assert_ne!(formatted, ugly);

        // Already-formatted input must come back as None, so the caller falls
        // through instead of writing an identical edit into the undo tree.
        assert_eq!(pint_format(&root, &target, &formatted), None);

        // The temporary file is the risk here: it is written into the user's
        // source tree, so it must never survive.
        let leftovers: Vec<_> = std::fs::read_dir(target.parent().unwrap())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with(".e-pint-"))
            .collect();
        assert!(leftovers.is_empty(), "left behind: {leftovers:?}");
    }
}
