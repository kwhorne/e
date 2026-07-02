//! Inertia awareness: `Inertia::render('Users/Index')` should behave like
//! `view('users.index')` does — resolve to the page component under
//! `resources/js/Pages`, complete existing pages, and let the architecture map
//! reach past the controller to the actual Vue/Svelte/React page.

use std::path::{Path, PathBuf};

/// Page-component file extensions, in resolution order.
const EXTS: &[&str] = &["vue", "tsx", "jsx", "ts", "js", "svelte"];

/// Candidate Pages directories (Inertia's default plus common variants).
pub fn page_roots(root: &Path) -> Vec<PathBuf> {
    [
        "resources/js/Pages",
        "resources/js/pages",
        "resources/ts/Pages",
        "resources/js/Components/Pages",
    ]
    .iter()
    .map(|p| root.join(p))
    .filter(|p| p.is_dir())
    .collect()
}

/// Resolve a page name (`Users/Index`) to its component file.
pub fn resolve_page(root: &Path, name: &str) -> Option<PathBuf> {
    let rel = name.trim().replace('\\', "/");
    for base in page_roots(root) {
        for ext in EXTS {
            let cand = base.join(&rel).with_extension(ext);
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    None
}

/// Every page component's name (`Users/Index`) for completion.
pub fn list_pages(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for base in page_roots(root) {
        collect(&base, &base, &mut out);
    }
    out.sort();
    out.dedup();
    out
}

fn collect(base: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for e in read.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(base, &p, out);
        } else if p
            .extension()
            .and_then(|x| x.to_str())
            .map(|x| EXTS.contains(&x))
            .unwrap_or(false)
        {
            if let Ok(rel) = p.strip_prefix(base) {
                let name = rel.with_extension("").to_string_lossy().replace('\\', "/");
                out.push(name);
            }
        }
    }
}

/// If `offset` sits inside the first string argument of any `needle(` call,
/// return that string.
pub fn call_string_at(text: &str, offset: usize, needles: &[&str]) -> Option<String> {
    for needle in needles {
        let mut search = 0;
        while let Some(rel) = text[search..].find(needle) {
            let open = search + rel + needle.len();
            search = open;
            let after = text[open..].trim_start();
            let ws = text[open..].len() - after.len();
            let Some(quote) = after.chars().next() else {
                continue;
            };
            if quote != '\'' && quote != '"' {
                continue;
            }
            let val_start = open + ws + 1;
            let Some(close_rel) = text[val_start..].find(quote) else {
                continue;
            };
            let val_end = val_start + close_rel;
            if offset >= val_start && offset <= val_end {
                return Some(text[val_start..val_end].to_string());
            }
        }
    }
    None
}

/// If the cursor line ends inside an unclosed `needle('…` value, return the
/// partial typed so far.
pub fn call_string_partial(line_before: &str, needles: &[&str]) -> Option<String> {
    for needle in needles {
        if let Some(at) = line_before.rfind(needle) {
            let after = line_before[at + needle.len()..].trim_start();
            let mut chars = after.chars();
            if let Some(q) = chars.next() {
                if (q == '\'' || q == '"') && !chars.as_str().contains(q) {
                    return Some(chars.as_str().to_string());
                }
            }
        }
    }
    None
}

const RENDER: &[&str] = &["Inertia::render(", "inertia("];
const ROUTE: &[&str] = &["route("];

/// Page name at `offset` inside `Inertia::render('…')`.
pub fn render_at(text: &str, offset: usize) -> Option<String> {
    call_string_at(text, offset, RENDER)
}

/// Partial page name for completion inside `Inertia::render('…`.
pub fn render_partial(line_before: &str) -> Option<String> {
    call_string_partial(line_before, RENDER)
}

/// Ziggy route name at `offset` inside `route('…')` (for goto/hover in JS).
pub fn route_at(text: &str, offset: usize) -> Option<String> {
    call_string_at(text, offset, ROUTE)
}

/// Partial route name for completion inside `route('…`.
pub fn route_partial(line_before: &str) -> Option<String> {
    call_string_partial(line_before, ROUTE)
}

