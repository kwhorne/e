//! Corpus smoke test: run the heuristic parsers over *real* Laravel projects and
//! assert only "no panic, sane counts". The unit tests exercise happy paths; wild
//! PHP in the field is what breaks regex/heuristic parsers, so this walks whole
//! projects and just checks nothing blows up.
//!
//! Ignored by default (needs checkouts). The CI job clones a few projects and
//! points `E_CORPUS_DIR` at the parent directory, then runs:
//!
//! ```sh
//! E_CORPUS_DIR=/path/to/projects cargo test -p e-app corpus -- --ignored --nocapture
//! ```

#![cfg(test)]

use std::path::{Path, PathBuf};

/// Each immediate subdirectory of `E_CORPUS_DIR` is treated as a project root.
fn corpus_projects() -> Vec<PathBuf> {
    let Ok(dir) = std::env::var("E_CORPUS_DIR") else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect()
}

/// Collect source files worth parsing, skipping vendored / generated trees.
fn source_files(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
        if depth > 8 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let path = e.path();
            let name = e.file_name();
            let name = name.to_string_lossy();
            if path.is_dir() {
                if matches!(
                    name.as_ref(),
                    "vendor" | "node_modules" | ".git" | "storage" | "public" | "dist"
                ) {
                    continue;
                }
                walk(&path, out, depth + 1);
            } else if matches!(
                path.extension().and_then(|x| x.to_str()),
                Some("php" | "vue" | "js" | "ts")
            ) {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, &mut out, 0);
    out
}

#[test]
#[ignore = "requires E_CORPUS_DIR with real Laravel projects (set by CI)"]
fn parsers_survive_real_projects() {
    let projects = corpus_projects();
    assert!(
        !projects.is_empty(),
        "E_CORPUS_DIR is unset or contains no project directories"
    );

    for root in projects {
        // Project-wide scans (routes/views/config, model + event graphs).
        let data = crate::laravel::load(&root);
        let models = crate::relations::build_graph(&root, &[]);
        let events = crate::events::dispatch_map(&root);

        // Per-file heuristics over every source file.
        let files = source_files(&root);
        for path in &files {
            let Ok(src) = std::fs::read_to_string(path) else {
                continue;
            };
            match path.extension().and_then(|x| x.to_str()) {
                Some("php") => {
                    let _ = crate::livewire::properties(&src);
                    let _ = crate::contract::component_props(&src);
                }
                Some("vue" | "js" | "ts") => {
                    let _ = crate::contract::component_props(&src);
                }
                _ => {}
            }
        }

        eprintln!(
            "{}: routes={} views={} components={} models={} events={} files={}",
            root.file_name().unwrap_or_default().to_string_lossy(),
            data.routes.len(),
            data.views.len(),
            data.components.len(),
            models.len(),
            events.len(),
            files.len(),
        );

        // Sanity: a real project should have parsed *some* source files. We keep
        // the bar low on purpose — the point is "didn't panic / didn't hang",
        // not exact counts against wild real-world code.
        assert!(
            !files.is_empty(),
            "no source files found under {} — is it a real project?",
            root.display()
        );
    }
}
