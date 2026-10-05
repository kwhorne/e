//! Validation-rule intelligence: completion for rule names in `validate([…])`
//! / FormRequest `rules()`, and generating rules from the live database schema
//! (nullability, string lengths, types) — something PhpStorm can't do precisely
//! without the database.

use e_db::ColumnInfo;

/// Common Laravel validation rule names for completion.
const RULES: &[&str] = &[
    "required",
    "nullable",
    "sometimes",
    "present",
    "filled",
    "string",
    "integer",
    "numeric",
    "boolean",
    "array",
    "date",
    "email",
    "url",
    "uuid",
    "ulid",
    "json",
    "ip",
    "timezone",
    "min",
    "max",
    "between",
    "size",
    "digits",
    "digits_between",
    "in",
    "not_in",
    "unique",
    "exists",
    "confirmed",
    "same",
    "different",
    "regex",
    "not_regex",
    "alpha",
    "alpha_num",
    "alpha_dash",
    "image",
    "mimes",
    "mimetypes",
    "file",
    "after",
    "after_or_equal",
    "before",
    "before_or_equal",
    "date_format",
    "distinct",
    "gt",
    "gte",
    "lt",
    "lte",
    "starts_with",
    "ends_with",
    "active_url",
    "accepted",
    "declined",
    "lowercase",
    "uppercase",
    "hex_color",
];

pub fn rule_names(partial: &str) -> Vec<&'static str> {
    let lower = partial.to_lowercase();
    RULES
        .iter()
        .copied()
        .filter(|r| lower.is_empty() || r.starts_with(&lower))
        .collect()
}

/// Detect that the cursor is typing a validation rule and return the partial
/// (the segment after the last `|`).
pub fn rule_partial(line_before: &str) -> Option<String> {
    // The unterminated string the cursor is in.
    let bytes = line_before.as_bytes();
    let mut in_str: Option<(char, usize)> = None;
    let mut i = 0;
    while i < line_before.len() {
        let c = bytes[i] as char;
        match in_str {
            Some((q, _)) if c == q => in_str = None,
            Some(_) => {}
            None if c == '\'' || c == '"' => in_str = Some((c, i + 1)),
            None => {}
        }
        i += 1;
    }
    let (_, start) = in_str?;
    let content = &line_before[start..];
    let seg = content.rsplit('|').next().unwrap_or(content);
    // Trigger on a rule pipe, or an array value position (`… => '…`).
    let has_pipe = content.contains('|');
    let before = line_before[..start.saturating_sub(1)].trim_end();
    let array_value = before.ends_with("=>");
    if has_pipe || array_value {
        // Don't fire mid-word for `max:255` (after a colon it's an argument).
        if seg.contains(':') {
            None
        } else {
            Some(seg.trim().to_string())
        }
    } else {
        None
    }
}

/// Whether the caret (at the end of `before`, the document up to it) is
/// where validation rules are written: in a `rules()` method's array, a
/// `$rules = [` property, the arguments of `validate(`, `validateWithBag(` or
/// `Validator::make(`, or a Livewire `#[Validate(…)]` / `#[Rule(…)]`
/// attribute. Without this, any `'key' => '` in a config file or a
/// `create([...])` call offered the rule list.
pub fn in_rules_context(before: &str) -> bool {
    // A bounded look-back: rules sit close to what encloses them.
    let mut from = before.len().saturating_sub(4000);
    while !before.is_char_boundary(from) {
        from += 1;
    }
    let w = &before[from..];
    let b = w.as_bytes();

    // The brackets still open at the caret (outermost first), skipping
    // strings and `//` comments.
    let mut open: Vec<usize> = Vec::new();
    let mut quote: Option<u8> = None;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        match quote {
            Some(q) => {
                if c == b'\\' {
                    i += 1;
                } else if c == q {
                    quote = None;
                }
            }
            None => match c {
                b'\'' | b'"' => quote = Some(c),
                b'/' if b.get(i + 1) == Some(&b'/') => {
                    while i < b.len() && b[i] != b'\n' {
                        i += 1;
                    }
                }
                b'[' | b'(' => open.push(i),
                b']' | b')' => {
                    open.pop();
                }
                _ => {}
            },
        }
        i += 1;
    }

    let in_rules_method = w
        .rfind("function ")
        .is_some_and(|f| w[f + 9..].trim_start().starts_with("rules("));
    open.iter().any(|&at| {
        let lead = w[..at].trim_end();
        match b[at] {
            b'(' => [
                "validate",
                "validateWithBag",
                "Validator::make",
                "#[Validate",
                "#[Rule",
            ]
            .iter()
            .any(|f| lead.ends_with(f)),
            _ => in_rules_method || lead.ends_with("$rules =") || lead.ends_with("$rules="),
        }
    })
}

