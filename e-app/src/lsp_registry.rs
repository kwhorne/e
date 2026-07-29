//! Which language servers `e` launches for a given language.
//!
//! A language can have **several** servers — PHP runs `intelephense` for general
//! PHP intelligence *and* the official `laravel-lsp` for framework-aware routes,
//! views, config, env, middleware, Inertia and validation rules. Requests are
//! merged across them (see `AppState::lsp_clients_for`).
//!
//! Pure and unit-tested; the spawning lives in `state.rs`.

use e_core::language::Language;

/// A language server we know how to launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerSpec {
    pub id: &'static str,
    pub program: &'static str,
    pub args: &'static [&'static str],
    /// The LSP `languageId` to announce for documents of this language.
    pub language_id: &'static str,
}

const INTELEPHENSE: ServerSpec = ServerSpec {
    id: "intelephense",
    program: "intelephense",
    args: &["--stdio"],
    language_id: "php",
};

/// The official Laravel language server (`composer global require laravel/lsp`).
/// Optional: if the binary isn't installed we simply don't get its features.
const LARAVEL_PHP: ServerSpec = ServerSpec {
    id: "laravel-lsp",
    program: "laravel-lsp",
    args: &[],
    language_id: "php",
};

const LARAVEL_BLADE: ServerSpec = ServerSpec {
    id: "laravel-lsp",
    program: "laravel-lsp",
    args: &[],
    language_id: "blade",
};

/// Every server to run for `language`. `laravel` enables the framework server in
/// Laravel projects (it has nothing to say about a plain PHP project).
pub fn server_specs(language: Language, laravel: bool) -> Vec<ServerSpec> {
    let spec = |id, program, args, language_id| ServerSpec {
        id,
        program,
        args,
        language_id,
    };
    match language {
        Language::Php => {
            let mut v = vec![INTELEPHENSE];
            if laravel {
                v.push(LARAVEL_PHP);
            }
            v
        }
        // Blade has no general-purpose server; only Laravel understands it.
        Language::Blade => {
            if laravel {
                vec![LARAVEL_BLADE]
            } else {
                vec![]
            }
        }
        Language::Rust => vec![spec("rust-analyzer", "rust-analyzer", &[], "rust")],
        Language::C => vec![spec("clangd", "clangd", &[], "c")],
        Language::Cpp => vec![spec("clangd", "clangd", &[], "cpp")],
        Language::TypeScript => vec![spec(
            "tsserver",
            "typescript-language-server",
            &["--stdio"],
            "typescript",
        )],
        Language::JavaScript => vec![spec(
            "tsserver",
            "typescript-language-server",
            &["--stdio"],
            "javascript",
        )],
        Language::Go => vec![spec("gopls", "gopls", &[], "go")],
        Language::Python => vec![spec(
            "pyright",
            "pyright-langserver",
            &["--stdio"],
            "python",
        )],
        _ => vec![],
    }
}

/// The server that owns single-answer operations (formatting, rename): the
/// first, i.e. the general-purpose one.
pub fn primary_spec(language: Language, laravel: bool) -> Option<ServerSpec> {
    server_specs(language, laravel).into_iter().next()
}

/// LSP `languageId` for a language, or `None` when no server handles it.
pub fn language_id(language: Language, laravel: bool) -> Option<&'static str> {
    primary_spec(language, laravel).map(|s| s.language_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(language: Language, laravel: bool) -> Vec<&'static str> {
        server_specs(language, laravel)
            .into_iter()
            .map(|s| s.id)
            .collect()
    }

    #[test]
    fn php_gets_laravel_server_only_in_laravel_projects() {
        assert_eq!(
            ids(Language::Php, true),
            vec!["intelephense", "laravel-lsp"]
        );
        assert_eq!(ids(Language::Php, false), vec!["intelephense"]);
    }

    #[test]
    fn blade_is_laravel_only() {
        assert_eq!(ids(Language::Blade, true), vec!["laravel-lsp"]);
        assert!(ids(Language::Blade, false).is_empty());
    }

    #[test]
    fn blade_announces_the_blade_language_id() {
        assert_eq!(language_id(Language::Blade, true), Some("blade"));
        assert_eq!(language_id(Language::Blade, false), None);
        assert_eq!(language_id(Language::Php, true), Some("php"));
    }

    #[test]
    fn primary_is_the_general_purpose_server() {
        // Formatting/rename must not go to the framework server.
        assert_eq!(
            primary_spec(Language::Php, true).unwrap().id,
            "intelephense"
        );
    }

    #[test]
    fn other_languages_are_unchanged_by_the_laravel_flag() {
        for lang in [
            Language::Rust,
            Language::Go,
            Language::Python,
            Language::TypeScript,
        ] {
            assert_eq!(ids(lang, true), ids(lang, false));
            assert_eq!(ids(lang, true).len(), 1);
        }
    }

    #[test]
    fn unsupported_language_has_no_server() {
        assert!(server_specs(Language::Markdown, true).is_empty());
        assert_eq!(language_id(Language::Markdown, true), None);
    }
}
