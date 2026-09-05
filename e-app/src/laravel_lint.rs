//! Laravel lints `e` can do from the project data it already scrapes — the
//! checks PhpStorm can't make without a plugin, and laravel-lsp makes only when
//! it is installed (when it is, these stay off; see `AppState::refresh_lint`).
//!
//! - `view('x')` / `@include` / `@extends` whose view file doesn't exist.
//! - `route('name')` that isn't defined, or that needs parameters it isn't given.
//! - `__('file.key')` whose key isn't in `lang/` (sentence keys are fine: JSON
//!   translations fall back to the key itself).
//! - `.env` missing keys that `.env.example` declares.
//!
//! Pure functions over text; columns are UTF-8 bytes like every diagnostic the
//! editor draws.

use std::path::{Path, PathBuf};

use lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Position, Range, TextEdit};

use e_core::language::Language;

use crate::laravel::{helper_calls, required_params, Helper, LaravelData};

const SOURCE: &str = "laravel";

/// Byte offset → LSP position, with the line starts computed once.
struct Lines {
    starts: Vec<usize>,
}

impl Lines {
    fn of(text: &str) -> Self {
        let starts = std::iter::once(0)
            .chain(text.match_indices('\n').map(|(i, _)| i + 1))
            .collect();
        Self { starts }
    }

    fn position(&self, off: usize) -> Position {
        let line = self.starts.partition_point(|&s| s <= off).saturating_sub(1);
        Position {
            line: line as u32,
            character: (off - self.starts[line]) as u32,
        }
    }

    fn range(&self, start: usize, end: usize) -> Range {
        Range {
            start: self.position(start),
            end: self.position(end.max(start)),
        }
    }
}

fn diag(range: Range, severity: DiagnosticSeverity, message: String) -> Diagnostic {
    Diagnostic {
        range,
        severity: Some(severity),
        source: Some(SOURCE.to_string()),
        message,
        ..Default::default()
    }
}

/// A diagnostic with a machine-readable `code` a fix can act on.
fn coded(mut d: Diagnostic, code: String) -> Diagnostic {
    d.code = Some(NumberOrString::String(code));
    d
}

/// Lint a PHP or Blade file against the project's routes, views and translations.
pub fn lint(text: &str, language: Language, data: &LaravelData) -> Vec<Diagnostic> {
    if !matches!(language, Language::Php | Language::Blade) {
        return Vec::new();
    }
    let lines = Lines::of(text);
    let mut out = Vec::new();
    for call in helper_calls(text) {
        let range = lines.range(call.start, call.end);
        match call.helper {
            Helper::View => {
                // Namespaced (`package::view`) views live in vendor packages we
                // don't scan; leave them alone.
                if call.token.contains("::") || data.views.is_empty() {
                    continue;
                }
                if !data.views.iter().any(|v| v.name == call.token) {
                    out.push(coded(
                        diag(
                            range,
                            DiagnosticSeverity::ERROR,
                            format!(
                                "View `{}` not found (resources/views/{}.blade.php)",
                                call.token,
                                call.token.replace('.', "/")
                            ),
                        ),
                        format!("missing-view:{}", call.token),
                    ));
                }
            }
            Helper::Route => {
                if data.routes.is_empty() {
                    continue;
                }
                match data.routes.iter().find(|r| r.name == call.token) {
                    None => out.push(diag(
                        range,
                        DiagnosticSeverity::ERROR,
                        format!("Route `{}` is not defined", call.token),
                    )),
                    Some(r) => {
                        let params = required_params(&r.uri);
                        if !params.is_empty() && !call.more_args {
                            out.push(diag(
                                range,
                                DiagnosticSeverity::WARNING,
                                format!(
                                    "Route `{}` needs {} parameter{}: {}",
                                    call.token,
                                    params.len(),
                                    if params.len() == 1 { "" } else { "s" },
                                    params
                                        .iter()
                                        .map(|p| format!("{{{p}}}"))
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                ),
                            ));
                        }
                    }
                }
            }
            Helper::Trans => {
                // Only dotted file keys are checkable; a sentence is a JSON key
                // that falls back to itself, and `pkg::key` lives elsewhere.
                let t = &call.token;
                if data.translations.is_empty()
                    || !t.contains('.')
                    || t.contains(char::is_whitespace)
                    || t.contains("::")
                {
                    continue;
                }
                if !data.translations.iter().any(|e| e.key == *t) {
                    out.push(coded(
                        diag(
                            range,
                            DiagnosticSeverity::WARNING,
                            format!("Translation key `{t}` not found in lang/"),
                        ),
                        format!("missing-trans:{t}"),
                    ));
                }
            }
            Helper::Config | Helper::Env | Helper::Component => {}
        }
    }
    out
}