/// Global props shared by `HandleInertiaRequests::share()`, as dotted paths
/// (`auth`, `auth.user`, `flash.message`, …) for completion everywhere.
pub fn shared_props(root: &Path) -> Vec<String> {
    let file = root.join("app/Http/Middleware/HandleInertiaRequests.php");
    let Ok(src) = std::fs::read_to_string(&file) else {
        return Vec::new();
    };
    let Some(start) = src.find("function share") else {
        return Vec::new();
    };
    let s = &src[start..];
    let bytes = s.as_bytes();
    let mut out: Vec<String> = Vec::new();
    let mut depth = 0i32;
    let mut entered = false;
    let mut parent: Option<String> = None;
    let mut pending: Option<String> = None;
    let mut i = 0;
    while i < s.len() {
        match bytes[i] as char {
            '[' => {
                depth += 1;
                entered = true;
                if depth == 2 {
                    parent = pending.clone();
                }
                i += 1;
            }
            ']' => {
                if depth == 2 {
                    parent = None;
                }
                depth -= 1;
                i += 1;
                if entered && depth == 0 {
                    break; // finished the props array
                }
            }
            c @ ('\'' | '"') => {
                let mut j = i + 1;
                while j < s.len() && bytes[j] as char != c {
                    j += 1;
                }
                let key = &s[i + 1..j.min(s.len())];
                // Key only if followed by `=>`.
                let mut k = j + 1;
                while k < s.len() && (bytes[k] as char).is_whitespace() {
                    k += 1;
                }
                let is_key = s[k..].starts_with("=>")
                    && !key.is_empty()
                    && key
                        .chars()
                        .all(|c| c.is_alphanumeric() || c == '_' || c == '-');
                if is_key {
                    if depth == 1 {
                        out.push(key.to_string());
                        pending = Some(key.to_string());
                    } else if depth == 2 {
                        if let Some(p) = &parent {
                            out.push(format!("{p}.{key}"));
                        }
                    }
                }
                i = j + 1;
            }
            _ => i += 1,
        }
    }
    out.dedup();
    out
}

/// Partial dotted path typed after `.props.` (for shared-prop completion).
pub fn props_partial(line_before: &str) -> Option<String> {
    let at = line_before.rfind(".props.")? + ".props.".len();
    let tail = &line_before[at..];
    if tail
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '.')
    {
        Some(tail.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_shared_props() {
        let root = std::env::temp_dir().join(format!("e-inertia-{}", std::process::id()));
        let dir = root.join("app/Http/Middleware");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("HandleInertiaRequests.php"),
            r#"<?php
            class HandleInertiaRequests extends Middleware {
                public function share(Request $request): array {
                    return array_merge(parent::share($request), [
                        'auth' => [
                            'user' => $request->user(),
                        ],
                        'flash' => [
                            'message' => fn () => $request->session()->get('message'),
                        ],
                        'appName' => config('app.name'),
                    ]);
                }
                public function other() { return ['nope' => 1]; }
            }"#,
        )
        .unwrap();
        let props = shared_props(&root);
        assert!(props.contains(&"auth".to_string()));
        assert!(props.contains(&"auth.user".to_string()));
        assert!(props.contains(&"flash.message".to_string()));
        assert!(props.contains(&"appName".to_string()));
        // Must stop at the props array — `other()` keys are excluded.
        assert!(!props.contains(&"nope".to_string()));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn detects_props_partial() {
        assert_eq!(
            props_partial("$page.props.auth.us").as_deref(),
            Some("auth.us")
        );
        assert_eq!(props_partial("usePage().props.").as_deref(), Some(""));
        assert!(props_partial("const x = foo(").is_none());
    }

    #[test]
    fn finds_render_name_at_cursor() {
        let t = "return Inertia::render('Users/Index', ['x' => 1]);";
        let idx = t.find("Users").unwrap() + 2;
        assert_eq!(render_at(t, idx).as_deref(), Some("Users/Index"));
        assert!(render_at(t, 0).is_none());
    }

    #[test]
    fn detects_partial() {
        assert_eq!(
            render_partial("Inertia::render('Users/Ind").as_deref(),
            Some("Users/Ind")
        );
        assert_eq!(render_partial("return inertia(\"").as_deref(), Some(""));
        assert!(render_partial("Inertia::render('Users/Index')").is_none());
    }
}
