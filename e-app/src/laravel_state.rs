//! Laravel / project-intelligence panel state: semantic search, the Eloquent
//! relationship graph, schema diff and the log tail.
//!
//! Views live in their `*_view` modules; this owns the driving `AppState`
//! methods. Extracted from the former `state.rs` god-module (fields stay on
//! `AppState`); same pattern as [`crate::debug`] / [`crate::runtime`].

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use floem::ext_event::create_ext_action;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};

use crate::state::AppState;

impl AppState {
    // ---- Semantic search -----------------------------------------------

    pub fn toggle_semantic_search(&self) {
        let open = !self.sem_open.get_untracked();
        self.sem_open.set(open);
        if open && self.sem_index.get_untracked().borrow().is_empty() {
            self.build_semantic_index();
        }
    }

    /// Build the project index in the background (chunks + embeddings if Ollama
    /// is available, otherwise a lexical index).
    pub fn build_semantic_index(&self) {
        let roots = self.roots.get_untracked();
        let status = self.sem_status;
        let idx_sig = self.sem_index;
        status.set("Indexing project…".to_string());
        let cx = self.cx;
        let send = create_ext_action(cx, move |index: crate::semantic::SemIndex| {
            let n = index.chunks.len();
            let mode = if index.semantic() {
                "semantic"
            } else {
                "lexical"
            };
            status.set(format!("Ready · {n} chunks · {mode}"));
            idx_sig.set(Rc::new(RefCell::new(index)));
        });
        std::thread::spawn(move || {
            let chunks = crate::semantic::build_chunks(&roots);
            let mut embeds = Vec::new();
            let mut model = String::new();
            if crate::semantic::ollama_up() {
                let m = crate::semantic::embed_model();
                let mut ok = true;
                for batch in chunks.chunks(64) {
                    let texts: Vec<String> = batch.iter().map(|c| c.text.clone()).collect();
                    match crate::semantic::embed_batch(&m, &texts) {
                        Some(mut v) => embeds.append(&mut v),
                        None => {
                            ok = false;
                            break;
                        }
                    }
                }
                if ok && embeds.len() == chunks.len() {
                    model = m;
                } else {
                    embeds.clear();
                }
            }
            send(crate::semantic::SemIndex {
                chunks,
                embeds,
                model,
            });
        });
    }

    /// Run the current semantic query against the index.
    pub fn run_semantic_search(&self) {
        let query = self.sem_query.get_untracked();
        if query.trim().is_empty() {
            return;
        }
        let index_rc = self.sem_index.get_untracked();
        if index_rc.borrow().is_empty() {
            self.sem_status
                .set("Building index — try again shortly…".to_string());
            self.build_semantic_index();
            return;
        }
        let results = self.sem_results;
        if index_rc.borrow().semantic() {
            // Embed the query off-thread, then rank on the UI thread.
            let model = index_rc.borrow().model.clone();
            let idx_sig = self.sem_index;
            let q = query.clone();
            let send = create_ext_action(self.cx, move |qvec: Option<Vec<f32>>| {
                let Some(qvec) = qvec else {
                    return;
                };
                let index = idx_sig.get_untracked();
                let index = index.borrow();
                let scores: Vec<f32> = index
                    .embeds
                    .iter()
                    .map(|e| crate::semantic::cosine(&qvec, e))
                    .collect();
                results.set(crate::semantic::top_hits(&index, &scores, 40));
            });
            std::thread::spawn(move || {
                send(crate::semantic::embed_one(&model, &q));
            });
        } else {
            let index = index_rc.borrow();
            let scores = crate::semantic::lexical_scores(&index.chunks, &query);
            results.set(crate::semantic::top_hits(&index, &scores, 40));
        }
    }

    pub fn open_semantic_hit(&self, hit: &crate::semantic::SemHit) {
        let uri = format!("file://{}", hit.path.display());
        self.jump_to(&uri, hit.line.saturating_sub(1), 0);
        self.sem_open.set(false);
    }

    // ---- Eloquent relationship graph -----------------------------------

    pub fn toggle_relations(&self) {
        let open = !self.rel_open.get_untracked();
        self.rel_open.set(open);
        if open {
            self.compute_relations();
        }
    }

