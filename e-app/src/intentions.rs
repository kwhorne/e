//! Laravel refactorings offered as code actions, computed locally — the kind
//! Laravel Idea calls intention actions. Each is a pure text transform of the
//! buffer around the caret; the editor turns the byte-range edits into a
//! workspace edit and shows them next to the language servers' actions.

use e_core::language::Language;

/// One offered action: a title and the byte-range edits that perform it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalAction {
    pub title: String,
    /// `(start, end, replacement)` in byte offsets of the text as given.
    pub edits: Vec<(usize, usize, String)>,
}

/// Every action that applies at `offset`.
pub fn actions(text: &str, language: Language, offset: usize) -> Vec<LocalAction> {
    let mut out = Vec::new();
    match language {
        Language::Blade => {
            out.extend(blade_braces(text, offset));
            out.extend(string_actions(text, offset));
        }
        Language::Php => {
            out.extend(string_actions(text, offset));
            out.extend(scope_attribute(text, offset));
        }
        _ => {}
    }
    out
}

/// The string literal the caret is in (or right after), as `(start, end)` of
/// its contents, plus the quote character.
fn string_at(text: &str, offset: usize) -> Option<(usize, usize, char)> {
    let offset = offset.min(text.len());
    let line_start = text[..offset].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let line_end = text[offset..]
        .find('\n')
        .map(|i| offset + i)
        .unwrap_or(text.len());
    let line = &text[line_start..line_end];
    let mut open: Option<(usize, char)> = None;
    for (i, ch) in line.char_indices() {
        match open {
            None if ch == '\'' || ch == '"' => open = Some((i, ch)),
            Some((start, q)) if ch == q => {
                let (s, e) = (line_start + start + 1, line_start + i);
                if offset >= s && offset <= e + 1 {
                    return Some((s, e, q));
                }
                open = None;
            }
            _ => {}
        }
    }
    None
}

fn string_actions(text: &str, offset: usize) -> Vec<LocalAction> {
    let Some((s, e, q)) = string_at(text, offset) else {
        return Vec::new();
    };
    let lit = &text[s..e];
    let mut out = Vec::new();

    // `'required|max:255'` → `['required', 'max:255']`
    if lit.contains('|') {
        let parts: Vec<&str> = lit.split('|').map(str::trim).collect();
        let rule_like = |p: &str| {
            let name = p.split(':').next().unwrap_or("");
            !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        };
        if parts.len() > 1 && parts.iter().all(|p| rule_like(p)) {
            let array = format!(
                "[{}]",
                parts
                    .iter()
                    .map(|p| format!("{q}{p}{q}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            out.push(LocalAction {
                title: "Convert validation string to array".into(),
                edits: vec![(s - 1, e + 1, array)],
            });
        }
    }

    // `'UserController@show'` → `[UserController::class, 'show']`
    if let Some((class, method)) = lit.split_once('@') {
        let class_ok = class
            .chars()
            .next()
            .map(|c| c.is_ascii_uppercase())
            .unwrap_or(false)
            && class
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '\\');
        let method_ok = !method.is_empty()
            && method
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_');
        if class_ok && method_ok {
            out.push(LocalAction {
                title: format!("Convert to [{}::class, {q}{method}{q}]", short_class(class)),
                edits: vec![(s - 1, e + 1, format!("[{class}::class, {q}{method}{q}]"))],
            });
        }
    }
    out
}

fn short_class(class: &str) -> &str {
    class.rsplit('\\').next().unwrap_or(class)
}

/// `{{ $x }}` ↔ `{!! $x !!}` for the echo the caret is in.
fn blade_braces(text: &str, offset: usize) -> Vec<LocalAction> {
    let offset = offset.min(text.len());
    let before = &text[..offset];
    let after = &text[offset..];
    // Escaped echo.
    if let Some(open) = before.rfind("{{") {
        if !before[open..].contains("}}") {
            if let Some(close_rel) = after.find("}}") {
                let close = offset + close_rel;
                return vec![LocalAction {
                    title: "Convert {{ }} to {!! !!} (unescaped)".into(),
                    edits: vec![
                        (open, open + 2, "{!!".into()),
                        (close, close + 2, "!!}".into()),
                    ],
                }];
            }
        }
    }
    // Unescaped echo.
    if let Some(open) = before.rfind("{!!") {
        if !before[open..].contains("!!}") {
            if let Some(close_rel) = after.find("!!}") {
                let close = offset + close_rel;
                return vec![LocalAction {
                    title: "Convert {!! !!} to {{ }} (escaped)".into(),
                    edits: vec![
                        (open, open + 3, "{{".into()),
                        (close, close + 3, "}}".into()),
                    ],
                }];
            }
        }
    }
    Vec::new()
}