/// A fix for one of our diagnostics: create or edit a file other than the one
/// being edited.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fix {
    pub title: String,
    pub path: PathBuf,
    pub edits: Vec<TextEdit>,
}

fn at(line: u32, character: u32) -> Position {
    Position { line, character }
}

/// Fixes for the diagnostics under the caret: create the missing view file,
/// or add the missing translation key to its `lang/<locale>/<file>.php`.
/// `read` supplies a file's current text (`None` when it doesn't exist), so
/// this stays pure and testable.
pub fn fixes(
    diags: &[Diagnostic],
    root: &Path,
    locale: &str,
    read: &dyn Fn(&Path) -> Option<String>,
) -> Vec<Fix> {
    let mut out = Vec::new();
    for d in diags {
        let Some(NumberOrString::String(code)) = &d.code else {
            continue;
        };
        if let Some(view) = code.strip_prefix("missing-view:") {
            let rel: PathBuf = view.split('.').collect();
            let path = root
                .join("resources/views")
                .join(rel)
                .with_extension("blade.php");
            let shown = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .display()
                .to_string();
            out.push(Fix {
                title: format!("Create {shown}"),
                path,
                edits: vec![TextEdit {
                    range: Range {
                        start: at(0, 0),
                        end: at(0, 0),
                    },
                    new_text: "<div>\n    \n</div>\n".into(),
                }],
            });
        } else if let Some(key) = code.strip_prefix("missing-trans:") {
            // Only `file.key` (one level) maps to a line in a file; deeper keys
            // would need a nested array we'd rather not guess at.
            let Some((file, name)) = key.split_once('.') else {
                continue;
            };
            if name.contains('.') || file.is_empty() || name.is_empty() {
                continue;
            }
            let lang_dir = if root.join("lang").is_dir() || !root.join("resources/lang").is_dir() {
                root.join("lang")
            } else {
                root.join("resources/lang")
            };
            let path = lang_dir.join(locale).join(format!("{file}.php"));
            let shown = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .display()
                .to_string();
            let line = format!("    '{name}' => '',\n");
            match read(&path) {
                Some(text) => {
                    // Insert before the closing `];`.
                    let Some(close) = text.rfind("];") else {
                        continue;
                    };
                    let line_no = text[..close].matches('\n').count() as u32;
                    let col =
                        (close - text[..close].rfind('\n').map(|i| i + 1).unwrap_or(0)) as u32;
                    out.push(Fix {
                        title: format!("Add `{name}` to {shown}"),
                        path,
                        edits: vec![TextEdit {
                            range: Range {
                                start: at(line_no, col),
                                end: at(line_no, col),
                            },
                            new_text: line,
                        }],
                    });
                }
                None => out.push(Fix {
                    title: format!("Create {shown} with `{name}`"),
                    path,
                    edits: vec![TextEdit {
                        range: Range {
                            start: at(0, 0),
                            end: at(0, 0),
                        },
                        new_text: format!("<?php\n\nreturn [\n{line}];\n"),
                    }],
                }),
            }
        }
    }
    out
}

