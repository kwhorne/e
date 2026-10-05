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

/// Files larger than this are refused rather than opened: the whole text is
/// held in memory several times over (rope, undo tree, language servers), and a
/// 300 MB `laravel.log` would freeze the window. A pager reads those better.
pub const MAX_OPEN_BYTES: u64 = 64 * 1024 * 1024;

/// Encoding label for UTF-8 with a byte-order mark. The mark is stripped on read
/// and written back on save, so a file keeps the bytes it was opened with.
pub const UTF8_BOM: &str = "UTF-8 BOM";

/// Why a path should not be opened as text, if it shouldn't be.
///
/// Reads at most the first 8 KB. A missing file is fine (opening a path that
/// doesn't exist yet creates it on save).
pub fn check_openable(path: &Path) -> std::result::Result<(), String> {
    use std::io::Read;
    let Ok(meta) = std::fs::metadata(path) else {
        return Ok(());
    };
    if meta.is_dir() {
        return Err("it is a folder".into());
    }
    if meta.len() > MAX_OPEN_BYTES {
        return Err(format!(
            "it is {} MB; e opens files up to {} MB",
            meta.len() / (1024 * 1024),
            MAX_OPEN_BYTES / (1024 * 1024)
        ));
    }
    let mut head = vec![0u8; 8192];
    let n = std::fs::File::open(path)
        .and_then(|mut f| f.read(&mut head))
        .map_err(|e| e.to_string())?;
    if looks_binary(&head[..n]) {
        return Err("it looks like a binary file".into());
    }
    Ok(())
}

/// A NUL byte in the first block means binary (SQLite, images, archives) —
/// unless a UTF-16 byte-order mark explains it. Opening one as text and saving
/// it back would corrupt it.
pub fn looks_binary(head: &[u8]) -> bool {
    if head.starts_with(&[0xFF, 0xFE]) || head.starts_with(&[0xFE, 0xFF]) {
        return false;
    }
    head.contains(&0)
}

/// Read a file, decoding it from a detected encoding. Returns the UTF-8 text and
/// the encoding label (e.g. `UTF-8`, `UTF-8 BOM`, `windows-1252`, `UTF-16LE`).
pub fn read_with_encoding(path: &Path) -> Result<(String, String)> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok((String::new(), "UTF-8".to_string()))
        }
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    Ok(decode(&bytes))
}

/// Decode file bytes into UTF-8 text plus the label to write them back with.
pub fn decode(bytes: &[u8]) -> (String, String) {
    // Honour a byte-order mark if present (`decode` strips it).
    if let Some((enc, _)) = encoding_rs::Encoding::for_bom(bytes) {
        let (text, _, _) = enc.decode(bytes);
        let label = if enc == encoding_rs::UTF_8 {
            UTF8_BOM.to_string()
        } else {
            enc.name().to_string()
        };
        return (text.into_owned(), label);
    }
    // Plain UTF-8?
    if let Ok(s) = std::str::from_utf8(bytes) {
        return (s.to_string(), "UTF-8".to_string());
    }
    // Fall back to Windows-1252, which maps every byte.
    let enc = encoding_rs::WINDOWS_1252;
    let (text, _, _) = enc.decode(bytes);
    (text.into_owned(), enc.name().to_string())
}

/// Encode UTF-8 text back into the bytes of `encoding` (a label from
/// [`decode`]). Fails rather than write a lossy file: encoding_rs would turn a
/// character windows-1252 lacks into an `&#…;` reference, silently changing the
/// source, and its UTF-16 "encoder" produces UTF-8.
pub fn encode(contents: &str, encoding: &str) -> Result<Vec<u8>> {
    if encoding.is_empty() || encoding.eq_ignore_ascii_case("UTF-8") {
        return Ok(contents.as_bytes().to_vec());
    }
    if encoding == UTF8_BOM {
        let mut out = Vec::with_capacity(contents.len() + 3);
        out.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
        out.extend_from_slice(contents.as_bytes());
        return Ok(out);
    }
    if encoding.eq_ignore_ascii_case("UTF-16LE") || encoding.eq_ignore_ascii_case("UTF-16BE") {
        let le = encoding.eq_ignore_ascii_case("UTF-16LE");
        let mut out = Vec::with_capacity(contents.len() * 2 + 2);
        let bom: u16 = 0xFEFF;
        for unit in std::iter::once(bom).chain(contents.encode_utf16()) {
            out.extend_from_slice(&if le {
                unit.to_le_bytes()
            } else {
                unit.to_be_bytes()
            });
        }
        return Ok(out);
    }
    let Some(enc) = encoding_rs::Encoding::for_label(encoding.as_bytes()) else {
        anyhow::bail!("unknown encoding {encoding}");
    };
    let (bytes, _, had_errors) = enc.encode(contents);
    if had_errors {
        anyhow::bail!(
            "the file is {encoding}, which cannot represent some of its characters; \
             nothing was written"
        );
    }
    Ok(bytes.into_owned())
}

/// Write `contents` to `path`, re-encoding from UTF-8 into `encoding`.
pub fn write_with_encoding(path: &Path, contents: &str, encoding: &str) -> Result<()> {
    let bytes = encode(contents, encoding).with_context(|| format!("saving {}", path.display()))?;
    write_bytes(path, &bytes)
}

/// Write `contents` to `path`, creating parent directories as needed.
pub fn write(path: &Path, contents: &str) -> Result<()> {
    write_bytes(path, contents.as_bytes())
}

