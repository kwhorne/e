//! Built-in completion that works without (and alongside) a language server:
//! language keywords, identifiers already present in the buffer, and
//! Laravel/Blade-specific tokens.

use std::collections::HashSet;

use lsp_types::{CompletionItem, CompletionItemKind};

use e_core::language::Language;

/// Maximum number of built-in items to offer.
const MAX_ITEMS: usize = 80;

fn keywords(language: Language) -> &'static [&'static str] {
    use Language::*;
    match language {
        Rust => &[
            "fn", "let", "mut", "const", "static", "struct", "enum", "trait", "impl", "pub", "use",
            "mod", "match", "if", "else", "for", "while", "loop", "return", "break", "continue",
            "self", "Self", "super", "crate", "where", "async", "await", "move", "ref", "dyn",
            "as", "in", "type", "unsafe", "extern", "Option", "Result", "Some", "None", "Ok",
            "Err", "Vec", "String", "Box", "true", "false",
        ],
        Php | Blade => &[
            "function",
            "class",
            "interface",
            "trait",
            "enum",
            "extends",
            "implements",
            "public",
            "private",
            "protected",
            "static",
            "abstract",
            "final",
            "const",
            "readonly",
            "return",
            "if",
            "else",
            "elseif",
            "foreach",
            "for",
            "while",
            "do",
            "switch",
            "case",
            "default",
            "break",
            "continue",
            "namespace",
            "use",
            "new",
            "clone",
            "echo",
            "print",
            "array",
            "list",
            "isset",
            "unset",
            "empty",
            "true",
            "false",
            "null",
            "try",
            "catch",
            "finally",
            "throw",
            "fn",
            "match",
            "yield",
            "global",
            "instanceof",
            "as",
            "and",
            "or",
        ],
        JavaScript | TypeScript | Vue | Svelte => &[
            "function",
            "const",
            "let",
            "var",
            "return",
            "if",
            "else",
            "for",
            "while",
            "do",
            "switch",
            "case",
            "default",
            "break",
            "continue",
            "class",
            "extends",
            "super",
            "import",
            "export",
            "from",
            "default",
            "async",
            "await",
            "new",
            "this",
            "typeof",
            "instanceof",
            "true",
            "false",
            "null",
            "undefined",
            "void",
            "delete",
            "yield",
            "try",
            "catch",
            "finally",
            "throw",
            "interface",
            "type",
            "enum",
            "implements",
            "public",
            "private",
            "readonly",
            "static",
            "as",
            "in",
            "of",
            "keyof",
        ],
        Python => &[
            "def", "class", "return", "if", "elif", "else", "for", "while", "break", "continue",
            "import", "from", "as", "with", "try", "except", "finally", "raise", "lambda", "True",
            "False", "None", "self", "async", "await", "yield", "global", "nonlocal", "pass",
            "assert", "del", "in", "is", "not", "and", "or", "match", "case",
        ],
        Go => &[
            "func",
            "var",
            "const",
            "type",
            "struct",
            "interface",
            "map",
            "chan",
            "package",
            "import",
            "return",
            "if",
            "else",
            "for",
            "range",
            "go",
            "defer",
            "select",
            "switch",
            "case",
            "default",
            "break",
            "continue",
            "fallthrough",
            "nil",
            "true",
            "false",
            "make",
            "new",
            "append",
            "len",
            "cap",
            "error",
            "string",
            "int",
            "bool",
        ],
        C | Cpp => &[
            "int",
            "char",
            "float",
            "double",
            "void",
            "long",
            "short",
            "unsigned",
            "signed",
            "const",
            "static",
            "struct",
            "union",
            "enum",
            "typedef",
            "return",
            "if",
            "else",
            "for",
            "while",
            "do",
            "switch",
            "case",
            "default",
            "break",
            "continue",
            "sizeof",
            "include",
            "define",
            "class",
            "public",
            "private",
            "protected",
            "virtual",
            "namespace",
            "using",
            "template",
            "typename",
            "auto",
            "nullptr",
            "true",
            "false",
            "new",
            "delete",
        ],
        Css => &[
            "display",
            "position",
            "color",
            "background",
            "background-color",
            "margin",
            "padding",
            "border",
            "width",
            "height",
            "min-width",
            "max-width",
            "flex",
            "flex-direction",
            "justify-content",
            "align-items",
            "grid",
            "gap",
            "font-size",
            "font-weight",
            "line-height",
            "text-align",
            "opacity",
            "overflow",
            "z-index",
            "transition",
            "transform",
            "cursor",
            "box-shadow",
            "border-radius",
            "absolute",
            "relative",
            "fixed",
            "flex-grow",
            "inline-block",
        ],
        Json | Toml | Markdown | Html | Shell | PlainText => &[],
        _ => &[],
    }
}

