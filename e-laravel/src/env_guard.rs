//! `env()` belongs in `config/`. Once a project runs `php artisan
//! config:cache` — every deploy should — Laravel no longer loads `.env`, and
//! every `env('…')` outside the config files returns `null`: the controller
//! that read `env('STRIPE_WEBHOOK_SECRET')` works on every laptop and fails in
//! production.
//!
//! This flags such a call, and its fix reads the value through `config()`
//! instead: the config key that already reads that variable when there is one
//! (`env('APP_URL')` → `config('app.url')`), else a new entry in
//! `config/services.php` (`STRIPE_WEBHOOK_SECRET` →
//! `services.stripe.webhook_secret`) — and the variable goes into
//! `.env.example` when it's missing there, so the next clone knows about it.
//!
//! Pure functions over text; columns are UTF-8 bytes, like every diagnostic
//! the editor draws.

use std::path::{Path, PathBuf};

use lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Position, Range, TextEdit};

/// The diagnostic code's prefix; the variable follows it.
pub const CODE: &str = "env-outside-config:";

/// One `env('KEY', default)` call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnvCall {
    /// The variable: `STRIPE_WEBHOOK_SECRET`.
    pub key: String,
    /// The default as written, when there is one: `'stripe'`, `false`.
    pub default: Option<String>,
    /// Byte range of the whole call, `env(` to its `)`.
    pub start: usize,
    pub end: usize,
}

/// Whether `env()` is fine in the file at `rel` (relative to the project):
/// config files read it — the project's and a local package's — tests and
/// dependencies are not deployed code, and a hidden directory (`.ai/`, an
/// agent's worktree) is someone else's.
pub fn allowed_in(rel: &Path) -> bool {
    let hidden = rel
        .components()
        .next()
        .is_some_and(|c| c.as_os_str().to_string_lossy().starts_with('.'));
    hidden
        || rel.components().any(|c| {
            ["config", "tests", "vendor", "storage", "node_modules"]
                .iter()
                .any(|dir| c.as_os_str() == *dir)
        })
}

/// The stretches of a Blade template that are PHP: `{{ … }}`, `{!! … !!}`,
/// `@php … @endphp`, `<?php … ?>` and a directive's `(…)`. The rest is
/// markup — an `env('X')` there is text, a documentation page's example.
fn blade_code(text: &str) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    for (open, close) in [
        ("{{", "}}"),
        ("{!!", "!!}"),
        ("@php", "@endphp"),
        ("<?php", "?>"),
    ] {
        for (at, _) in text.match_indices(open) {
            if open == "{{" && text[at..].starts_with("{{--") {
                continue;
            }
            if let Some(end) = text[at..].find(close) {
                out.push(at..at + end);
            }
        }
    }
    let bytes = text.as_bytes();
    for (at, _) in text.match_indices('@') {
        let name_len = text[at + 1..]
            .find(|c: char| !c.is_ascii_alphabetic())
            .unwrap_or(0);
        let mut paren = at + 1 + name_len;
        // `@if (…)` as well as `@if(…)`.
        while bytes.get(paren) == Some(&b' ') {
            paren += 1;
        }
        if name_len > 0 && bytes.get(paren) == Some(&b'(') {
            if let Some(end) = closing_paren(text, paren + 1) {
                out.push(paren..end);
            }
        }
    }
    out
}

/// The `env()` calls in `text` whose first argument is a plain string — not
/// Blade's `@env('local')`, a method called `env`, or one in a comment.
pub fn env_calls(text: &str) -> Vec<EnvCall> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    for (at, _) in text.match_indices("env(") {
        if let Some(&p) = at.checked_sub(1).and_then(|i| bytes.get(i)) {
            if p.is_ascii_alphanumeric() || matches!(p, b'_' | b'@' | b'$' | b'>' | b':' | b'\\') {
                continue;
            }
        }
        let line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
        let before = &text[line_start..at];
        let trimmed = before.trim_start();
        if before.contains("//") || trimmed.starts_with('*') || trimmed.starts_with('#') {
            continue;
        }
        let open = at + "env(".len();
        let Some(close) = closing_paren(text, open) else {
            continue;
        };
        let args = text[open..close].trim();
        let Some(q) = args.chars().next().filter(|c| *c == '\'' || *c == '"') else {
            continue;
        };
        let Some(key_len) = args[1..].find(q) else {
            continue;
        };
        let key = &args[1..1 + key_len];
        if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        let rest = args[1 + key_len + 1..].trim_start();
        let default = match rest.strip_prefix(',') {
            Some(d) if !d.trim().is_empty() => Some(d.trim().to_string()),
            Some(_) => None,
            None if rest.is_empty() => None,
            None => continue,
        };
        out.push(EnvCall {
            key: key.to_string(),
            default,
            start: at,
            end: close + 1,
        });
    }
    out
}