/// Write `bytes` to `path` atomically, creating parent directories as needed.
pub fn write_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
    }
    write_atomic(path, bytes).with_context(|| format!("writing {}", path.display()))
}

/// Replace a file's contents so that a crash, power loss or full disk midway
/// leaves either the old file or the new one — never a truncated one.
///
/// The bytes go to a temporary file beside the target, are flushed to disk, take
/// the original's permissions, and are renamed over it. A symlink is followed so
/// the link survives; a file with several hard links is written in place, since
/// a rename would split it from its other names. Where the directory refuses the
/// temporary file, it falls back to an ordinary in-place write.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;

    let target = match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => {
            std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
        }
        _ => path.to_path_buf(),
    };
    let existing = std::fs::metadata(&target).ok();
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if existing.as_ref().is_some_and(|m| m.nlink() > 1) {
            return std::fs::write(&target, bytes);
        }
    }
    let (Some(dir), Some(name)) = (target.parent(), target.file_name()) else {
        return std::fs::write(&target, bytes);
    };
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = dir.join(format!(
        ".{}.e-save-{}-{seq}",
        name.to_string_lossy(),
        std::process::id()
    ));
    let staged = (|| {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        if let Some(m) = &existing {
            std::fs::set_permissions(&tmp, m.permissions())?;
        }
        std::fs::rename(&tmp, &target)
    })();
    match staged {
        Ok(()) => {
            // Make the rename itself durable. Best effort: not every
            // filesystem lets a directory be opened and synced.
            #[cfg(unix)]
            if let Ok(d) = std::fs::File::open(dir) {
                let _ = d.sync_all();
            }
            Ok(())
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            // Out of space is real: an in-place write would only truncate the
            // file. Anything else (a read-only directory around a writable
            // file) gets the plain write.
            if e.raw_os_error() == Some(28) {
                return Err(e);
            }
            std::fs::write(&target, bytes)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("e-buffer-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn utf8_bom_survives_a_round_trip() {
        let bytes = b"\xEF\xBB\xBF<?php echo 1;\n";
        let (text, label) = decode(bytes);
        assert_eq!(text, "<?php echo 1;\n");
        assert_eq!(label, UTF8_BOM);
        assert_eq!(encode(&text, &label).unwrap(), bytes);
    }

    #[test]
    fn utf16_is_written_as_utf16() {
        for (label, bytes) in [
            ("UTF-16LE", vec![0xFF, 0xFE, b'h', 0, 0xE5, 0]),
            ("UTF-16BE", vec![0xFE, 0xFF, 0, b'h', 0, 0xE5]),
        ] {
            let (text, got) = decode(&bytes);
            assert_eq!(text, "hå");
            assert_eq!(got, label);
            assert_eq!(encode(&text, &got).unwrap(), bytes);
        }
    }

    #[test]
    fn plain_utf8_and_windows_1252_round_trip() {
        let (text, label) = decode("blåbær".as_bytes());
        assert_eq!(label, "UTF-8");
        assert_eq!(encode(&text, &label).unwrap(), "blåbær".as_bytes());

        let latin = b"bl\xE5b\xE6r";
        let (text, label) = decode(latin);
        assert_eq!(text, "blåbær");
        assert_eq!(label, "windows-1252");
        assert_eq!(encode(&text, &label).unwrap(), latin);
    }

    #[test]
    fn unrepresentable_characters_refuse_to_write() {
        assert!(encode("arrow → here", "windows-1252").is_err());
    }

    #[test]
    fn binary_files_are_recognised() {
        assert!(looks_binary(b"SQLite format 3\0\x10\0"));
        assert!(!looks_binary(b"<?php\nreturn [];\n"));
        assert!(!looks_binary(&[0xFF, 0xFE, b'a', 0]));
    }

    #[test]
    fn check_openable_refuses_binary_and_accepts_text_and_missing() {
        let dir = scratch("openable");
        let db = dir.join("database.sqlite");
        std::fs::write(&db, b"SQLite format 3\0rest").unwrap();
        assert!(check_openable(&db).is_err());
        let php = dir.join("a.php");
        std::fs::write(&php, "<?php\n").unwrap();
        assert!(check_openable(&php).is_ok());
        assert!(check_openable(&dir.join("new.php")).is_ok());
        assert!(check_openable(&dir).is_err());
    }

    #[test]
    fn atomic_write_replaces_contents_and_leaves_no_temp_files() {
        let dir = scratch("atomic");
        let f = dir.join("a.txt");
        std::fs::write(&f, "old contents that are longer").unwrap();
        write(&f, "new").unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "new");
        let names: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().collect();
        assert_eq!(names.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_keeps_permissions_symlinks_and_hard_links() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("links");
        let script = dir.join("deploy.sh");
        std::fs::write(&script, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        write(&script, "#!/bin/sh\necho hi\n").unwrap();
        let mode = std::fs::metadata(&script).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755);

        let link = dir.join("link.sh");
        std::os::unix::fs::symlink(&script, &link).unwrap();
        write(&link, "via link\n").unwrap();
        assert!(std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read_to_string(&script).unwrap(), "via link\n");

        let hard = dir.join("hard.sh");
        std::fs::hard_link(&script, &hard).unwrap();
        write(&script, "both names\n").unwrap();
        assert_eq!(std::fs::read_to_string(&hard).unwrap(), "both names\n");
    }

    #[test]
    fn write_creates_missing_parent_folders() {
        let dir = scratch("parents");
        let f = dir.join("app/Http/Controllers/New.php");
        write(&f, "<?php\n").unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "<?php\n");
    }
}
