//! Built-in code snippets. Surfaced in the completion popup; on accept the
//! body is expanded with the caret placed at the `$0` marker.

use lsp_types::{CompletionItem, CompletionItemKind};

use e_core::language::Language;

/// `(prefix, body)`. `$0` marks the final caret position; indentation uses
/// four spaces and continuation lines are re-indented to the current line.
fn snippets(language: Language) -> &'static [(&'static str, &'static str)] {
    match language {
        Language::Rust => &[
            ("fn", "fn $0() {\n    \n}"),
            ("pubfn", "pub fn $0() {\n    \n}"),
            ("test", "#[test]\nfn $0() {\n    \n}"),
            ("impl", "impl $0 {\n    \n}"),
            ("match", "match $0 {\n    \n}"),
            ("for", "for $0 in iter {\n    \n}"),
            ("println", "println!(\"$0\");"),
            ("derive", "#[derive(Debug, Clone)]\n$0"),
        ],
        Language::Php | Language::Blade => &[
            ("fn", "function $0() {\n    \n}"),
            ("pubf", "public function $0() {\n    \n}"),
            ("privf", "private function $0() {\n    \n}"),
            ("class", "class $0 {\n    \n}"),
            ("foreach", "foreach ($0 as $item) {\n    \n}"),
            ("if", "if ($0) {\n    \n}"),
            ("dd", "dd($0);"),
        ],
        Language::JavaScript | Language::TypeScript | Language::Vue | Language::Svelte => &[
            ("fn", "function $0() {\n    \n}"),
            ("arrow", "const $0 = () => {\n    \n}"),
            ("log", "console.log($0);"),
            ("for", "for (let i = 0; i < $0; i++) {\n    \n}"),
            ("if", "if ($0) {\n    \n}"),
        ],
        Language::Python => &[
            ("def", "def $0():\n    "),
            ("class", "class $0:\n    "),
            ("for", "for $0 in iterable:\n    "),
            ("if", "if $0:\n    "),
            ("main", "if __name__ == \"__main__\":\n    $0"),
        ],
        Language::Go => &[
            ("func", "func $0() {\n    \n}"),
            ("for", "for $0 {\n    \n}"),
            ("if", "if $0 {\n    \n}"),
        ],
        Language::C | Language::Cpp => &[
            ("main", "int main() {\n    $0\n    return 0;\n}"),
            ("for", "for (int i = 0; i < $0; i++) {\n    \n}"),
            ("if", "if ($0) {\n    \n}"),
        ],
        _ => &[],
    }
}

/// The body for a snippet prefix, if any.
pub fn body(language: Language, prefix: &str) -> Option<&'static str> {
    snippets(language)
        .iter()
        .find(|(p, _)| *p == prefix)
        .map(|(_, b)| *b)
}

/// Completion items for snippets whose prefix starts with `word`.
pub fn completion_items(language: Language, word: &str) -> Vec<CompletionItem> {
    snippets(language)
        .iter()
        .filter(|(p, _)| word.is_empty() || p.starts_with(word))
        .map(|(p, _)| CompletionItem {
            label: p.to_string(),
            insert_text: Some(p.to_string()),
            kind: Some(CompletionItemKind::SNIPPET),
            detail: Some("snippet".to_string()),
            ..Default::default()
        })
        .collect()
}

/// Expand a snippet body: re-indent continuation lines and locate `$0`.
/// Returns `(expanded_text, caret_offset_within_text)`.
pub fn expand(body: &str, line_indent: &str) -> (String, usize) {
    // Re-indent: lines after the first get the current line's indentation.
    let mut indented = String::new();
    for (i, line) in body.split('\n').enumerate() {
        if i > 0 {
            indented.push('\n');
            indented.push_str(line_indent);
        }
        indented.push_str(line);
    }
    // Locate and strip the `$0` marker; strip any other `$n` markers too.
    let caret = indented.find("$0").unwrap_or(indented.len());
    let mut out = indented.replacen("$0", "", 1);
    // Remove stray ${n}/$n placeholders, keeping the text simple.
    while let Some(pos) = find_placeholder(&out) {
        let end = out[pos..]
            .char_indices()
            .skip(1)
            .find(|(_, c)| !c.is_ascii_digit())
            .map(|(i, _)| pos + i)
            .unwrap_or(out.len());
        out.replace_range(pos..end, "");
    }
    (out, caret)
}

fn find_placeholder(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    for i in 0..bytes.len() {
        if bytes[i] == b'$' && bytes.get(i + 1).is_some_and(|c| c.is_ascii_digit()) {
            return Some(i);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::expand;

    #[test]
    fn expands_and_reindents() {
        let (out, caret) = expand("fn $0() {\n    \n}", "  ");
        assert_eq!(&out[..caret], "fn ");
        assert!(out.starts_with("fn () {\n"));
        // continuation lines re-indented by two spaces
        assert!(out.contains("\n      \n  }"));
        assert!(!out.contains("$0"));
    }

    #[test]
    fn strips_extra_placeholders() {
        let (out, _) = expand("foo($1, $2)$0", "");
        assert_eq!(out, "foo(, )");
    }
}
