//! Local semantic search over the project.
//!
//! Files are split into overlapping line-windows ("chunks"). If a local
//! [Ollama](https://ollama.com) server is running we embed the chunks and the
//! query with a real embedding model and rank by cosine similarity — genuinely
//! semantic, and fully local/private. Otherwise we fall back to a lexical
//! (token-overlap) score so the feature still works with zero dependencies.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

const WINDOW: usize = 40;
const OVERLAP: usize = 8;
const MAX_FILE_BYTES: u64 = 400_000;
const MAX_CHUNKS: usize = 6000;

#[derive(Clone)]
pub struct Chunk {
    pub path: PathBuf,
    /// 1-based start line.
    pub line: usize,
    pub text: String,
}

#[derive(Clone)]
pub struct SemHit {
    pub path: PathBuf,
    pub line: usize,
    pub snippet: String,
}

/// The built index. Plain (Send) data so it can be produced off-thread.
#[derive(Clone, Default)]
pub struct SemIndex {
    pub chunks: Vec<Chunk>,
    /// Per-chunk embedding vectors (empty when using the lexical fallback).
    pub embeds: Vec<Vec<f32>>,
    /// Embedding model used, or empty for lexical.
    pub model: String,
}

impl SemIndex {
    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }
    pub fn semantic(&self) -> bool {
        !self.embeds.is_empty()
    }
}

fn skip_dir(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | "node_modules"
            | "vendor"
            | "target"
            | "dist"
            | "build"
            | ".next"
            | ".nuxt"
            | ".svelte-kit"
            | "storage"
            | "bootstrap"
            | ".idea"
            | ".vscode"
            | "coverage"
            | "__pycache__"
    ) || name.starts_with('.') && name.len() > 1 && name != ".env"
}

fn is_text_file(path: &Path) -> bool {
    const EXTS: &[&str] = &[
        "php", "blade", "rs", "js", "ts", "jsx", "tsx", "vue", "svelte", "py", "go", "rb", "java",
        "c", "h", "cpp", "hpp", "cs", "css", "scss", "html", "json", "yaml", "yml", "toml", "md",
        "txt", "sql", "sh", "env", "conf",
    ];
    match path.extension().and_then(|e| e.to_str()) {
        Some(e) => EXTS.contains(&e.to_lowercase().as_str()),
        None => path.file_name().and_then(|n| n.to_str()) == Some(".env"),
    }
}

/// Walk the roots and split every text file into overlapping line windows.
pub fn build_chunks(roots: &[PathBuf]) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    let mut queue: Vec<PathBuf> = roots.to_vec();
    while let Some(dir) = queue.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in read.flatten() {
            let path = entry.path();
            let Ok(ft) = entry.file_type() else { continue };
            let name = entry.file_name().to_string_lossy().into_owned();
            if ft.is_dir() {
                if !skip_dir(&name) {
                    queue.push(path);
                }
            } else if ft.is_file() && is_text_file(&path) {
                let too_big = entry
                    .metadata()
                    .map(|m| m.len() > MAX_FILE_BYTES)
                    .unwrap_or(true);
                if too_big {
                    continue;
                }
                if let Ok(text) = std::fs::read_to_string(&path) {
                    chunk_file(&path, &text, &mut chunks);
                }
            }
            if chunks.len() >= MAX_CHUNKS {
                return chunks;
            }
        }
    }
    chunks
}

fn chunk_file(path: &Path, text: &str, out: &mut Vec<Chunk>) {
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return;
    }
    let step = WINDOW.saturating_sub(OVERLAP).max(1);
    let mut start = 0;
    while start < lines.len() {
        let end = (start + WINDOW).min(lines.len());
        let body = lines[start..end].join("\n");
        if body.trim().len() > 8 {
            out.push(Chunk {
                path: path.to_path_buf(),
                line: start + 1,
                text: body,
            });
        }
        if end == lines.len() {
            break;
        }
        start += step;
    }
}

// ---- Ollama embedding backend --------------------------------------------

pub fn embed_model() -> String {
    std::env::var("E_EMBED_MODEL").unwrap_or_else(|_| "nomic-embed-text".to_string())
}

/// Is a local Ollama server reachable?
pub fn ollama_up() -> bool {
    ureq::get("http://localhost:11434/api/tags")
        .timeout(std::time::Duration::from_millis(800))
        .call()
        .is_ok()
}

