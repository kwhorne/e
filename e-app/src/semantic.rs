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

// ---- Embeddings kept on disk ----------------------------------------------

/// A chunk's key in the cache: its text, under the model that embedded it.
/// Content-addressed, so a chunk that moved, or a file that was renamed,
/// is still found; one that changed is not.
pub type EmbedKey = [u8; 16];

pub fn embed_key(model: &str, text: &str) -> EmbedKey {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(model.as_bytes());
    h.update([0]);
    h.update(text.as_bytes());
    let mut key = [0; 16];
    key.copy_from_slice(&h.finalize()[..16]);
    key
}

/// Where a project's embeddings under `model` are kept:
/// `~/.config/e/embeddings/<project>-<model>.bin`.
pub fn cache_path(roots: &[PathBuf], model: &str) -> Option<PathBuf> {
    use sha2::{Digest, Sha256};
    let home = std::env::var_os("HOME")?;
    let mut h = Sha256::new();
    for r in roots {
        h.update(r.to_string_lossy().as_bytes());
        h.update([0]);
    }
    let project: String = h.finalize()[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let model: String = model
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    Some(
        PathBuf::from(home)
            .join(".config/e/embeddings")
            .join(format!("{project}-{model}.bin")),
    )
}

const CACHE_MAGIC: &[u8; 6] = b"EEMB1\n";

/// The embeddings kept at `path`; empty when there are none or the file is
/// not one this wrote.
pub fn load_cache(path: &Path) -> HashMap<EmbedKey, Vec<f32>> {
    let mut out = HashMap::new();
    let Ok(bytes) = std::fs::read(path) else {
        return out;
    };
    let Some(rest) = bytes.strip_prefix(CACHE_MAGIC.as_slice()) else {
        return out;
    };
    if rest.len() < 8 {
        return out;
    }
    let dim = u32::from_le_bytes(rest[0..4].try_into().unwrap_or_default()) as usize;
    let count = u32::from_le_bytes(rest[4..8].try_into().unwrap_or_default()) as usize;
    let entry = 16 + dim * 4;
    let body = &rest[8..];
    if dim == 0 || body.len() != count * entry {
        return out;
    }
    for e in body.chunks_exact(entry) {
        let mut key = [0; 16];
        key.copy_from_slice(&e[..16]);
        let v = e[16..]
            .chunks_exact(4)
            .map(|f| f32::from_le_bytes([f[0], f[1], f[2], f[3]]))
            .collect();
        out.insert(key, v);
    }
    out
}

/// Keep these embeddings at `path`, replacing what was there — so the file
/// holds only chunks the project still has. Written beside and renamed into
/// place, so a crash leaves the old file, never half a new one.
pub fn save_cache(path: &Path, entries: &[(EmbedKey, &[f32])]) -> std::io::Result<()> {
    let dim = entries.first().map_or(0, |(_, v)| v.len());
    let entries: Vec<&(EmbedKey, &[f32])> =
        entries.iter().filter(|(_, v)| v.len() == dim).collect();
    let mut out = Vec::with_capacity(14 + entries.len() * (16 + dim * 4));
    out.extend_from_slice(CACHE_MAGIC);
    out.extend_from_slice(&(dim as u32).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for (key, v) in entries {
        out.extend_from_slice(key);
        for f in v.iter() {
            out.extend_from_slice(&f.to_le_bytes());
        }
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, &out)?;
    std::fs::rename(&tmp, path)
}

/// What embedding the chunks came to.
pub struct Embedded {
    /// One per chunk, in order; empty when they couldn't all be embedded.
    pub embeds: Vec<Vec<f32>>,
    /// How many were in the cache.
    pub cached: usize,
    /// Every embedding there is now, made or found, to keep for next time —
    /// also when the model gave out partway, so the next build resumes.
    pub keep: Vec<(EmbedKey, Vec<f32>)>,
}

/// Embed `chunks` under `model`, asking `embed` only for those `cache` lacks,
/// a batch at a time.
pub fn embed_chunks(
    model: &str,
    chunks: &[Chunk],
    mut cache: HashMap<EmbedKey, Vec<f32>>,
    mut embed: impl FnMut(&[String]) -> Option<Vec<Vec<f32>>>,
) -> Embedded {
    let keys: Vec<EmbedKey> = chunks.iter().map(|c| embed_key(model, &c.text)).collect();
    let cached = keys.iter().filter(|k| cache.contains_key(*k)).count();
    // The same text twice is embedded once.
    let mut seen = std::collections::HashSet::new();
    let missing: Vec<usize> = (0..chunks.len())
        .filter(|&i| !cache.contains_key(&keys[i]) && seen.insert(keys[i]))
        .collect();
    let mut complete = true;
    for batch in missing.chunks(64) {
        let texts: Vec<String> = batch.iter().map(|&i| chunks[i].text.clone()).collect();
        match embed(&texts) {
            Some(vs) if vs.len() == batch.len() => {
                for (&i, v) in batch.iter().zip(vs) {
                    cache.insert(keys[i], v);
                }
            }
            _ => {
                complete = false;
                break;
            }
        }
    }
    let embeds: Vec<Vec<f32>> = if complete {
        keys.iter().filter_map(|k| cache.get(k).cloned()).collect()
    } else {
        Vec::new()
    };
    let wanted: std::collections::HashSet<EmbedKey> = keys.iter().copied().collect();
    let keep = cache
        .into_iter()
        .filter(|(k, _)| wanted.contains(k))
        .collect();
    Embedded {
        embeds: if embeds.len() == chunks.len() {
            embeds
        } else {
            Vec::new()
        },
        cached,
        keep,
    }
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
    fn only_chunks_the_cache_lacks_are_embedded_and_the_cache_survives_a_restart() {
        let chunk = |t: &str| Chunk {
            path: PathBuf::from("a.php"),
            line: 1,
            text: t.to_string(),
        };
        let chunks = vec![chunk("one"), chunk("two"), chunk("one"), chunk("three")];
        let mut asked: Vec<String> = Vec::new();
        let fake = |asked: &mut Vec<String>, texts: &[String]| {
            asked.extend(texts.iter().cloned());
            Some(texts.iter().map(|t| vec![t.len() as f32, 1.0]).collect())
        };
        let first = embed_chunks("m", &chunks, HashMap::new(), |t| fake(&mut asked, t));
        assert_eq!(asked, ["one", "two", "three"]);
        assert_eq!(first.cached, 0);
        assert_eq!(first.embeds.len(), 4);
        assert_eq!(first.embeds[2], vec![3.0, 1.0]);

        let dir = std::env::temp_dir().join(format!("e-embeds-{}", std::process::id()));
        let path = dir.join("p-m.bin");
        let keep: Vec<(EmbedKey, &[f32])> =
            first.keep.iter().map(|(k, v)| (*k, v.as_slice())).collect();
        save_cache(&path, &keep).unwrap();
        let cache = load_cache(&path);
        assert_eq!(cache.len(), 3);

        // After a restart, with "two" edited: only the new text is embedded,
        // and the old one is not kept.
        let chunks = vec![chunk("one"), chunk("two!"), chunk("three")];
        asked.clear();
        let second = embed_chunks("m", &chunks, cache, |t| fake(&mut asked, t));
        assert_eq!(asked, ["two!"]);
        assert_eq!(second.cached, 2);
        assert_eq!(second.embeds[1], vec![4.0, 1.0]);
        assert_eq!(second.keep.len(), 3);
        // Another model's embeddings are not this one's.
        asked.clear();
        let other = embed_chunks("n", &chunks, load_cache(&path), |t| fake(&mut asked, t));
        assert_eq!(other.cached, 0);
        assert_eq!(asked.len(), 3);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_model_that_gives_out_keeps_what_it_made() {
        let chunks: Vec<Chunk> = (0..100)
            .map(|i| Chunk {
                path: PathBuf::from("a.php"),
                line: i,
                text: format!("chunk {i}"),
            })
            .collect();
        let mut calls = 0;
        let out = embed_chunks("m", &chunks, HashMap::new(), |texts| {
            calls += 1;
            (calls == 1).then(|| texts.iter().map(|_| vec![1.0]).collect())
        });
        assert!(out.embeds.is_empty());
        assert_eq!(out.keep.len(), 64);
        assert!(load_cache(Path::new("/nonexistent/e-embeds.bin")).is_empty());
    }

    #[test]
    fn tokenizer_drops_noise() {
        assert_eq!(tokenize("Mail::to($user)"), vec!["mail", "to", "user"]);
    }
}
