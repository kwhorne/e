//! On-disk document handling: loading a file into memory and writing it back.
//!
//! The in-memory *text* itself lives in Floem's `TextDocument` (a rope) on the
//! UI side. This module only deals with the filesystem boundary so the rest of
//! the app never touches `std::fs` directly.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::language::Language;

/// A file that is (or will be) open in the editor.
#[derive(Debug, Clone)]
pub struct FileInfo {
    /// Absolute path on disk, if the buffer is backed by a file.
    pub path: Option<PathBuf>,
    /// Detected language, used later for syntax highlighting / LSP.
    pub language: Language,
}

impl FileInfo {
    /// A brand-new, untitled buffer.
    pub fn scratch() -> Self {
        Self {
            path: None,
            language: Language::PlainText,
        }
    }

    /// Build [`FileInfo`] for a path (does not read the file).
    pub fn for_path(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let language = Language::from_path(&path);
        Self {
            path: Some(path),
            language,
        }
    }

    /// Short name shown in tabs / titles.
    pub fn display_name(&self) -> String {
        match &self.path {
            Some(p) => p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.to_string_lossy().into_owned()),
            None => "untitled".to_string(),
        }
    }
}

/// Read a file's contents as a UTF-8 string.
///
/// Returns an empty string if the path does not exist yet, so "open a new
/// file that isn't created" works like in most editors.
pub fn read_to_string(path: &Path) -> Result<String> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Read a file, decoding it from a detected encoding. Returns the UTF-8 text and
/// the encoding label (e.g. `UTF-8`, `windows-1252`, `UTF-16LE`).
pub fn read_with_encoding(path: &Path) -> Result<(String, String)> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok((String::new(), "UTF-8".to_string()))
        }
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    // Honour a byte-order mark if present.
    if let Some((enc, _)) = encoding_rs::Encoding::for_bom(&bytes) {
        let (text, _, _) = enc.decode(&bytes);
        return Ok((text.into_owned(), enc.name().to_string()));
    }
    // Plain UTF-8?
    if let Ok(s) = std::str::from_utf8(&bytes) {
        return Ok((s.to_string(), "UTF-8".to_string()));
    }
    // Fall back to Windows-1252, which maps every byte.
    let enc = encoding_rs::WINDOWS_1252;
    let (text, _, _) = enc.decode(&bytes);
    Ok((text.into_owned(), enc.name().to_string()))
}

/// Write `contents` to `path`, re-encoding from UTF-8 into `encoding`.
pub fn write_with_encoding(path: &Path, contents: &str, encoding: &str) -> Result<()> {
    if encoding.is_empty() || encoding.eq_ignore_ascii_case("UTF-8") {
        return write(path, contents);
    }
    let enc = encoding_rs::Encoding::for_label(encoding.as_bytes()).unwrap_or(encoding_rs::UTF_8);
    let (bytes, _, _) = enc.encode(contents);
    std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))
}

/// Write `contents` to `path`, creating parent directories as needed.
pub fn write(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
    }
    std::fs::write(path, contents).with_context(|| format!("writing {}", path.display()))
}