    /// Parse model relationships and cross-check them against the live schema's
    /// foreign keys, in the background.
    pub fn compute_relations(&self) {
        let root = self.root.get_untracked();
        let sig = self.rel_graph;
        let send = create_ext_action(self.cx, move |g: Vec<crate::relations::ModelNode>| {
            sig.set(g)
        });
        std::thread::spawn(move || {
            let fks = e_db::from_env(&root)
                .and_then(|cfg| e_db::connect(&cfg).ok())
                .and_then(|conn| e_db::foreign_keys(&conn).ok())
                .unwrap_or_default();
            send(crate::relations::build_graph(&root, &fks));
        });
    }

    // ---- Schema diff ---------------------------------------------------

    /// Diff the project's migrations against the live database schema.
    pub fn compute_schema_diff(&self) {
        let root = self.root.get_untracked();
        self.schema_diff_open.set(true);
        let sig = self.schema_diff;
        let send = create_ext_action(self.cx, move |rows: Vec<crate::schema_diff::DiffRow>| {
            sig.set(rows)
        });
        std::thread::spawn(move || {
            let expected = crate::schema_diff::parse_migrations(&root.join("database/migrations"));
            let mut actual: std::collections::HashMap<String, std::collections::HashSet<String>> =
                std::collections::HashMap::new();
            if let Some(cfg) = e_db::from_env(&root) {
                if let Ok(conn) = e_db::connect(&cfg) {
                    if let Ok(tables) = e_db::tables(&conn) {
                        for t in tables {
                            if let Ok(cols) = e_db::columns(&conn, &t) {
                                actual.insert(t, cols.into_iter().map(|c| c.name).collect());
                            }
                        }
                    }
                }
            }
            send(crate::schema_diff::diff(&expected, &actual));
        });
    }

    // ---- Laravel log tail ----------------------------------------------

    pub fn toggle_laravel_log(&self) {
        let open = !self.log_open.get_untracked();
        self.log_open.set(open);
        if open {
            self.refresh_laravel_log();
        }
    }

    /// Read the tail of the project's Laravel log (off the UI thread).
    pub fn refresh_laravel_log(&self) {
        let root = self.root.get_untracked();
        let sig = self.log_lines;
        let send = create_ext_action(self.cx, move |lines: Vec<String>| sig.set(lines));
        std::thread::spawn(move || {
            let lines = find_laravel_log(&root)
                .map(|p| tail_lines(&p, 64 * 1024, 600))
                .unwrap_or_default();
            send(lines);
        });
    }

    /// Send the recent log tail to the agent for diagnosis.
    pub fn log_fix_with_agent(&self) {
        let tail: String = self.log_lines.with_untracked(|l| {
            l.iter()
                .rev()
                .take(60)
                .rev()
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        });
        if !tail.trim().is_empty() {
            self.send_to_agent(&format!(
                "Diagnose and fix this from the Laravel log. Use propose_edit for changes.\n{tail}"
            ));
        }
    }
}

/// Locate the active Laravel log file (single or the newest daily file).
fn find_laravel_log(root: &std::path::Path) -> Option<PathBuf> {
    let dir = root.join("storage").join("logs");
    let single = dir.join("laravel.log");
    if single.is_file() {
        return Some(single);
    }
    // Newest *.log by modified time (daily logs).
    std::fs::read_dir(&dir)
        .ok()?
        .filter_map(|e| e.ok())
        .fold(None::<(std::time::SystemTime, PathBuf)>, |best, e| {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("log") {
                return best;
            }
            let m = e.metadata().and_then(|m| m.modified()).ok();
            match (best, m) {
                (Some((bt, _bp)), Some(mt)) if mt > bt => Some((mt, p)),
                (None, Some(mt)) => Some((mt, p)),
                (b, _) => b,
            }
        })
        .map(|(_, p)| p)
}

/// Read the last `max` lines from the final `bytes` of a (possibly huge) file.
fn tail_lines(path: &std::path::Path, bytes: u64, max: usize) -> Vec<String> {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(bytes);
    let _ = f.seek(SeekFrom::Start(start));
    let mut buf = Vec::new();
    let _ = f.read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf);
    let all: Vec<&str> = text.lines().collect();
    let from = all.len().saturating_sub(max);
    all[from..].iter().map(|s| s.to_string()).collect()
}