/// The cursor is typing a rule *parameter* that names a table or a column:
/// `exists:` / `unique:` (table), `exists:users,` (column), and the object
/// forms `Rule::exists('` / `Rule::unique('users', '`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParamCtx {
    Table { partial: String },
    Column { table: String, partial: String },
}

/// Byte offset just after the opening quote of the unterminated string the
/// cursor is in, if any.
fn open_string(line_before: &str) -> Option<usize> {
    let mut in_str: Option<(char, usize)> = None;
    for (i, c) in line_before.char_indices() {
        match in_str {
            Some((q, _)) if c == q => in_str = None,
            Some(_) => {}
            None if c == '\'' || c == '"' => in_str = Some((c, i + 1)),
            None => {}
        }
    }
    in_str.map(|(_, start)| start)
}

pub fn param_context(line_before: &str) -> Option<ParamCtx> {
    let start = open_string(line_before)?;
    let content = &line_before[start..];
    let before = line_before[..start - 1].trim_end();

    // `Rule::exists('users', 'id` / `Rule::unique('`
    for rule in ["Rule::exists(", "Rule::unique("] {
        if let Some(pos) = before.rfind(rule) {
            let args = &before[pos + rule.len()..];
            if args.trim().is_empty() {
                return Some(ParamCtx::Table {
                    partial: content.to_string(),
                });
            }
            // One earlier string argument, then a comma: the column.
            let t = args.trim();
            let t = t.strip_suffix(',')?.trim_end();
            let table = t
                .strip_prefix('\'')
                .and_then(|x| x.strip_suffix('\''))
                .or_else(|| t.strip_prefix('"').and_then(|x| x.strip_suffix('"')))?;
            return Some(ParamCtx::Column {
                table: table.to_string(),
                partial: content.to_string(),
            });
        }
    }

    // `'required|exists:users,id` — the last pipe segment with a colon.
    let seg = content.rsplit('|').next().unwrap_or(content);
    let (rule, rest) = seg.split_once(':')?;
    if !matches!(rule.trim(), "exists" | "unique") {
        return None;
    }
    match rest.split_once(',') {
        None => Some(ParamCtx::Table {
            partial: rest.to_string(),
        }),
        Some((table, after)) if !after.contains(',') => Some(ParamCtx::Column {
            table: table.trim().to_string(),
            partial: after.to_string(),
        }),
        Some(_) => None,
    }
}

/// Generate `'field' => 'rules'` lines from a table's columns.
pub fn generate_rules(table: &str, cols: &[ColumnInfo]) -> String {
    let skip = [
        "id",
        "created_at",
        "updated_at",
        "deleted_at",
        "remember_token",
    ];
    let mut out = String::new();
    for c in cols {
        if skip.contains(&c.name.as_str()) {
            continue;
        }
        let mut rules: Vec<String> = Vec::new();
        rules.push(if c.nullable {
            "nullable".into()
        } else {
            "required".into()
        });
        let t = c.data_type.to_lowercase();
        if c.name == "email" {
            rules.push("email".into());
        } else if t.contains("int") {
            rules.push("integer".into());
        } else if t.contains("bool") || t == "tinyint(1)" {
            rules.push("boolean".into());
        } else if t.contains("decimal")
            || t.contains("float")
            || t.contains("double")
            || t.contains("numeric")
        {
            rules.push("numeric".into());
        } else if t.contains("date") || t.contains("time") || t.contains("timestamp") {
            rules.push("date".into());
        } else if t.contains("json") {
            rules.push("array".into());
        } else {
            rules.push("string".into());
            if let Some(n) = varchar_len(&t) {
                rules.push(format!("max:{n}"));
            }
        }
        let _ = table;
        out.push_str(&format!(
            "            '{}' => '{}',\n",
            c.name,
            rules.join("|")
        ));
    }
    out
}

