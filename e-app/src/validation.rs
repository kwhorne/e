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
}
