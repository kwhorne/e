//! Where a Laravel key is used, across the project: a route name, a view or a
//! translation key — the strings Laravel resolves at runtime, which no PHP
//! language server can follow. Find Usages lists them; Rename rewrites them
//! (and moves a view's file), refusing what it can't do completely.

use std::path::{Path, PathBuf};

use crate::laravel::{helper_calls, Helper};

/// One place a key appears.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyUse {
    pub path: PathBuf,
    /// 0-based line, and the byte column and length of the key on it.
    pub line: usize,
    pub col: usize,
    pub len: usize,
    /// Where the key is defined (`->name('…')`, a lang file line, the view
    /// file) rather than used.
    pub definition: bool,
    /// The line, trimmed, for display.
    pub text: String,
}

/// Every use and definition of `key` in the project.
pub fn key_uses(root: &Path, helper: Helper, key: &str) -> Vec<KeyUse> {
    let mut out = Vec::new();
    for file in project_files(root, helper) {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let mut found: Vec<(usize, usize, bool)> = helper_calls(&text)
            .into_iter()
            .filter(|c| c.helper == helper && c.token == key)
            .map(|c| (c.start, c.end, false))
            .collect();
        let extra: &[(&str, bool)] = match helper {
            Helper::Route => &[
                ("->name(", true),
                ("routeIs(", false),
                ("Route::has(", false),
            ],
            Helper::View => &[
                ("->view(", false),
                ("->markdown(", false),
                ("View::make(", false),
            ],
            _ => &[],
        };
        for (pattern, definition) in extra {
            for (s, e) in quoted_args(&text, pattern, 0) {
                if &text[s..e] == key {
                    found.push((s, e, *definition));
                }
            }
        }
        if helper == Helper::View {
            // `Route::view('/about', 'pages.about')`: the view is the second argument.
            for (s, e) in quoted_args(&text, "Route::view(", 1) {
                if &text[s..e] == key {
                    found.push((s, e, false));
                }
            }
        }
        found.sort();
        found.dedup();
        for (s, e, definition) in found {
            out.push(use_at(&file, &text, s, e - s, definition));
        }
    }
    match helper {
        Helper::View => {
            if let Some(file) = view_file(root, key) {
                out.push(KeyUse {
                    path: file,
                    line: 0,
                    col: 0,
                    len: 0,
                    definition: true,
                    text: String::new(),
                });
            }
        }
        Helper::Trans => out.extend(trans_definitions(root, key)),
        _ => {}
    }
    out.sort_by(|a, b| (!a.definition, &a.path, a.line).cmp(&(!b.definition, &b.path, b.line)));
    out
}

/// The file a dotted view name lives in, if it exists.
pub fn view_file(root: &Path, name: &str) -> Option<PathBuf> {
    if name.contains("::") {
        return None;
    }
    let rel: PathBuf = name.split('.').collect();
    let file = root
        .join("resources/views")
        .join(rel)
        .with_extension("blade.php");
    file.is_file().then_some(file)
}

/// The view name of a file under `resources/views`.
pub fn view_name(root: &Path, file: &Path) -> Option<String> {
    let rel = file.strip_prefix(root.join("resources/views")).ok()?;
    let s = rel.to_string_lossy();
    let stem = s.strip_suffix(".blade.php")?;
    Some(stem.replace('/', "."))
}

/// The line of `key` in each language's file: `auth.failed` → `failed` in
/// `lang/*/auth.php`; a JSON key → its line in `lang/*.json`.
fn trans_definitions(root: &Path, key: &str) -> Vec<KeyUse> {
    let mut out = Vec::new();
    for base in [root.join("lang"), root.join("resources/lang")] {
        let Ok(entries) = std::fs::read_dir(&base) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            let (file, json) = if p.is_dir() {
                let Some(group) = key.split('.').next() else {
                    continue;
                };
                (p.join(format!("{group}.php")), false)
            } else if p.extension().is_some_and(|x| x == "json") {
                (p, true)
            } else {
                continue;
            };
            let Ok(text) = std::fs::read_to_string(&file) else {
                continue;
            };
            let Some(line) = crate::laravel::find_trans_line_in(&text, key, json) else {
                continue;
            };
            let needle = if json {
                key
            } else {
                key.rsplit('.').next().unwrap_or(key)
            };
            let line_text = text.lines().nth(line).unwrap_or("");
            let Some(col) = [format!("'{needle}'"), format!("\"{needle}\"")]
                .iter()
                .find_map(|q| line_text.find(q.as_str()))
            else {
                continue;
            };
            let start = line_start(&text, line) + col + 1;
            out.push(use_at(&file, &text, start, needle.len(), true));
        }
    }
    out
}

