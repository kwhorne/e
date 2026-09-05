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

/// An action that edits the current file *and* creates another.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileAction {
    pub title: String,
    pub edits: Vec<(usize, usize, String)>,
    /// `(path relative to the project root, contents)`.
    pub new_file: (String, String),
}

/// Actions at `offset` that also create a file.
pub fn file_actions(text: &str, language: Language, offset: usize) -> Vec<FileAction> {
    match language {
        Language::Php => promote_to_form_request(text, offset).into_iter().collect(),
        _ => Vec::new(),
    }
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

/// The end (exclusive) of the bracketed literal starting at `open` (`[` or
/// `(`), skipping strings, or `None` when unbalanced.
fn matching_bracket(text: &str, open: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut in_str: Option<u8> = None;
    let mut i = open;
    while i < bytes.len() {
        let b = bytes[i];
        if let Some(q) = in_str {
            if b == b'\\' {
                i += 2;
                continue;
            }
            if b == q {
                in_str = None;
            }
        } else {
            match b {
                b'\'' | b'"' => in_str = Some(b),
                b'[' | b'(' => depth += 1,
                b']' | b')' => {
                    depth = depth.checked_sub(1)?;
                    if depth == 0 {
                        return Some(i + 1);
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    None
}

fn studly(s: &str) -> String {
    s.split(['_', '-'])
        .filter(|p| !p.is_empty())
        .map(|p| {
            let mut c = p.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// `$request->validate([...])` (or `$this->validate($request, [...])`) in a
/// controller method → a FormRequest class in `app/Http/Requests` holding the
/// rules, the call replaced by `$request->validated()`, and the method's
/// `Request` parameter retyped to it. Laravel Idea's most used intention.
fn promote_to_form_request(text: &str, offset: usize) -> Option<FileAction> {
    let offset = offset.min(text.len());
    // (call_start, array_open, call_end, variable)
    let mut found: Option<(usize, usize, usize, String)> = None;
    let needle = "->validate(";
    let mut search = 0;
    while let Some(rel) = text[search..].find(needle) {
        let arrow = search + rel;
        search = arrow + needle.len();
        let var_start = text[..arrow]
            .char_indices()
            .rev()
            .take_while(|(_, c)| c.is_ascii_alphanumeric() || *c == '_' || *c == '$')
            .last()
            .map(|(i, _)| i);
        let Some(var_start) = var_start else { continue };
        let receiver = &text[var_start..arrow];
        if !receiver.starts_with('$') {
            continue;
        }
        let paren = arrow + needle.len() - 1;
        let Some(call_end) = matching_bracket(text, paren) else {
            continue;
        };
        if offset < var_start || offset > call_end {
            continue;
        }
        let args_start = paren + 1;
        let args = &text[args_start..call_end - 1];
        // `$this->validate($request, [...])`: the array is the second argument.
        let (var, array_open) = if receiver == "$this" {
            let Some(comma) = args.find(',') else {
                continue;
            };
            let first = args[..comma].trim();
            if !first.starts_with('$') {
                continue;
            }
            let Some(arr) = args[comma + 1..].find('[') else {
                continue;
            };
            (first.to_string(), args_start + comma + 1 + arr)
        } else {
            let Some(arr) = args.find('[') else { continue };
            (receiver.to_string(), args_start + arr)
        };
        found = Some((var_start, array_open, call_end, var));
        break;
    }
    let (call_start, array_open, call_end, var) = found?;
    let array_end = matching_bracket(text, array_open)?;
    let inner = text[array_open + 1..array_end - 1].trim();
    if inner.is_empty() {
        return None;
    }

    // Names: `store` in `UserController` → StoreUserRequest.
    let fn_pos = text[..call_start].rfind("function ")?;
    let method: String = text[fn_pos + "function ".len()..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    let class_pos = text.find("class ")?;
    let class: String = text[class_pos + "class ".len()..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    let base = class.strip_suffix("Controller").unwrap_or(&class);
    let request_class = if method == "__invoke" || method.is_empty() {
        format!("{base}Request")
    } else {
        format!("{}{base}Request", studly(&method))
    };

    let mut edits = Vec::new();
    // 1. The call becomes `$request->validated()`.
    edits.push((call_start, call_end, format!("{var}->validated()")));
    // 2. The parameter is retyped, within this method's signature.
    let sig_open = text[fn_pos..].find('(')? + fn_pos;
    let sig_end = matching_bracket(text, sig_open)?;
    let sig = &text[sig_open..sig_end];
    let param = format!("Request {var}");
    if let Some(rel) = sig.find(&param) {
        let at = sig_open + rel;
        // `\Illuminate\Http\Request $request` retypes as a whole.
        let fqn = "\\Illuminate\\Http\\";
        let start = if text[..at].ends_with(fqn) {
            at - fqn.len()
        } else {
            at
        };
        edits.push((start, at + "Request".len(), request_class.clone()));
    }
    // 3. The import, unless already there.
    let import = format!("App\\Http\\Requests\\{request_class}");
    if !text.contains(&format!("use {import};")) {
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
            edits.push((at, at, format!("use {import};\n")));
        }
    }

    // The new class: rules re-indented under `return [`.
    let rules: Vec<String> = inner
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|l| format!("            {l}"))
        .collect();
    let content = format!(
        "<?php\n\nnamespace App\\Http\\Requests;\n\nuse Illuminate\\Foundation\\Http\\FormRequest;\n\n\
class {request_class} extends FormRequest\n{{\n    public function authorize(): bool\n    {{\n        return true;\n    }}\n\n\
    /**\n     * @return array<string, \\Illuminate\\Contracts\\Validation\\ValidationRule|array<mixed>|string>\n     */\n\
    public function rules(): array\n    {{\n        return [\n{}\n        ];\n    }}\n}}\n",
        rules.join("\n")
    );
    Some(FileAction {
        title: format!("Promote to FormRequest ({request_class})"),
        edits,
        new_file: (format!("app/Http/Requests/{request_class}.php"), content),
    })
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

    #[test]
    fn promotes_an_inline_validate_to_a_form_request() {
        let src = "<?php\n\nnamespace App\\Http\\Controllers;\n\nuse App\\Models\\User;\nuse Illuminate\\Http\\Request;\n\nclass UserController extends Controller\n{\n    public function store(Request $request)\n    {\n        $data = $request->validate([\n            'name' => 'required|max:255',\n            'email' => ['required', 'email'],\n        ]);\n        User::create($data);\n    }\n}\n";
        let at = src.find("'email'").unwrap();
        let acts = file_actions(src, Language::Php, at);
        assert_eq!(acts.len(), 1);
        let a = &acts[0];
        assert_eq!(a.title, "Promote to FormRequest (StoreUserRequest)");
        assert_eq!(a.new_file.0, "app/Http/Requests/StoreUserRequest.php");
        let out = apply(src, &a.edits);
        assert!(
            out.contains("public function store(StoreUserRequest $request)"),
            "{out}"
        );
        assert!(out.contains("$data = $request->validated();"));
        assert!(out.contains(
            "use Illuminate\\Http\\Request;\nuse App\\Http\\Requests\\StoreUserRequest;\n"
        ));
        let file = &a.new_file.1;
        assert!(file.contains("class StoreUserRequest extends FormRequest"));
        assert!(file.contains("            'name' => 'required|max:255',\n            'email' => ['required', 'email'],\n        ];"));
        // Outside the call: nothing.
        assert!(file_actions(src, Language::Php, src.find("User::create").unwrap()).is_empty());
    }

    #[test]
    fn promotes_the_this_validate_form_and_invokable_controllers() {
        let src = "<?php\nnamespace App\\Http\\Controllers;\nuse Illuminate\\Http\\Request;\nclass WebhookController extends Controller\n{\n    public function __invoke(\\Illuminate\\Http\\Request $request)\n    {\n        $this->validate($request, ['id' => 'required']);\n    }\n}\n";
        let at = src.find("'id'").unwrap();
        let a = &file_actions(src, Language::Php, at)[0];
        assert_eq!(a.title, "Promote to FormRequest (WebhookRequest)");
        let out = apply(src, &a.edits);
        assert!(out.contains("__invoke(WebhookRequest $request)"), "{out}");
        assert!(out.contains("$request->validated();"));
    }
}