/// The `)` that closes the paren opened just before `from`, past strings
/// and nested parens.
fn closing_paren(text: &str, from: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (i, c) in text[from..].char_indices() {
        if let Some(q) = quote {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '\'' | '"' => quote = Some(c),
            '(' => depth += 1,
            ')' if depth == 0 => return Some(from + i),
            ')' => depth -= 1,
            '\n' if depth == 0 && text[from..from + i].trim().is_empty() => return None,
            ';' | '{' | '}' => return None,
            _ => {}
        }
    }
    None
}

/// Byte offset → LSP position (UTF-8 columns).
fn position(text: &str, off: usize) -> Position {
    let before = &text[..off];
    let line = before.matches('\n').count();
    let col = off - before.rfind('\n').map_or(0, |i| i + 1);
    Position::new(line as u32, col as u32)
}

/// A warning on each `env()` call in `text`, the file at `rel`, unless
/// `env()` is fine there.
pub fn lint(rel: &Path, text: &str) -> Vec<Diagnostic> {
    if allowed_in(rel) {
        return Vec::new();
    }
    let code = rel
        .to_string_lossy()
        .ends_with(".blade.php")
        .then(|| blade_code(text));
    env_calls(text)
        .into_iter()
        .filter(|call| {
            code.as_ref()
                .is_none_or(|code| code.iter().any(|r| r.contains(&call.start)))
        })
        .map(|call| Diagnostic {
            range: Range::new(position(text, call.start), position(text, call.end)),
            severity: Some(DiagnosticSeverity::WARNING),
            source: Some("laravel".to_string()),
            code: Some(NumberOrString::String(format!("{CODE}{}", call.key))),
            message: format!(
                "env('{}') is null once the config is cached (`php artisan config:cache`) — \
                 read it with config()",
                call.key
            ),
            ..Default::default()
        })
        .collect()
}

/// What a config file's arrays hold, as the scanner walks it.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Item {
    /// `env('KEY')` as the value at `path` (keys from the file's top).
    Env { path: Vec<String>, key: String },
    /// An array at `path`, from its `[` to its `]`.
    Array {
        path: Vec<String>,
        open: usize,
        close: usize,
    },
}

