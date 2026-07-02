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

/// If `offset` sits inside an `Inertia::render('NAME')` / `inertia('NAME')`
/// string literal, return the page name.
pub fn render_at(text: &str, offset: usize) -> Option<String> {
    for needle in ["Inertia::render(", "inertia("] {
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

/// If the cursor line is inside an unclosed `Inertia::render('…` value, return
/// the partial typed so far (for completion).
pub fn render_partial(line_before: &str) -> Option<String> {
    for needle in ["Inertia::render(", "inertia("] {
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

#[cfg(test)]
mod tests {
    use super::*;

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
