//! Minimal language detection by file extension.
//!
//! This is the seed for Milepæl 3 (tree-sitter highlighting) and Milepæl 4
//! (LSP). For now it just classifies buffers so we can show the language in
//! the status bar and pick a grammar later.

use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
#[derive(Default)]
pub enum Language {
    #[default]
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
    Php,
    Blade,
    Vue,
    Svelte,
}

impl Language {
    /// Detect a language from a path's extension (and a few special names).
    pub fn from_path(path: &Path) -> Self {
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            let lower = name.to_ascii_lowercase();
            // Laravel Blade / Livewire templates: `*.blade.php`.
            if lower.ends_with(".blade.php") {
                return Language::Blade;
            }
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
            "ts" | "tsx" | "mts" | "cts" => Language::TypeScript,
            "py" | "pyi" => Language::Python,
            "html" | "htm" => Language::Html,
            "css" | "scss" | "sass" | "less" | "pcss" | "postcss" => Language::Css,
            "sh" | "bash" | "zsh" => Language::Shell,
            "c" | "h" => Language::C,
            "cc" | "cpp" | "cxx" | "hpp" => Language::Cpp,
            "go" => Language::Go,
            "php" => Language::Php,
            "vue" => Language::Vue,
            "svelte" => Language::Svelte,
            _ => Language::PlainText,
        }
    }

    /// The line-comment token for this language, if it has one.
    pub fn line_comment(self) -> Option<&'static str> {
        use Language::*;
        match self {
            Rust | C | Cpp | Go | JavaScript | TypeScript | Php | Vue | Svelte => Some("//"),
            Python | Shell | Toml => Some("#"),
            // Languages without a line comment (block-only or none).
            PlainText | Json | Markdown | Html | Css | Blade => None,
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
            Language::Php => "PHP",
            Language::Blade => "Blade",
            Language::Vue => "Vue",
            Language::Svelte => "Svelte",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Language;
    use std::path::Path;

    #[test]
    fn detects_by_extension() {
        assert_eq!(Language::from_path(Path::new("a.rs")), Language::Rust);
        assert_eq!(Language::from_path(Path::new("a.php")), Language::Php);
        assert_eq!(
            Language::from_path(Path::new("view.blade.php")),
            Language::Blade
        );
        assert_eq!(Language::from_path(Path::new("a.vue")), Language::Vue);
        assert_eq!(Language::from_path(Path::new("Cargo.lock")), Language::Toml);
        assert_eq!(
            Language::from_path(Path::new("weird.xyz")),
            Language::PlainText
        );
    }
}
