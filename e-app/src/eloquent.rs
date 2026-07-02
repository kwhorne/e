//! Eloquent attribute completion from the *live* database schema.
//!
//! When the cursor is at `$user->…`, we infer the model for `$user`, map it to
//! its table, and offer the table's real columns as completions — something
//! Intelephense alone can't do without an ide-helper file.

use std::collections::HashMap;
use std::path::Path;

use e_core::language::Language;
use e_db::ColumnInfo;
use lsp_types::{CompletionItem, CompletionItemKind};

fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Detect `$var->partial` immediately before the cursor.
/// Returns `(var, partial, partial_start_byte)`.
fn member_access(text: &str, offset: usize) -> Option<(String, String, usize)> {
    let upto = &text[..offset.min(text.len())];
    let bytes = upto.as_bytes();
    // Trailing identifier (the partial being typed).
    let mut i = bytes.len();
    while i > 0 && is_word(bytes[i - 1] as char) {
        i -= 1;
    }
    let partial_start = i;
    let partial = upto[partial_start..].to_string();
    // Must be preceded by "->".
    if partial_start < 2 || &upto[partial_start - 2..partial_start] != "->" {
        return None;
    }
    // Before "->": $var
    let mut j = partial_start - 2;
    let end = j;
    while j > 0 && is_word(bytes[j - 1] as char) {
        j -= 1;
    }
    if j == 0 || bytes[j - 1] != b'$' || end == j {
        return None;
    }
    let var = upto[j..end].to_string();
    Some((var, partial, partial_start))
}

/// Infer the model class for `$var` from assignments and type hints above.
fn resolve_model(text: &str, var: &str) -> Option<String> {
    let needle = format!("${var}");
    let mut found: Option<String> = None;
    let mut from = 0;
    while let Some(rel) = text[from..].find(&needle) {
        let pos = from + rel;
        from = pos + needle.len();
        // Whole-variable boundary.
        if text[pos + needle.len()..]
            .chars()
            .next()
            .map(is_word)
            .unwrap_or(false)
        {
            continue;
        }
        // Assignment: `$var = [new] Class`
        let after = text[pos + needle.len()..].trim_start();
        if let Some(rest) = after.strip_prefix('=') {
            let rest = rest.trim_start();
            let rest = rest.strip_prefix("new ").unwrap_or(rest);
            let cls: String = rest
                .chars()
                .take_while(|c| is_word(*c) || *c == '\\')
                .collect();
            let cls = cls.rsplit('\\').next().unwrap_or(&cls).to_string();
            if starts_upper(&cls) {
                found = Some(cls);
            }
        }
        // Type hint: `Class $var`
        let before = text[..pos].trim_end();
        let word: String = before
            .chars()
            .rev()
            .take_while(|c| is_word(*c) || *c == '\\')
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        let word = word.rsplit('\\').next().unwrap_or(&word).to_string();
        if starts_upper(&word) && word != var {
            found = Some(word);
        }
    }
    found
}

fn starts_upper(s: &str) -> bool {
    s.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)
}

/// Map a model class to its table: honour `protected $table`, else the Laravel
/// snake_case + pluralise convention.
fn model_table(root: &Path, class: &str) -> String {
    for cand in [
        root.join(format!("app/Models/{class}.php")),
        root.join(format!("app/{class}.php")),
    ] {
        if let Ok(src) = std::fs::read_to_string(&cand) {
            if let Some(t) = find_table_property(&src) {
                return t;
            }
        }
    }
    pluralize(&snake_case(class))
}

fn find_table_property(src: &str) -> Option<String> {
    let idx = src.find("$table")?;
    let rest = &src[idx..];
    let eq = rest.find('=')?;
    let after = rest[eq + 1..].trim_start();
    let quote = after.chars().next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    let inner = &after[1..];
    let end = inner.find(quote)?;
    Some(inner[..end].to_string())
}

pub(crate) fn snake_case(s: &str) -> String {
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() {
            if i != 0 {
                out.push('_');
            }
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

pub(crate) fn pluralize(s: &str) -> String {
    if s.ends_with('y')
        && !s.ends_with("ay")
        && !s.ends_with("ey")
        && !s.ends_with("oy")
        && !s.ends_with("uy")
    {
        format!("{}ies", &s[..s.len() - 1])
    } else if s.ends_with('s') || s.ends_with('x') || s.ends_with("ch") || s.ends_with("sh") {
        format!("{s}es")
    } else {
        format!("{s}s")
    }
}

/// Column completions for `$model->…`, or `None` if it doesn't apply.
pub fn complete(
    language: Language,
    text: &str,
    offset: usize,
    root: &Path,
    schema: &HashMap<String, Vec<ColumnInfo>>,
) -> Option<(usize, Vec<CompletionItem>)> {
    if !matches!(language, Language::Php | Language::Blade) || schema.is_empty() {
        return None;
    }
    let (var, partial, start) = member_access(text, offset)?;
    let model = resolve_model(&text[..offset.min(text.len())], &var)?;
    let table = model_table(root, &model);
    let cols = schema.get(&table)?;
    let lower = partial.to_lowercase();
    let items: Vec<CompletionItem> = cols
        .iter()
        .filter(|c| lower.is_empty() || c.name.to_lowercase().starts_with(&lower))
        .map(|c| CompletionItem {
            label: c.name.clone(),
            insert_text: Some(c.name.clone()),
            kind: Some(CompletionItemKind::FIELD),
            detail: Some(format!("{} · {}", c.data_type, table)),
            ..Default::default()
        })
        .collect();
    if items.is_empty() {
        return None;
    }
    Some((start, items))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_member_access() {
        let (v, p, _) = member_access("$user->cre", 10).unwrap();
        assert_eq!(v, "user");
        assert_eq!(p, "cre");
        assert!(member_access("$user->", 7).is_some());
        assert!(member_access("foo", 3).is_none());
    }

    #[test]
    fn resolves_model() {
        assert_eq!(
            resolve_model("$user = User::find(1); $user->", "user").as_deref(),
            Some("User")
        );
        assert_eq!(
            resolve_model("public function show(User $user) { $user->", "user").as_deref(),
            Some("User")
        );
    }

    #[test]
    fn pluralizes() {
        assert_eq!(pluralize("user"), "users");
        assert_eq!(pluralize("category"), "categories");
        assert_eq!(pluralize("class"), "classes");
        assert_eq!(snake_case("BlogPost"), "blog_post");
    }
}