fn line_start(text: &str, line: usize) -> usize {
    if line == 0 {
        return 0;
    }
    text.match_indices('\n')
        .nth(line - 1)
        .map_or(text.len(), |(i, _)| i + 1)
}

fn use_at(path: &Path, text: &str, start: usize, len: usize, definition: bool) -> KeyUse {
    let line = text[..start].matches('\n').count();
    let ls = text[..start].rfind('\n').map_or(0, |i| i + 1);
    let le = text[start..].find('\n').map_or(text.len(), |i| start + i);
    KeyUse {
        path: path.to_path_buf(),
        line,
        col: start - ls,
        len,
        definition,
        text: text[ls..le].trim().to_string(),
    }
}

/// Byte ranges of the `nth` (0-based) quoted string argument of each
/// `pattern(` call — only when the string is the whole argument.
fn quoted_args(text: &str, pattern: &str, nth: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut from = 0;
    'calls: while let Some(rel) = text[from..].find(pattern) {
        let mut at = from + rel + pattern.len();
        from = at;
        for i in 0..=nth {
            let rest = &text[at..];
            let ws = rest.len() - rest.trim_start().len();
            let Some(q) = rest[ws..]
                .chars()
                .next()
                .filter(|c| *c == '\'' || *c == '"')
            else {
                continue 'calls;
            };
            let s = at + ws + 1;
            let Some(len) = text[s..].find(q) else {
                continue 'calls;
            };
            let e = s + len;
            let after = text[e + 1..].trim_start();
            if i == nth {
                if after.starts_with(')') || after.starts_with(',') {
                    out.push((s, e));
                }
                continue 'calls;
            }
            let Some(comma) = after.strip_prefix(',') else {
                continue 'calls;
            };
            at = text.len() - comma.len();
        }
    }
    out
}

/// The files a key of this kind can appear in.
fn project_files(root: &Path, helper: Helper) -> Vec<PathBuf> {
    let mut dirs = vec![
        "app",
        "routes",
        "resources/views",
        "config",
        "tests",
        "database",
    ];
    if helper == Helper::Route {
        dirs.push("resources/js"); // Ziggy's `route('…')`
    }
    let mut out = Vec::new();
    for d in dirs {
        collect(&root.join(d), &mut out, 0);
    }
    out
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > 10 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name();
        if p.is_dir() {
            if !matches!(name.to_str(), Some("node_modules" | "vendor" | ".git")) {
                collect(&p, out, depth + 1);
            }
        } else if p
            .extension()
            .and_then(|x| x.to_str())
            .is_some_and(|x| matches!(x, "php" | "js" | "ts" | "jsx" | "tsx" | "vue" | "svelte"))
        {
            out.push(p);
        }
    }
}

/// Why a rename of `old` to `new` can't be done completely, if it can't.
pub fn rename_blocker(helper: Helper, old: &str, new: &str, uses: &[KeyUse]) -> Option<String> {
    if new.trim().is_empty() || new.contains(['\'', '"', ' ']) {
        return Some(format!("“{new}” isn't a valid name"));
    }
    match helper {
        Helper::Route if !uses.iter().any(|u| u.definition) => Some(format!(
            "“{old}” isn't declared as ->name('{old}') — it is built from a group's \
             name prefix. Rename it where it is declared."
        )),
        Helper::Trans => {
            let (op, _) = old.rsplit_once('.').unwrap_or(("", old));
            let (np, _) = new.rsplit_once('.').unwrap_or(("", new));
            (op != np).then(|| {
                "Only a translation key's last part can be renamed: the rest names \
                 the file and the arrays it sits in."
                    .to_string()
            })
        }
        Helper::View if !uses.iter().any(|u| u.definition) => {
            Some(format!("No file for the view “{old}” in resources/views"))
        }
        Helper::Route | Helper::View => None,
        _ => Some("Only route names, views and translation keys can be renamed here".into()),
    }
}

