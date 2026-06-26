//! Minimal language detection by file extension.
//!
//! This is the seed for Milepæl 3 (tree-sitter highlighting) and Milepæl 4
//! (LSP). For now it just classifies buffers so we can show the language in
//! the status bar and pick a grammar later.

use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Language {
    PlainText,
    Rust,
    Toml,
    Json,
    Markdown,
    JavaScript,
    TypeScript,
    Python,
    Html,
    Css,
    Shell,
    C,
    Cpp,
    Go,
}

impl Language {
    /// Detect a language from a path's extension (and a few special names).
    pub fn from_path(path: &Path) -> Self {
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            match name {
                "Cargo.lock" => return Language::Toml,
                "Makefile" => return Language::Shell,
                _ => {}
            }
        }

        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();

        match ext.as_str() {
            "rs" => Language::Rust,
            "toml" => Language::Toml,
            "json" => Language::Json,
            "md" | "markdown" => Language::Markdown,
            "js" | "mjs" | "cjs" | "jsx" => Language::JavaScript,
            "ts" | "tsx" => Language::TypeScript,
            "py" | "pyi" => Language::Python,
            "html" | "htm" => Language::Html,
            "css" => Language::Css,
            "sh" | "bash" | "zsh" => Language::Shell,
            "c" | "h" => Language::C,
            "cc" | "cpp" | "cxx" | "hpp" => Language::Cpp,
            "go" => Language::Go,
            _ => Language::PlainText,
        }
    }

    /// Human-readable name for the status bar.
    pub fn name(self) -> &'static str {
        match self {
            Language::PlainText => "Plain Text",
            Language::Rust => "Rust",
            Language::Toml => "TOML",
            Language::Json => "JSON",
            Language::Markdown => "Markdown",
            Language::JavaScript => "JavaScript",
            Language::TypeScript => "TypeScript",
            Language::Python => "Python",
            Language::Html => "HTML",
            Language::Css => "CSS",
            Language::Shell => "Shell",
            Language::C => "C",
            Language::Cpp => "C++",
            Language::Go => "Go",
        }
    }
}

impl Default for Language {
    fn default() -> Self {
        Language::PlainText
    }
}
