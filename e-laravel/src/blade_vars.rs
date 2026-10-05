//! What a Blade view's variables are: `{{ $user->` in `users/show.blade.php`
//! is the `$user` a controller passed with `view('users.show', compact('user'))`
//! — and that one was type-hinted `User $user`. Found by reading where the
//! view is rendered, so `$user->` completes the model's columns in Blade as it
//! does in PHP.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::eloquent::resolve_model;

/// The model class of `$var` in the view at `view_file`, if where it is
/// rendered says. Cached per view for a few seconds: completion asks on every
/// keystroke and the answer means reading the controllers.
pub fn view_var_model(root: &Path, view_file: &Path, var: &str) -> Option<String> {
    type Cache = HashMap<PathBuf, (Instant, HashMap<String, String>)>;
    static CACHE: Mutex<Option<Cache>> = Mutex::new(None);
    const FRESH: Duration = Duration::from_secs(10);

    let mut guard = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let cache = guard.get_or_insert_with(HashMap::new);
    if let Some((at, vars)) = cache.get(view_file) {
        if at.elapsed() < FRESH {
            return vars.get(var).cloned();
        }
    }
    let vars = view_vars(root, view_file);
    let found = vars.get(var).cloned();
    cache.insert(view_file.to_path_buf(), (Instant::now(), vars));
    found
}

/// Every variable a view is given whose model can be told: view variable →
/// model class. The first render site that types a variable wins.
pub fn view_vars(root: &Path, view_file: &Path) -> HashMap<String, String> {
    let mut out = HashMap::new();
    // A Livewire component's view sees the component's public properties.
    if let Some(comp) = crate::livewire::resolve(root, view_file) {
        if let Ok(src) = std::fs::read_to_string(&comp.class_file) {
            let class = crate::livewire::class_part(&src);
            for prop in crate::livewire::properties(class) {
                if let Some(model) = resolve_model(class, &prop) {
                    out.entry(prop).or_insert(model);
                }
            }
        }
    }
    let Some(name) = crate::keys::view_name(root, view_file) else {
        return out;
    };
    for file in php_files(root) {
        let Ok(src) = std::fs::read_to_string(&file) else {
            continue;
        };
        for at in render_sites(&src, &name) {
            for (var, expr) in passed(&src, at) {
                if out.contains_key(&var) {
                    continue;
                }
                if let Some(model) = expr_model(&src[..at], &expr) {
                    out.insert(var, model);
                }
            }
        }
    }
    out
}

/// Offsets just past the closing quote of `name` in every call that renders
/// it: `view('name'`, `View::make('name'`, `->view('name'`, `->markdown('name'`.
fn render_sites(src: &str, name: &str) -> Vec<usize> {
    let mut out = Vec::new();
    for call in ["view(", "View::make(", "->view(", "->markdown("] {
        for q in ['\'', '"'] {
            let needle = format!("{call}{q}{name}{q}");
            let mut from = 0;
            while let Some(rel) = src[from..].find(&needle) {
                let at = from + rel + needle.len();
                from = at;
                out.push(at);
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// What a render call passes, from just after the view name: the data
/// argument (`compact('a', 'b')` or `['a' => $x]`) and any `->with(…)`
/// chained after the call. Each as (view variable, expression).
fn passed(src: &str, after_name: usize) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let rest = &src[after_name..];
    let (args, close) = until_close(rest, ')');
    if let Some(data) = args.trim_start().strip_prefix(',') {
        out.extend(data_vars(data.trim()));
    }
    // `->with('a', $x)`, `->with(['a' => $x])`, `->with(compact('a'))`, chained.
    let mut tail = &rest[close.min(rest.len())..];
    while let Some(w) = tail
        .trim_start()
        .strip_prefix(")->with(")
        .or_else(|| tail.trim_start().strip_prefix("->with("))
    {
        let (args, close) = until_close(w, ')');
        let a = args.trim();
        if a.starts_with('\'') || a.starts_with('"') {
            if let Some((key, expr)) = a.split_once(',') {
                out.push((unquote(key.trim()), expr.trim().to_string()));
            }
        } else {
            out.extend(data_vars(a));
        }
        tail = &w[close.min(w.len())..];
    }
    out
}

/// `compact('a', 'b')` → `[("a", "$a"), ("b", "$b")]`; `['a' => $x]` → `[("a", "$x")]`.
fn data_vars(data: &str) -> Vec<(String, String)> {
    if let Some(inner) = data.strip_prefix("compact(") {
        let (names, _) = until_close(inner, ')');
        return names
            .split(',')
            .map(|n| unquote(n.trim()))
            .filter(|n| !n.is_empty())
            .map(|n| (n.clone(), format!("${n}")))
            .collect();
    }
    let Some(inner) = data.strip_prefix('[') else {
        return Vec::new();
    };
    let (body, _) = until_close(inner, ']');
    split_top(body)
        .into_iter()
        .filter_map(|entry| {
            let (k, v) = entry.split_once("=>")?;
            let k = unquote(k.trim());
            (!k.is_empty()).then(|| (k, v.trim().to_string()))
        })
        .collect()
}

/// The model an expression evaluates to, where that's plain: `$var` (from its
/// assignment or type hint above), `User::find(…)`, `new User`, `auth()->user()`.
fn expr_model(before: &str, expr: &str) -> Option<String> {
    let expr = expr.trim();
    if let Some(var) = expr.strip_prefix('$') {
        let name: String = var
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if name.len() == var.len() {
            return resolve_model(before, &name);
        }
    }
    if expr.starts_with("auth()->user()") || expr.starts_with("$request->user()") {
        return Some("User".into());
    }
    let expr = expr.strip_prefix("new ").unwrap_or(expr);
    let class: String = expr
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '\\')
        .collect();
    let class = class.rsplit('\\').next().unwrap_or(&class);
    let rest = &expr[class.len().min(expr.len())..];
    let single = rest.starts_with("::find(")
        || rest.starts_with("::findOrFail(")
        || rest.starts_with("::first")
        || rest.starts_with("::create(")
        || rest.starts_with('(')
        || rest.is_empty();
    (class.chars().next().is_some_and(char::is_uppercase) && single).then(|| class.to_string())
}

/// `s` up to the `close` that ends it (nesting and strings respected), and
/// the offset just past that close.
fn until_close(s: &str, close: char) -> (&str, usize) {
    let (mut depth, mut quote) = (0i32, None);
    for (i, c) in s.char_indices() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None => match c {
                '\'' | '"' => quote = Some(c),
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' if depth > 0 => depth -= 1,
                c2 if c2 == close && depth == 0 => return (&s[..i], i + 1),
                _ => {}
            },
        }
    }
    (s, s.len())
}