/// What a rename writes: for each use, the text that replaces the key there
/// (a lang file's line holds only the key's last part).
pub fn replacement(helper: Helper, use_: &KeyUse, new: &str) -> String {
    if helper == Helper::Trans
        && use_.definition
        && use_.path.extension().is_some_and(|x| x == "php")
    {
        new.rsplit('.').next().unwrap_or(new).to_string()
    } else {
        new.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> PathBuf {
        // A directory per call: the tests run in parallel.
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("e-keys-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for d in [
            "app/Http/Controllers",
            "routes",
            "resources/views/users",
            "resources/js",
            "lang/en",
            "lang/nb",
        ] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::fs::write(
            root.join("routes/web.php"),
            "<?php\nRoute::get('/users/{user}', [UserController::class, 'show'])->name('users.show');\nRoute::view('/about', 'users.index');\n",
        )
        .unwrap();
        std::fs::write(
            root.join("app/Http/Controllers/UserController.php"),
            "<?php\nclass UserController {\n    public function show() {\n        if (request()->routeIs('users.show')) {}\n        return view('users.index', ['t' => __('auth.failed')]);\n    }\n    public function go() { return to_route('users.show', 1); }\n}\n",
        )
        .unwrap();
        std::fs::write(
            root.join("resources/views/users/index.blade.php"),
            "<a href=\"{{ route('users.show', $u) }}\">@lang('auth.failed')</a>\n@include('users.index')\n",
        )
        .unwrap();
        std::fs::write(
            root.join("resources/js/app.js"),
            "visit(route('users.show', 1));\n",
        )
        .unwrap();
        std::fs::write(
            root.join("lang/en/auth.php"),
            "<?php\nreturn [\n    'failed' => 'Nope.',\n];\n",
        )
        .unwrap();
        std::fs::write(
            root.join("lang/nb/auth.php"),
            "<?php\nreturn [\n    'failed' => 'Nei.',\n];\n",
        )
        .unwrap();
        root
    }

    #[test]
    fn a_route_name_is_found_everywhere_it_is_used_and_declared() {
        let root = project();
        let uses = key_uses(&root, Helper::Route, "users.show");
        assert!(uses[0].definition && uses[0].path.ends_with("routes/web.php"));
        let files: Vec<String> = uses
            .iter()
            .map(|u| u.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            files,
            [
                "web.php",
                "UserController.php",
                "UserController.php",
                "app.js",
                "index.blade.php"
            ]
        );
        assert!(rename_blocker(Helper::Route, "users.show", "people.show", &uses).is_none());
    }

    #[test]
    fn a_view_is_found_with_its_file() {
        let root = project();
        let uses = key_uses(&root, Helper::View, "users.index");
        assert!(uses
            .iter()
            .any(|u| u.definition && u.path.ends_with("users/index.blade.php")));
        assert_eq!(uses.iter().filter(|u| !u.definition).count(), 3); // view(), Route::view, @include
        let file = root.join("resources/views/users/index.blade.php");
        assert_eq!(view_name(&root, &file).as_deref(), Some("users.index"));
    }

    #[test]
    fn a_translation_key_is_found_in_every_language() {
        let root = project();
        let uses = key_uses(&root, Helper::Trans, "auth.failed");
        assert_eq!(uses.iter().filter(|u| u.definition).count(), 2);
        assert_eq!(uses.iter().filter(|u| !u.definition).count(), 2);
        let def = uses.iter().find(|u| u.definition).unwrap();
        assert_eq!(def.len, "failed".len());
        assert_eq!(replacement(Helper::Trans, def, "auth.denied"), "denied");
        assert!(rename_blocker(Helper::Trans, "auth.failed", "login.failed", &uses).is_some());
        assert!(rename_blocker(Helper::Trans, "auth.failed", "auth.denied", &uses).is_none());
    }

    #[test]
    fn a_prefixed_route_name_refuses_a_rename() {
        let uses = vec![KeyUse {
            path: PathBuf::from("a.php"),
            line: 0,
            col: 0,
            len: 3,
            definition: false,
            text: String::new(),
        }];
        assert!(rename_blocker(Helper::Route, "admin.users", "admin.people", &uses).is_some());
    }
}
