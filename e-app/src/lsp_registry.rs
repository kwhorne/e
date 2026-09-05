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
    /// Command that installs the server, shown when the binary isn't on `PATH`.
    pub install: &'static str,
    /// A missing optional server costs features, never correctness.
    pub optional: bool,
    /// Settings the server asks for over `workspace/configuration`, as the
    /// JSON tree a VS Code `settings.json` would hold. `None` = the defaults.
    pub settings: Option<&'static str>,
}

const INTELEPHENSE: ServerSpec = ServerSpec {
    id: "intelephense",
    program: "intelephense",
    args: &["--stdio"],
    language_id: "php",
    install: "npm i -g intelephense",
    optional: false,
    settings: Some(INTELEPHENSE_SETTINGS),
};

/// Intelephense skips files over 1 MB by default, which in a Laravel project
/// means Composer's class map and — more to the point — the model helper
/// Laravel Idea writes to `vendor/_laravel_ide/`. Five megabytes covers those.
const INTELEPHENSE_SETTINGS: &str = r#"{"intelephense":{"files":{"maxSize":5000000}}}"#;

/// The official Laravel language server (`composer global require laravel/lsp`).
/// Optional: if the binary isn't installed we simply don't get its features.
const LARAVEL_PHP: ServerSpec = ServerSpec {
    id: "laravel-lsp",
    program: "laravel-lsp",
    args: &[],
    language_id: "php",
    install: LARAVEL_INSTALL,
    optional: true,
    settings: None,
};

const LARAVEL_BLADE: ServerSpec = ServerSpec {
    id: "laravel-lsp",
    program: "laravel-lsp",
    args: &[],
    language_id: "blade",
    install: LARAVEL_INSTALL,
    optional: true,
    settings: None,
};

const LARAVEL_INSTALL: &str = "composer global require laravel/lsp";
const CLANGD_INSTALL: &str = "brew install llvm";
const TSSERVER_INSTALL: &str = "npm i -g typescript-language-server typescript";

/// Every server to run for `language`. `laravel` enables the framework server in
/// Laravel projects (it has nothing to say about a plain PHP project).
pub fn server_specs(language: Language, laravel: bool) -> Vec<ServerSpec> {
    let spec = |id, program, args, language_id, install| ServerSpec {
        id,
        program,
        args,
        language_id,
        install,
        optional: false,
        settings: None,
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
        Language::Rust => vec![spec(
            "rust-analyzer",
            "rust-analyzer",
            &[],
            "rust",
            "rustup component add rust-analyzer",
        )],
        Language::C => vec![spec("clangd", "clangd", &[], "c", CLANGD_INSTALL)],
        Language::Cpp => vec![spec("clangd", "clangd", &[], "cpp", CLANGD_INSTALL)],
        Language::TypeScript => vec![spec(
            "tsserver",
            "typescript-language-server",
            &["--stdio"],
            "typescript",
            TSSERVER_INSTALL,
        )],
        Language::JavaScript => vec![spec(
            "tsserver",
            "typescript-language-server",
            &["--stdio"],
            "javascript",
            TSSERVER_INSTALL,
        )],
        Language::Go => vec![spec(
            "gopls",
            "gopls",
            &[],
            "go",
            "go install golang.org/x/tools/gopls@latest",
        )],
        Language::Python => vec![spec(
            "pyright",
            "pyright-langserver",
            &["--stdio"],
            "python",
            "npm i -g pyright",
        )],
        _ => vec![],
    }
}

/// The server with `id` among `language`'s servers, if it has one.
pub fn spec_for(language: Language, laravel: bool, id: &str) -> Option<ServerSpec> {
    server_specs(language, laravel)
        .into_iter()
        .find(|s| s.id == id)
}

/// The server that owns single-answer operations (formatting, rename): the
/// first, i.e. the general-purpose one.
pub fn primary_spec(language: Language, laravel: bool) -> Option<ServerSpec> {
    server_specs(language, laravel).into_iter().next()
}

/// What to tell the user when `spec`'s binary isn't installed: what's missing,
/// that it's optional when it is, and the one command that fixes it.
pub fn missing_message(spec: &ServerSpec) -> String {
    // The command goes last so it can be copied without trailing punctuation.
    let note = if spec.optional {
        " (optional — e works without it, with fewer features)"
    } else {
        ""
    };
    format!(
        "{} is not installed{note}. Install it with: {}",
        spec.program, spec.install
    )
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

    #[test]
    fn every_server_knows_how_to_install_itself() {
        // A missing binary is only actionable if we can name the fix.
        for lang in [
            Language::Php,
            Language::Blade,
            Language::Rust,
            Language::C,
            Language::Cpp,
            Language::TypeScript,
            Language::JavaScript,
            Language::Go,
            Language::Python,
        ] {
            for spec in server_specs(lang, true) {
                assert!(
                    !spec.install.is_empty(),
                    "{} has no install command",
                    spec.id
                );
            }
        }
    }

    #[test]
    fn server_settings_are_valid_json_trees() {
        for spec in server_specs(Language::Php, true) {
            if let Some(s) = spec.settings {
                let v: serde_json::Value = serde_json::from_str(s).expect("valid JSON");
                assert!(v.is_object(), "{}: settings must be a tree", spec.id);
            }
        }
        assert!(INTELEPHENSE.settings.unwrap().contains("maxSize"));
    }

    #[test]
    fn missing_message_names_the_binary_and_the_command() {
        let msg = missing_message(&LARAVEL_PHP);
        assert!(msg.contains("laravel-lsp"), "{msg}");
        assert!(msg.contains("composer global require laravel/lsp"), "{msg}");
        // The Laravel server is optional; say so, so it doesn't read as a fault.
        assert!(msg.contains("optional"), "{msg}");
        // The command must end the line — a trailing period gets copied with it.
        assert!(
            msg.ends_with("composer global require laravel/lsp"),
            "{msg}"
        );

        let required = missing_message(&INTELEPHENSE);
        assert!(!required.contains("optional"), "{required}");
        assert!(required.ends_with("npm i -g intelephense"), "{required}");
    }
}