/// Keys `.env.example` declares that `.env` doesn't set. Reported at the top
/// of `.env`, one per key, so a fresh clone or a teammate's new key can't go
/// unnoticed until something reads it as null.
pub fn lint_env(env_text: &str, example_text: &str) -> Vec<Diagnostic> {
    fn keys(text: &str) -> Vec<&str> {
        text.lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .filter_map(|l| l.split_once('=').map(|(k, _)| k.trim()))
            .filter(|k| !k.is_empty())
            .collect()
    }
    let have = keys(env_text);
    let first_line_len = env_text.lines().next().map(str::len).unwrap_or(0) as u32;
    keys(example_text)
        .into_iter()
        .filter(|k| !have.contains(k))
        .map(|k| {
            diag(
                Range {
                    start: Position {
                        line: 0,
                        character: 0,
                    },
                    end: Position {
                        line: 0,
                        character: first_line_len,
                    },
                },
                DiagnosticSeverity::WARNING,
                format!("`{k}` is in .env.example but not set here"),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::laravel::{RouteInfo, TransEntry, ViewInfo};
    use std::path::PathBuf;

    fn data() -> LaravelData {
        LaravelData {
            routes: vec![
                RouteInfo {
                    name: "home".into(),
                    uri: "/".into(),
                    methods: "GET".into(),
                    action: "HomeController@index".into(),
                    middleware: "web".into(),
                },
                RouteInfo {
                    name: "users.show".into(),
                    uri: "users/{user}".into(),
                    methods: "GET".into(),
                    action: "UserController@show".into(),
                    middleware: "web,auth".into(),
                },
            ],
            views: vec![ViewInfo {
                name: "orders.index".into(),
                path: PathBuf::from("/p/resources/views/orders/index.blade.php"),
            }],
            translations: vec![TransEntry {
                key: "auth.failed".into(),
                value: "Feil".into(),
                file: PathBuf::from("/p/lang/nb/auth.php"),
            }],
            ..Default::default()
        }
    }

    fn messages(d: &[Diagnostic]) -> Vec<String> {
        d.iter().map(|x| x.message.clone()).collect()
    }

    #[test]
    fn flags_missing_view_route_and_translation() {
        let src =
            "<?php\n// Håndter\nreturn view('orders.missing') . route('nope') . __('auth.nope');";
        let d = lint(src, Language::Php, &data());
        assert_eq!(
            messages(&d),
            vec![
                "View `orders.missing` not found (resources/views/orders/missing.blade.php)",
                "Route `nope` is not defined",
                "Translation key `auth.nope` not found in lang/",
            ]
        );
        // Byte columns, on line 2, pointing at the literal.
        let line = src.lines().nth(2).unwrap();
        assert_eq!(d[0].range.start.line, 2);
        assert_eq!(
            d[0].range.start.character as usize,
            line.find("orders.missing").unwrap()
        );
        assert_eq!(d[0].severity, Some(DiagnosticSeverity::ERROR));
    }

    #[test]
    fn a_route_needing_parameters_is_flagged_only_without_them() {
        let d = lint("route('users.show');", Language::Php, &data());
        assert_eq!(
            messages(&d),
            vec!["Route `users.show` needs 1 parameter: {user}"]
        );
        assert_eq!(d[0].severity, Some(DiagnosticSeverity::WARNING));
        assert!(lint("route('users.show', $user);", Language::Php, &data()).is_empty());
        assert!(lint("route('home');", Language::Php, &data()).is_empty());
    }

    #[test]
    fn known_things_and_uncheckable_things_pass() {
        let src = "view('orders.index'); __('auth.failed'); __('Welcome back'); \
                   __('pkg::x.y'); view('pkg::page'); view('a.' . $b); @include('orders.index')";
        assert!(lint(src, Language::Blade, &data()).is_empty());
    }

    #[test]
    fn nothing_is_flagged_without_data_to_check_against() {
        // A project whose scrape hasn't produced views yet mustn't paint every
        // view() red.
        let src = "view('x'); route('y'); __('a.b');";
        assert!(lint(src, Language::Php, &LaravelData::default()).is_empty());
        assert!(lint(src, Language::Rust, &data()).is_empty());
    }

    #[test]
    fn env_example_keys_missing_from_env() {
        let env = "APP_NAME=e\nAPP_KEY=abc\n# DB_HOST=commented\n";
        let example = "APP_NAME=\nAPP_KEY=\nDB_HOST=127.0.0.1\nMAIL_MAILER=smtp\n";
        let d = lint_env(env, example);
        assert_eq!(
            messages(&d),
            vec![
                "`DB_HOST` is in .env.example but not set here",
                "`MAIL_MAILER` is in .env.example but not set here",
            ]
        );
        assert_eq!(d[0].range.start.line, 0);
    }

    #[test]
    fn fixes_create_the_view_and_add_the_key() {
        let src = "view('orders.missing'); __('auth.nope'); __('deep.a.b');";
        let d = lint(src, Language::Php, &data());
        let root = Path::new("/p");
        let read = |p: &Path| -> Option<String> {
            (p == Path::new("/p/lang/nb/auth.php"))
                .then(|| "<?php\n\nreturn [\n    'failed' => 'Feil',\n];\n".to_string())
        };
        let f = fixes(&d, root, "nb", &read);
        assert_eq!(f.len(), 2, "{f:?}");
        assert_eq!(
            f[0].title,
            "Create resources/views/orders/missing.blade.php"
        );
        assert_eq!(
            f[0].path,
            PathBuf::from("/p/resources/views/orders/missing.blade.php")
        );
        assert_eq!(f[1].title, "Add `nope` to lang/nb/auth.php");
        // Inserted on the `];` line, at its start.
        assert_eq!(f[1].edits[0].range.start, at(4, 0));
        assert_eq!(f[1].edits[0].new_text, "    'nope' => '',\n");
        // A file that doesn't exist yet is created around the key.
        let none = |_: &Path| -> Option<String> { None };
        let f = fixes(&d, root, "nb", &none);
        assert!(f[1].title.starts_with("Create lang/nb/auth.php"));
        assert!(f[1].edits[0]
            .new_text
            .contains("return [\n    'nope' => '',\n];"));
    }
}