/// Embed a batch of texts. Returns `None` on any failure (caller falls back).
pub fn embed_batch(model: &str, texts: &[String]) -> Option<Vec<Vec<f32>>> {
    let body = serde_json::json!({ "model": model, "input": texts });
    let resp = ureq::post("http://localhost:11434/api/embed")
        .timeout(std::time::Duration::from_secs(120))
        .send_json(body)
        .ok()?;
    let v: serde_json::Value = resp.into_json().ok()?;
    let arr = v.get("embeddings")?.as_array()?;
    let out: Vec<Vec<f32>> = arr
        .iter()
        .filter_map(|row| {
            row.as_array().map(|xs| {
                xs.iter()
                    .filter_map(|x| x.as_f64().map(|f| f as f32))
                    .collect()
            })
        })
        .collect();
    if out.len() == texts.len() {
        Some(out)
    } else {
        None
    }
}

pub fn embed_one(model: &str, text: &str) -> Option<Vec<f32>> {
    embed_batch(model, std::slice::from_ref(&text.to_string()))?
        .into_iter()
        .next()
}

pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let mut dot = 0.0;
    let mut na = 0.0;
    let mut nb = 0.0;
    for i in 0..a.len().min(b.len()) {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na.sqrt() * nb.sqrt())
    }
}

// ---- Lexical fallback -----------------------------------------------------

fn tokenize(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in s.chars() {
        if c.is_alphanumeric() {
            cur.extend(c.to_lowercase());
        } else if !cur.is_empty() {
            if cur.len() > 1 {
                out.push(std::mem::take(&mut cur));
            } else {
                cur.clear();
            }
        }
    }
    if cur.len() > 1 {
        out.push(cur);
    }
    out
}

/// Rank chunks by IDF-weighted query-token overlap.
pub fn lexical_scores(chunks: &[Chunk], query: &str) -> Vec<f32> {
    let q = tokenize(query);
    if q.is_empty() {
        return vec![0.0; chunks.len()];
    }
    // Document frequency per query token.
    let mut df: HashMap<&str, usize> = HashMap::new();
    let toks: Vec<Vec<String>> = chunks.iter().map(|c| tokenize(&c.text)).collect();
    for tset in &toks {
        for qt in &q {
            if tset.iter().any(|t| t == qt) {
                *df.entry(qt.as_str()).or_default() += 1;
            }
        }
    }
    let n = chunks.len().max(1) as f32;
    toks.iter()
        .map(|tset| {
            let mut score = 0.0;
            for qt in &q {
                let tf = tset.iter().filter(|t| *t == qt).count() as f32;
                if tf > 0.0 {
                    let idf = (n / (1.0 + *df.get(qt.as_str()).unwrap_or(&0) as f32))
                        .ln()
                        .max(0.0);
                    score += (1.0 + tf.ln()) * (idf + 0.5);
                }
            }
            score
        })
        .collect()
}

pub fn snippet(text: &str) -> String {
    text.lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .take(2)
        .collect::<Vec<_>>()
        .join(" · ")
        .chars()
        .take(160)
        .collect()
}

/// Turn `(chunk index, score)` rankings into the top hits.
pub fn top_hits(index: &SemIndex, scores: &[f32], limit: usize) -> Vec<SemHit> {
    let mut idx: Vec<usize> = (0..index.chunks.len()).collect();
    idx.sort_by(|&a, &b| {
        scores[b]
            .partial_cmp(&scores[a])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    idx.into_iter()
        .filter(|&i| scores[i] > 0.0)
        .take(limit)
        .map(|i| {
            let c = &index.chunks[i];
            SemHit {
                path: c.path.clone(),
                line: c.line,
                snippet: snippet(&c.text),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_and_lexical_rank() {
        let mut chunks = Vec::new();
        chunk_file(
            Path::new("a.php"),
            "Mail::to($user)->send(new InvoiceMail($invoice));\nreturn back();",
            &mut chunks,
        );
        chunk_file(
            Path::new("b.php"),
            "public function index() { return view('home'); }",
            &mut chunks,
        );
        let scores = lexical_scores(&chunks, "send invoice email");
        // The invoice/mail chunk should outrank the home view chunk.
        assert!(scores[0] > scores[1]);
    }

    #[test]
    fn tokenizer_drops_noise() {
        assert_eq!(tokenize("Mail::to($user)"), vec!["mail", "to", "user"]);
    }
}
