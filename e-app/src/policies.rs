//! Gate & policy intelligence: completion and go-to-definition for abilities in
//! `can()`, `authorize()`, `Gate::allows()`, `@can`, … resolving to the policy
//! method or `Gate::define()` that declares them.

use std::path::{Path, PathBuf};

/// Call expressions whose first string argument is an ability name.
pub const CALLS: &[&str] = &[
    "can(",
    "cannot(",
    "cant(",
    "authorize(",
    "authorizeForUser(",
    "Gate::allows(",
    "Gate::denies(",
    "Gate::any(",
    "Gate::none(",
    "Gate::authorize(",
];

fn is_ident(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// All abilities in the project: policy methods and `Gate::define()` names,
/// as `(name, file, line)`.
/// The model class named by the argument after the ability string the caret
/// is in: `can('update', $post)` → `Post`, likewise `$this->post`,
/// `$comment->post`, `Post::class` and `[$post]`. `None` without one.
pub fn model_argument(text: &str, offset: usize) -> Option<String> {
    let rest = text.get(offset..)?;
    let close = rest.find(['\'', '"'])?;
    let after = rest[close + 1..]
        .trim_start()
        .strip_prefix(',')?
        .trim_start();
    let after = after.strip_prefix('[').unwrap_or(after).trim_start();
    let expr: String = after
        .chars()
        .take_while(|c| !matches!(c, ')' | ',' | ']' | '\n'))
        .collect();
    let expr = expr.trim();
    let name = match expr.strip_suffix("::class") {
        Some(class) => class.rsplit('\\').next()?.to_string(),
        None => {
            let last = expr.rsplit("->").next()?.trim_start_matches('$');
            let mut c = last.chars();
            let first = c.next()?;
            first.to_uppercase().collect::<String>() + c.as_str()
        }
    };
    (!name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_')).then_some(name)
}

pub fn abilities(root: &Path) -> Vec<(String, PathBuf, usize)> {
    let mut out = Vec::new();

    // Policy methods (each public method is an ability).
    collect_php(&root.join("app/Policies"), &mut |file, src| {
        for (i, line) in src.lines().enumerate() {
            let t = line.trim_start();
            if let Some(rest) = t.strip_prefix("public function ") {
                let name: String = rest.chars().take_while(|c| is_ident(*c)).collect();
                if !name.is_empty()
                    && !name.starts_with("__")
                    && !matches!(name.as_str(), "before" | "after")
                {
                    out.push((name, file.to_path_buf(), i));
                }
            }
        }
    });

    // `Gate::define('ability', …)` in providers.
    collect_php(&root.join("app/Providers"), &mut |file, src| {
        let mut search = 0;
        while let Some(rel) = src[search..].find("Gate::define(") {
            let at = search + rel + "Gate::define(".len();
            search = at;
            let after = src[at..].trim_start();
            if let Some(q) = after.chars().next() {
                if q == '\'' || q == '"' {
                    if let Some(end) = after[1..].find(q) {
                        let name = after[1..1 + end].to_string();
                        let line = src[..at].bytes().filter(|b| *b == b'\n').count();
                        out.push((name, file.to_path_buf(), line));
                    }
                }
            }
        }
    });

    out.sort_by(|a, b| a.0.cmp(&b.0));
    out.dedup_by(|a, b| a.0 == b.0);
    out
}

fn collect_php(dir: &Path, f: &mut dyn FnMut(&Path, &str)) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for e in read.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_php(&p, f);
        } else if p.extension().and_then(|x| x.to_str()) == Some("php") {
            if let Ok(src) = std::fs::read_to_string(&p) {
                f(&p, &src);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_policy_and_gate_abilities() {
        let root = std::env::temp_dir().join(format!("e-pol-{}", std::process::id()));
        std::fs::create_dir_all(root.join("app/Policies")).unwrap();
        std::fs::create_dir_all(root.join("app/Providers")).unwrap();
        std::fs::write(
            root.join("app/Policies/PostPolicy.php"),
            "<?php\nclass PostPolicy {\n    public function before() {}\n    public function update(User $u, Post $p) {}\n    public function delete(User $u, Post $p) {}\n}",
        )
        .unwrap();
        std::fs::write(
            root.join("app/Providers/AppServiceProvider.php"),
            "<?php\nGate::define('access-admin', fn ($u) => $u->admin);",
        )
        .unwrap();
        let ab = abilities(&root);
        let names: Vec<&str> = ab.iter().map(|(n, _, _)| n.as_str()).collect();
        assert!(names.contains(&"update"));
        assert!(names.contains(&"delete"));
        assert!(names.contains(&"access-admin"));
        assert!(!names.contains(&"before")); // hook, not an ability
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_model_comes_from_the_second_argument() {
        let at = |src: &str| model_argument(src, src.find("upd").unwrap());
        assert_eq!(
            at("$this->authorize('update', $post);").as_deref(),
            Some("Post")
        );
        assert_eq!(
            at("@can('update', $comment->post)").as_deref(),
            Some("Post")
        );
        assert_eq!(
            at("Gate::allows('update', [$this->invoice])").as_deref(),
            Some("Invoice")
        );
        assert_eq!(
            at("$user->can('update', \\App\\Models\\Team::class)").as_deref(),
            Some("Team")
        );
        assert_eq!(at("Gate::allows('update')"), None);
    }
}