/// Split on top-level commas.
fn split_top(s: &str) -> Vec<&str> {
    let (mut depth, mut quote, mut start) = (0i32, None, 0);
    let mut out = Vec::new();
    for (i, c) in s.char_indices() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None => match c {
                '\'' | '"' => quote = Some(c),
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth -= 1,
                ',' if depth == 0 => {
                    out.push(&s[start..i]);
                    start = i + 1;
                }
                _ => {}
            },
        }
    }
    out.push(&s[start..]);
    out
}

fn unquote(s: &str) -> String {
    s.trim_matches(['\'', '"']).to_string()
}

fn php_files(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
        if depth > 8 {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out, depth + 1);
            } else if p.extension().is_some_and(|x| x == "php")
                && !p.to_string_lossy().ends_with(".blade.php")
            {
                out.push(p);
            }
        }
    }
    let mut out = Vec::new();
    walk(&root.join("app"), &mut out, 0);
    walk(&root.join("routes"), &mut out, 0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variables_come_from_where_the_view_is_rendered() {
        let src = "<?php\nclass UserController {\n    public function show(User $user, Request $request) {\n        $posts = Post::query()->latest()->get();\n        $team = Team::findOrFail(1);\n        return view('users.show', compact('user', 'posts'))\n            ->with('owner', auth()->user())\n            ->with(['team' => $team, 'title' => 'Hi']);\n    }\n}\n";
        let at = render_sites(src, "users.show")[0];
        let vars: HashMap<String, Option<String>> = passed(src, at)
            .into_iter()
            .map(|(v, e)| {
                let m = expr_model(&src[..at], &e);
                (v, m)
            })
            .collect();
        assert_eq!(vars["user"].as_deref(), Some("User"));
        assert_eq!(vars["owner"].as_deref(), Some("User"));
        assert_eq!(vars["team"].as_deref(), Some("Team"));
        assert_eq!(vars["title"], None);
        // A query, not a model: left alone.
        assert_eq!(vars["posts"], None);
    }

    #[test]
    fn an_array_argument_maps_names_to_expressions() {
        let src = "return view(\"orders.edit\", ['order' => Order::find($id), 'items' => $items]);";
        let at = render_sites(src, "orders.edit")[0];
        let vars = passed(src, at);
        assert_eq!(
            vars[0],
            ("order".to_string(), "Order::find($id)".to_string())
        );
        assert_eq!(expr_model(&src[..at], &vars[0].1).as_deref(), Some("Order"));
    }

    #[test]
    fn a_whole_project_is_read() {
        let root = std::env::temp_dir().join(format!("e-bv-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("app/Http/Controllers")).unwrap();
        std::fs::create_dir_all(root.join("resources/views/users")).unwrap();
        std::fs::write(
            root.join("app/Http/Controllers/UserController.php"),
            "<?php\nclass UserController {\n    public function show(User $user) {\n        return view('users.show', compact('user'));\n    }\n}\n",
        )
        .unwrap();
        let view = root.join("resources/views/users/show.blade.php");
        std::fs::write(&view, "{{ $user->name }}").unwrap();
        assert_eq!(
            view_var_model(&root, &view, "user").as_deref(),
            Some("User")
        );
        assert_eq!(view_var_model(&root, &view, "nobody"), None);
    }
}