fn varchar_len(ty: &str) -> Option<u32> {
    let open = ty.find('(')? + 1;
    let close = ty[open..].find(')')? + open;
    ty[open..close].split(',').next()?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_rule_partial() {
        assert_eq!(
            rule_partial("'email' => 'required|str").as_deref(),
            Some("str")
        );
        assert_eq!(rule_partial("'email' => 'req").as_deref(), Some("req"));
        // After a colon (rule argument) we don't complete rule names.
        assert!(rule_partial("'name' => 'max:25").is_none());
        // Plain string, not an array value.
        assert!(rule_partial("$x = 'hello").is_none());
    }

    #[test]
    fn generates_rules_from_columns() {
        let cols = vec![
            ColumnInfo {
                name: "id".into(),
                data_type: "bigint".into(),
                nullable: false,
                key: "PRI".into(),
            },
            ColumnInfo {
                name: "email".into(),
                data_type: "varchar(255)".into(),
                nullable: false,
                key: String::new(),
            },
            ColumnInfo {
                name: "age".into(),
                data_type: "int".into(),
                nullable: true,
                key: String::new(),
            },
        ];
        let r = generate_rules("users", &cols);
        assert!(!r.contains("'id'")); // skipped
        assert!(r.contains("'email' => 'required|email',"));
        assert!(r.contains("'age' => 'nullable|integer',"));
    }

    #[test]
    fn exists_and_unique_parameters_name_tables_then_columns() {
        assert_eq!(
            param_context("'email' => 'required|exists:us"),
            Some(ParamCtx::Table {
                partial: "us".into()
            })
        );
        assert_eq!(
            param_context("'email' => 'required|unique:users,em"),
            Some(ParamCtx::Column {
                table: "users".into(),
                partial: "em".into()
            })
        );
        // A third segment (ignore column) isn't a name we can complete.
        assert_eq!(param_context("'x' => 'unique:users,email,"), None);
        // Object form.
        assert_eq!(
            param_context("Rule::exists('us"),
            Some(ParamCtx::Table {
                partial: "us".into()
            })
        );
        assert_eq!(
            param_context("Rule::unique('users', 'em"),
            Some(ParamCtx::Column {
                table: "users".into(),
                partial: "em".into()
            })
        );
        // Not a table/column rule.
        assert_eq!(param_context("'x' => 'max:2"), None);
    }

    #[test]
    fn rules_are_offered_only_where_rules_are_written() {
        let yes = [
            "class StoreUser extends FormRequest {\n    public function rules(): array\n    {\n        return [\n            'email' => '",
            "class StoreUser extends FormRequest {\n    public function rules(): array\n    {\n        return [\n            'name' => ['required', '",
            "$request->validate([\n    'title' => 'required|",
            "$v = Validator::make($request->all(), [\n    'age' => '",
            "#[Validate('",
            "class Form extends Component {\n    protected $rules = [\n        'body' => '",
        ];
        for before in yes {
            assert!(in_rules_context(before), "should offer rules: {before}");
        }
        let no = [
            "<?php\nreturn [\n    'driver' => '",
            "User::create([\n    'name' => '",
            "return view('users.index', ['title' => '",
            "class StoreUser {\n    public function rules(): array { return []; }\n    public function messages(): array\n    {\n        return [\n            'email.required' => '",
            "$request->validate(['a' => 'required']);\nUser::create(['name' => '",
        ];
        for before in no {
            assert!(
                !in_rules_context(before),
                "should not offer rules: {before}"
            );
        }
    }
}