/// `public function scopeActive(Builder $q)` → `#[Scope] public function active(Builder $q)`,
/// importing `Illuminate\Database\Eloquent\Attributes\Scope` when needed (Laravel 12.6+).
fn scope_attribute(text: &str, offset: usize) -> Vec<LocalAction> {
    let offset = offset.min(text.len());
    let line_start = text[..offset].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let line_end = text[offset..]
        .find('\n')
        .map(|i| offset + i)
        .unwrap_or(text.len());
    let line = &text[line_start..line_end];
    let Some(fn_pos) = line.find("function ") else {
        return Vec::new();
    };
    let after_fn = &line[fn_pos + "function ".len()..];
    let Some(rest) = after_fn.strip_prefix("scope") else {
        return Vec::new();
    };
    let name_len = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .count();
    if name_len == 0 || !rest[..1].chars().all(|c| c.is_ascii_uppercase()) {
        return Vec::new();
    }
    let camel = &rest[..name_len];
    let mut chars = camel.chars();
    let lower = match chars.next() {
        Some(c) => c.to_lowercase().collect::<String>() + chars.as_str(),
        None => return Vec::new(),
    };
    let indent: String = line
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();
    let name_start = line_start + fn_pos + "function ".len();
    let mut edits = vec![
        // The attribute on its own line above, same indentation.
        (
            line_start + indent.len(),
            line_start + indent.len(),
            format!("#[Scope]\n{indent}"),
        ),
        // The method name without its prefix.
        (
            name_start,
            name_start + "scope".len() + name_len,
            lower.clone(),
        ),
    ];
    const IMPORT: &str = "Illuminate\\Database\\Eloquent\\Attributes\\Scope";
    if !text.contains(&format!("use {IMPORT};")) {
        // After the last top-level `use` before the class, else after `namespace`.
        let class_pos = text
            .find("\nclass ")
            .or_else(|| text.find("\nfinal class "))
            .unwrap_or(text.len());
        let head = &text[..class_pos];
        let insert_at = head
            .match_indices("\nuse ")
            .last()
            .map(|(i, _)| {
                head[i + 1..]
                    .find('\n')
                    .map(|n| i + 1 + n + 1)
                    .unwrap_or(head.len())
            })
            .or_else(|| {
                head.find("namespace ")
                    .and_then(|i| head[i..].find('\n').map(|n| i + n + 1))
            });
        if let Some(at) = insert_at {
            edits.push((at, at, format!("use {IMPORT};\n")));
        }
    }
    // Apply order doesn't matter for the caller (it sorts), but keep the edits
    // non-overlapping: the import lands above the class, the rest on this line.
    vec![LocalAction {
        title: format!("Convert scope{camel} to #[Scope] {lower}() (Laravel 12.6+)"),
        edits,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Apply byte-range edits (last-first) to see the result.
    fn apply(text: &str, edits: &[(usize, usize, String)]) -> String {
        let mut edits = edits.to_vec();
        edits.sort_by_key(|e| std::cmp::Reverse(e.0));
        let mut out = text.to_string();
        for (s, e, r) in edits {
            out.replace_range(s..e, &r);
        }
        out
    }

    #[test]
    fn validation_string_becomes_an_array() {
        let src = "$r->validate(['name' => 'required|max:255|unique:users,name']);";
        let at = src.find("max").unwrap();
        let acts = actions(src, Language::Php, at);
        let a = acts
            .iter()
            .find(|a| a.title.contains("validation"))
            .unwrap();
        assert_eq!(
            apply(src, &a.edits),
            "$r->validate(['name' => ['required', 'max:255', 'unique:users,name']]);"
        );
        // A plain string with a pipe that isn't rules is left alone.
        assert!(actions("echo 'a b|c d';", Language::Php, 7).is_empty());
    }

    #[test]
    fn string_controller_becomes_class_syntax() {
        let src = "Route::get('/users', 'App\\\\Http\\\\Controllers\\\\UserController@index');";
        let at = src.find("UserController").unwrap();
        let acts = actions(src, Language::Php, at);
        let a = acts.iter().find(|a| a.title.contains("::class")).unwrap();
        assert_eq!(a.title, "Convert to [UserController::class, 'index']");
        assert_eq!(
            apply(src, &a.edits),
            "Route::get('/users', [App\\\\Http\\\\Controllers\\\\UserController::class, 'index']);"
        );
    }

    #[test]
    fn blade_echo_toggles_escaping() {
        let src = "<p>{{ $post->body }}</p>";
        let at = src.find("body").unwrap();
        let acts = actions(src, Language::Blade, at);
        assert_eq!(acts.len(), 1);
        assert_eq!(apply(src, &acts[0].edits), "<p>{!! $post->body !!}</p>");
        let back = actions("<p>{!! $x !!}</p>", Language::Blade, 6);
        assert_eq!(
            apply("<p>{!! $x !!}</p>", &back[0].edits),
            "<p>{{ $x }}</p>"
        );
        // Outside an echo: nothing.
        assert!(actions(src, Language::Blade, 1).is_empty());
    }

    #[test]
    fn scope_method_becomes_an_attribute_with_its_import() {
        let src = "<?php\n\nnamespace App\\Models;\n\nuse Illuminate\\Database\\Eloquent\\Builder;\nuse Illuminate\\Database\\Eloquent\\Model;\n\nclass Order extends Model\n{\n    public function scopeActive(Builder $query): void\n    {\n        $query->where('active', true);\n    }\n}\n";
        let at = src.find("scopeActive").unwrap();
        let acts = actions(src, Language::Php, at);
        assert_eq!(acts.len(), 1);
        assert!(acts[0].title.contains("#[Scope] active()"));
        let out = apply(src, &acts[0].edits);
        assert!(out.contains("use Illuminate\\Database\\Eloquent\\Model;\nuse Illuminate\\Database\\Eloquent\\Attributes\\Scope;\n"));
        assert!(out.contains("    #[Scope]\n    public function active(Builder $query): void"));
        assert!(!out.contains("scopeActive"));
        // Already imported: no second import.
        let again = actions(
            &out.replace("active(Builder", "scopeRecent(Builder"),
            Language::Php,
            out.find("function").unwrap(),
        );
        assert!(again[0]
            .edits
            .iter()
            .all(|(_, _, r)| !r.contains("use Illuminate")));
    }
}