const BLADE_DIRECTIVES: &[&str] = &[
    "@if",
    "@elseif",
    "@else",
    "@endif",
    "@unless",
    "@endunless",
    "@isset",
    "@endisset",
    "@empty",
    "@endempty",
    "@switch",
    "@case",
    "@break",
    "@default",
    "@endswitch",
    "@foreach",
    "@endforeach",
    "@forelse",
    "@endforelse",
    "@for",
    "@endfor",
    "@while",
    "@endwhile",
    "@php",
    "@endphp",
    "@extends",
    "@section",
    "@endsection",
    "@yield",
    "@parent",
    "@show",
    "@include",
    "@includeIf",
    "@includeWhen",
    "@each",
    "@component",
    "@endcomponent",
    "@slot",
    "@endslot",
    "@props",
    "@csrf",
    "@method",
    "@error",
    "@enderror",
    "@auth",
    "@endauth",
    "@guest",
    "@endguest",
    "@can",
    "@cannot",
    "@endcan",
    "@endcannot",
    "@json",
    "@push",
    "@endpush",
    "@stack",
    "@prepend",
    "@once",
    "@endonce",
    "@verbatim",
    "@endverbatim",
    "@vite",
    "@livewire",
    "@livewireStyles",
    "@livewireScripts",
    "@class",
    "@style",
    "@checked",
    "@selected",
    "@disabled",
    "@dd",
    "@dump",
];

const LARAVEL_FACADES: &[&str] = &[
    "Route",
    "DB",
    "Cache",
    "Auth",
    "Session",
    "Request",
    "Response",
    "Config",
    "Log",
    "Mail",
    "Queue",
    "Storage",
    "Validator",
    "Hash",
    "Str",
    "Arr",
    "Schema",
    "Artisan",
    "Event",
    "Gate",
    "URL",
    "Redirect",
    "View",
    "Cookie",
    "Crypt",
    "App",
    "Blade",
    "Bus",
    "Notification",
    "Process",
    "Password",
    "RateLimiter",
    "Http",
    "Number",
    "Date",
    "Context",
    "Pipeline",
    "Vite",
];

/// Collect distinct identifier-like words from `text` that start with `prefix`.
fn buffer_words(text: &str, prefix: &str) -> Vec<String> {
    let pl = prefix.to_lowercase();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut Vec<String>, seen: &mut HashSet<String>| {
        if word.len() > 2 && word.to_lowercase().starts_with(&pl) && *word != prefix {
            if seen.insert(word.clone()) {
                out.push(std::mem::take(word));
            } else {
                word.clear();
            }
        } else {
            word.clear();
        }
    };
    for c in text.chars() {
        if c.is_alphanumeric() || c == '_' {
            word.push(c);
        } else {
            flush(&mut word, &mut out, &mut seen);
        }
        if out.len() >= MAX_ITEMS {
            break;
        }
    }
    flush(&mut word, &mut out, &mut seen);
    out
}

fn item(label: String, kind: CompletionItemKind, detail: Option<&str>) -> CompletionItem {
    CompletionItem {
        label,
        kind: Some(kind),
        detail: detail.map(|d| d.to_string()),
        ..Default::default()
    }
}

fn push(
    out: &mut Vec<CompletionItem>,
    seen: &mut HashSet<String>,
    wl: &str,
    word: &str,
    label: &str,
    kind: CompletionItemKind,
    detail: Option<&str>,
) {
    if label.to_lowercase().starts_with(wl) && label != word && seen.insert(label.to_string()) {
        out.push(item(label.to_string(), kind, detail));
    }
}

/// Built-in completion items for `word` in a `language` buffer.
pub fn items(language: Language, word: &str, text: &str) -> Vec<CompletionItem> {
    // Don't flood the popup before the user has typed something.
    if word.is_empty() {
        return Vec::new();
    }
    let wl = word.to_lowercase();
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<CompletionItem> = Vec::new();

    for kw in keywords(language) {
        push(
            &mut out,
            &mut seen,
            &wl,
            word,
            kw,
            CompletionItemKind::KEYWORD,
            None,
        );
    }

    if matches!(language, Language::Php | Language::Blade) {
        for f in LARAVEL_FACADES {
            push(
                &mut out,
                &mut seen,
                &wl,
                word,
                f,
                CompletionItemKind::CLASS,
                Some("Laravel facade"),
            );
        }
    }
    if matches!(language, Language::Blade | Language::Html) {
        for d in BLADE_DIRECTIVES {
            push(
                &mut out,
                &mut seen,
                &wl,
                word,
                d,
                CompletionItemKind::KEYWORD,
                Some("Blade directive"),
            );
        }
    }

    for w in buffer_words(text, word) {
        push(
            &mut out,
            &mut seen,
            &wl,
            word,
            &w,
            CompletionItemKind::TEXT,
            None,
        );
    }

    out.truncate(MAX_ITEMS);
    out
}