/// Walk a config file (`return [ 'key' => …, ]`): which `env()` each key
/// reads, and where each keyed array opens and closes. A value inside a list
/// (an array without keys) has no path and isn't reported.
fn scan(text: &str) -> Vec<Item> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    // One entry per open array: its key (None for the top one or a list),
    // and where it opened.
    let mut stack: Vec<(Option<String>, usize)> = Vec::new();
    let mut pending: Option<String> = None;
    let path_of =
        |stack: &[(Option<String>, usize)], last: Option<&String>| -> Option<Vec<String>> {
            let mut path: Vec<String> = stack
                .iter()
                .skip(1)
                .map(|(k, _)| k.clone())
                .collect::<Option<_>>()?;
            path.push(last?.clone());
            Some(path)
        };
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        match c {
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                i = text[i..].find('\n').map_or(bytes.len(), |n| i + n);
                continue;
            }
            b'#' => {
                i = text[i..].find('\n').map_or(bytes.len(), |n| i + n);
                continue;
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i = text[i + 2..]
                    .find("*/")
                    .map_or(bytes.len(), |n| i + 2 + n + 2);
                continue;
            }
            b'\'' | b'"' => {
                let mut j = i + 1;
                while j < bytes.len() && bytes[j] != c {
                    if bytes[j] == b'\\' {
                        j += 1;
                    }
                    j += 1;
                }
                let s = &text[i + 1..j.min(bytes.len())];
                let after = text.get(j + 1..).unwrap_or("").trim_start();
                if after.starts_with("=>") {
                    pending = Some(s.to_string());
                }
                i = j + 1;
                continue;
            }
            b'[' => stack.push((pending.take(), i)),
            b']' => {
                if let Some((key, open)) = stack.pop() {
                    if let Some(path) = path_of(&stack, key.as_ref()) {
                        out.push(Item::Array {
                            path,
                            open,
                            close: i,
                        });
                    }
                }
                pending = None;
            }
            b',' => pending = None,
            b'e' if text[i..].starts_with("env(")
                && (i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_')) =>
            {
                if let Some(call) = env_calls(&text[i..]).first().filter(|c| c.start == 0) {
                    if let Some(path) = path_of(&stack, pending.as_ref()) {
                        out.push(Item::Env {
                            path,
                            key: call.key.clone(),
                        });
                    }
                    i += call.end;
                    continue;
                }
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// The config key that reads `key` among `configs` — `(file name without
/// `.php`, text)` of each `config/*.php`: `app.url` for `APP_URL`.
pub fn config_key_for(configs: &[(String, String)], key: &str) -> Option<String> {
    let mut configs: Vec<&(String, String)> = configs.iter().collect();
    configs.sort_by(|a, b| a.0.cmp(&b.0));
    configs.iter().find_map(|(name, text)| {
        scan(text).into_iter().find_map(|item| match item {
            Item::Env { path, key: k } if k == key => Some(format!("{name}.{}", path.join("."))),
            _ => None,
        })
    })
}

/// Where a variable goes in `config/services.php`: `STRIPE_WEBHOOK_SECRET`
/// → `stripe` → `webhook_secret`; one without a prefix sits on its own.
pub fn service_key(key: &str) -> (String, Option<String>) {
    let low = key.to_ascii_lowercase();
    match low.split_once('_') {
        Some((service, name)) if !service.is_empty() && !name.is_empty() => {
            (service.to_string(), Some(name.to_string()))
        }
        _ => (low, None),
    }
}

/// The fix for an `env()` call: the edits, file by file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnvFix {
    pub title: String,
    pub edits: Vec<(PathBuf, Vec<TextEdit>)>,
}

fn insert(text: &str, at: usize, new_text: String) -> TextEdit {
    let p = position(text, at);
    TextEdit {
        range: Range::new(p, p),
        new_text,
    }
}

/// Read `call` — in `file`, whose text is `text` — through `config()`
/// instead. `configs` are the project's `config/*.php` as `(name, text)`;
/// `read` gives a file's text, `None` when it doesn't exist.
pub fn fix(
    root: &Path,
    file: &Path,
    text: &str,
    call: &EnvCall,
    configs: &[(String, String)],
    read: &dyn Fn(&Path) -> Option<String>,
) -> Option<EnvFix> {
    let key = &call.key;
    let entry = match &call.default {
        Some(d) => format!("env('{key}', {d})"),
        None => format!("env('{key}')"),
    };
    let mut edits: Vec<(PathBuf, Vec<TextEdit>)> = Vec::new();

    let (config_key, title) = match config_key_for(configs, key) {
        Some(existing) => {
            let title = format!("Use config('{existing}') instead of env('{key}')");
            (existing, title)
        }
        None => {
            let services = root.join("config/services.php");
            let (service, name) = service_key(key);
            let config_key = match &name {
                Some(n) => format!("services.{service}.{n}"),
                None => format!("services.{service}"),
            };
            let edit = match read(&services) {
                Some(s) => services_edit(&s, &service, name.as_deref(), &entry)?,
                None => {
                    let body = match &name {
                        Some(n) => {
                            format!("    '{service}' => [\n        '{n}' => {entry},\n    ],\n")
                        }
                        None => format!("    '{service}' => {entry},\n"),
                    };
                    TextEdit {
                        range: Range::new(Position::new(0, 0), Position::new(0, 0)),
                        new_text: format!("<?php\n\nreturn [\n\n{body}\n];\n"),
                    }
                }
            };
            edits.push((services, vec![edit]));
            let title =
                format!("Move env('{key}') to config/services.php and read config('{config_key}')");
            (config_key, title)
        }
    };

    // The call itself, last edit first is not needed: one edit per file.
    edits.insert(
        0,
        (
            file.to_path_buf(),
            vec![TextEdit {
                range: Range::new(position(text, call.start), position(text, call.end)),
                new_text: format!("config('{config_key}')"),
            }],
        ),
    );

    let example = root.join(".env.example");
    if let Some(ex) = read(&example) {
        let has = ex.lines().any(|l| {
            l.trim()
                .split_once('=')
                .is_some_and(|(k, _)| k.trim() == key)
        });
        if !has {
            let sep = if ex.is_empty() || ex.ends_with('\n') {
                ""
            } else {
                "\n"
            };
            edits.push((
                example,
                vec![insert(&ex, ex.len(), format!("{sep}{key}=\n"))],
            ));
        }
    }
    Some(EnvFix { title, edits })
}

/// The edit to `config/services.php` (text `s`) that adds `entry` as
/// `service.name`, in the service's array when it has one. `None` when that
/// key is already taken.
fn services_edit(s: &str, service: &str, name: Option<&str>, entry: &str) -> Option<TextEdit> {
    let items = scan(s);
    let taken = |path: &[&str]| {
        items.iter().any(|item| match item {
            Item::Env { path: p, .. } | Item::Array { path: p, .. } => p == path,
        })
    };
    let top_close = s.rfind(']')?;
    let line_start = |at: usize| s[..at].rfind('\n').map_or(0, |i| i + 1);
    // A new entry at the end, a blank line from the one before as the file
    // spaces them.
    let at_end = |entry: String| {
        let at = line_start(top_close);
        let blank_before = s[..at].ends_with("\n\n");
        let text = if blank_before {
            format!("{entry}\n")
        } else {
            format!("\n{entry}")
        };
        insert(s, at, text)
    };
    let Some(name) = name else {
        if taken(&[service]) {
            return None;
        }
        return Some(at_end(format!("    '{service}' => {entry},\n")));
    };
    if taken(&[service, name]) {
        return None;
    }
    let block = items.iter().find_map(|item| match item {
        Item::Array { path, open, close } if path.len() == 1 && path[0] == service => {
            Some((*open, *close))
        }
        _ => None,
    });
    match block {
        // On lines of its own: a line before the `]`, indented one more.
        Some((open, close)) if s[open..close].contains('\n') => {
            let at = line_start(close);
            let indent: String = s[at..close].chars().take_while(|c| *c == ' ').collect();
            Some(insert(s, at, format!("{indent}    '{name}' => {entry},\n")))
        }
        Some((open, _)) => Some(insert(s, open + 1, format!("'{name}' => {entry}, "))),
        // Taken by something that isn't an array.
        None if taken(&[service]) => None,
        None => Some(at_end(format!(
            "    '{service}' => [\n        '{name}' => {entry},\n    ],\n"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SERVICES: &str = "<?php\n\nreturn [\n\n    // Mail\n    'postmark' => [\n        'token' => env('POSTMARK_TOKEN'),\n    ],\n\n    'slack' => [\n        'notifications' => [\n            'bot_user_oauth_token' => env('SLACK_BOT_USER_OAUTH_TOKEN'),\n            'channel' => env('SLACK_BOT_USER_DEFAULT_CHANNEL'),\n        ],\n    ],\n\n];\n";

    const APP: &str = "<?php\n\nreturn [\n    'name' => env('APP_NAME', 'Laravel'),\n    'url' => env('APP_URL', 'http://localhost'),\n    'providers' => [\n        env('NOT_KEYED'),\n    ],\n];\n";

    fn configs() -> Vec<(String, String)> {
        vec![
            ("services".to_string(), SERVICES.to_string()),
            ("app".to_string(), APP.to_string()),
        ]
    }

    /// `text` with `edits` (in one file) applied.
    fn apply(text: &str, edits: &[TextEdit]) -> String {
        let offset = |p: Position| {
            let start: usize = text
                .split_inclusive('\n')
                .take(p.line as usize)
                .map(str::len)
                .sum();
            start + p.character as usize
        };
        let mut edits = edits.to_vec();
        edits.sort_by_key(|e| std::cmp::Reverse(offset(e.range.start)));
        let mut out = text.to_string();
        for e in edits {
            out.replace_range(offset(e.range.start)..offset(e.range.end), &e.new_text);
        }
        out
    }

    #[test]
    fn calls_are_found_with_their_defaults() {
        let text = "$a = env('STRIPE_SECRET');\n$b = env(\"APP_DEBUG\", false) ?: 1;\n$c = env('A', config('x', 'y'));\n";
        let calls = env_calls(text);
        let keys: Vec<(&str, Option<&str>)> = calls
            .iter()
            .map(|c| (c.key.as_str(), c.default.as_deref()))
            .collect();
        assert_eq!(
            keys,
            [
                ("STRIPE_SECRET", None),
                ("APP_DEBUG", Some("false")),
                ("A", Some("config('x', 'y')")),
            ]
        );
        assert_eq!(
            &text[calls[2].start..calls[2].end],
            "env('A', config('x', 'y'))"
        );
    }

    #[test]
    fn what_isnt_the_helper_is_left_alone() {
        let text = "@env('local')\n$this->env('X');\nApp::env('X');\ngetenv('X');\n// env('X')\n * env('X')\nenv($name);\n";
        assert!(env_calls(text).is_empty(), "{:?}", env_calls(text));
    }

    #[test]
    fn only_code_outside_config_is_flagged() {
        let text =
            "<?php\nclass Pay\n{\n    public function key() { return env('STRIPE_SECRET'); }\n}\n";
        let found = lint(Path::new("app/Services/Pay.php"), text);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].range.start, Position::new(3, 35));
        assert_eq!(
            found[0].code,
            Some(NumberOrString::String(
                "env-outside-config:STRIPE_SECRET".into()
            ))
        );
        for fine in [
            "config/services.php",
            "tests/Feature/PayTest.php",
            "packages/themer/config/themer.php",
            ".claude/worktrees/x/app/Pay.php",
        ] {
            assert!(lint(Path::new(fine), text).is_empty(), "{fine}");
        }
    }

    #[test]
    fn in_blade_only_the_php_counts() {
        let view = Path::new("resources/views/a.blade.php");
        let text = "<h1>{{ env('APP_NAME') }}</h1>\n@if (env('SHOW'))\n<pre>'key' => env('DOC_EXAMPLE'),</pre>\n@endif\n{{-- env('COMMENTED') --}}\n@php\n    $k = env('IN_PHP');\n@endphp\n{!! env('RAW') !!}\n";
        let lines: Vec<u32> = lint(view, text)
            .iter()
            .map(|d| d.range.start.line)
            .collect();
        assert_eq!(lines, [0, 1, 6, 8]);
    }

    #[test]
    fn a_config_key_is_found_by_the_variable_it_reads() {
        let c = configs();
        assert_eq!(config_key_for(&c, "APP_URL").as_deref(), Some("app.url"));
        assert_eq!(
            config_key_for(&c, "SLACK_BOT_USER_DEFAULT_CHANNEL").as_deref(),
            Some("services.slack.notifications.channel")
        );
        assert_eq!(
            config_key_for(&c, "POSTMARK_TOKEN").as_deref(),
            Some("services.postmark.token")
        );
        // In a list there is no key to read it by.
        assert_eq!(config_key_for(&c, "NOT_KEYED"), None);
        assert_eq!(config_key_for(&c, "STRIPE_SECRET"), None);
    }

    #[test]
    fn variables_are_named_for_their_service() {
        assert_eq!(
            service_key("STRIPE_WEBHOOK_SECRET"),
            ("stripe".into(), Some("webhook_secret".into()))
        );
        assert_eq!(service_key("SENTRY"), ("sentry".into(), None));
    }

    fn files(extra: &[(&str, &str)]) -> impl Fn(&Path) -> Option<String> {
        let map: Vec<(PathBuf, String)> = extra
            .iter()
            .map(|(p, t)| (Path::new("/p").join(p), t.to_string()))
            .collect();
        move |p: &Path| map.iter().find(|(q, _)| q == p).map(|(_, t)| t.clone())
    }

    fn edits_for<'a>(fix: &'a EnvFix, rel: &str) -> &'a [TextEdit] {
        fix.edits
            .iter()
            .find(|(p, _)| p == &Path::new("/p").join(rel))
            .map(|(_, e)| e.as_slice())
            .unwrap_or_else(|| panic!("no edit to {rel}: {fix:?}"))
    }

    #[test]
    fn a_known_variable_is_read_through_its_config_key() {
        let text = "<?php\n$url = env('APP_URL');\n";
        let call = &env_calls(text)[0];
        let read = files(&[(
            ".env.example",
            "APP_NAME=Laravel\nAPP_URL=http://localhost\n",
        )]);
        let fix = fix(
            Path::new("/p"),
            Path::new("/p/app/A.php"),
            text,
            call,
            &configs(),
            &read,
        )
        .unwrap();
        assert_eq!(fix.title, "Use config('app.url') instead of env('APP_URL')");
        assert_eq!(fix.edits.len(), 1, "nothing else to change: {fix:?}");
        assert_eq!(
            apply(text, edits_for(&fix, "app/A.php")),
            "<?php\n$url = config('app.url');\n"
        );
    }

    #[test]
    fn a_new_variable_moves_into_its_service_and_the_example() {
        let text = "<?php\n$s = env('POSTMARK_SECRET', 'x');\n$t = env('STRIPE_WEBHOOK_SECRET');\n";
        let read = files(&[
            ("config/services.php", SERVICES),
            (".env.example", "APP_NAME=Laravel"),
        ]);
        let root = Path::new("/p");
        let file = Path::new("/p/app/Hook.php");

        // Into the service's array that is there.
        let calls = env_calls(text);
        let fix1 = fix(root, file, text, &calls[0], &configs(), &read).unwrap();
        assert_eq!(
            fix1.title,
            "Move env('POSTMARK_SECRET') to config/services.php and read config('services.postmark.secret')"
        );
        assert_eq!(
            apply(text, edits_for(&fix1, "app/Hook.php")),
            "<?php\n$s = config('services.postmark.secret');\n$t = env('STRIPE_WEBHOOK_SECRET');\n"
        );
        let services = apply(SERVICES, edits_for(&fix1, "config/services.php"));
        assert!(
            services.contains("        'token' => env('POSTMARK_TOKEN'),\n        'secret' => env('POSTMARK_SECRET', 'x'),\n    ],"),
            "{services}"
        );
        assert_eq!(
            apply("APP_NAME=Laravel", edits_for(&fix1, ".env.example")),
            "APP_NAME=Laravel\nPOSTMARK_SECRET=\n"
        );

        // A service of its own, before the closing `];`.
        let fix2 = fix(root, file, text, &calls[1], &configs(), &read).unwrap();
        let services = apply(SERVICES, edits_for(&fix2, "config/services.php"));
        assert!(
            services.ends_with("    ],\n\n    'stripe' => [\n        'webhook_secret' => env('STRIPE_WEBHOOK_SECRET'),\n    ],\n\n];\n"),
            "{services}"
        );
        // And the result reads back as the key the call now uses.
        let back = vec![("services".to_string(), services)];
        assert_eq!(
            config_key_for(&back, "STRIPE_WEBHOOK_SECRET").as_deref(),
            Some("services.stripe.webhook_secret")
        );
    }

    #[test]
    fn without_a_services_file_one_is_made() {
        let text = "<?php\n$k = env('OPENAI_API_KEY');\n";
        let call = &env_calls(text)[0];
        let fix = fix(
            Path::new("/p"),
            Path::new("/p/app/Ai.php"),
            text,
            call,
            &[],
            &files(&[]),
        )
        .unwrap();
        let made = apply("", edits_for(&fix, "config/services.php"));
        assert_eq!(
            made,
            "<?php\n\nreturn [\n\n    'openai' => [\n        'api_key' => env('OPENAI_API_KEY'),\n    ],\n\n];\n"
        );
        assert_eq!(
            config_key_for(&[("services".into(), made)], "OPENAI_API_KEY").as_deref(),
            Some("services.openai.api_key")
        );
        // No `.env.example`, nothing to add to.
        assert_eq!(fix.edits.len(), 2);
    }

    #[test]
    fn a_taken_key_is_not_overwritten() {
        let services =
            "<?php\nreturn [\n    'postmark' => [\n        'token' => env('OTHER'),\n    ],\n];\n";
        let text = "<?php\nenv('POSTMARK_TOKEN');\n";
        let call = &env_calls(text)[0];
        let read = files(&[("config/services.php", services)]);
        let configs = vec![("services".to_string(), services.to_string())];
        assert_eq!(
            fix(
                Path::new("/p"),
                Path::new("/p/app/A.php"),
                text,
                call,
                &configs,
                &read
            ),
            None
        );
    }
}
