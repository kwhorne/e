//! Laying JSON out without reading it into a map: the keys keep their
//! order, and numbers and strings keep the exact text they were written
//! with. `serde_json` sorts an object's keys (the crate is built without
//! `preserve_order`), so formatting through it would change the value
//! shown, and change the data if saved back.

/// `s` indented two spaces a level, if it is a JSON object or array.
pub(crate) fn pretty(s: &str) -> Option<String> {
    let t = s.trim();
    if !(t.starts_with('{') || t.starts_with('[')) || !is_json(t) {
        return None;
    }
    let toks = tokens(t);
    let mut out = String::with_capacity(t.len() * 2);
    let mut depth = 0usize;
    let newline = |out: &mut String, depth: usize| {
        out.push('\n');
        out.push_str(&"  ".repeat(depth));
    };
    for (i, tok) in toks.iter().enumerate() {
        match *tok {
            "{" | "[" => {
                out.push_str(tok);
                // An empty object or array stays `{}` / `[]`.
                if !matches!(toks.get(i + 1), Some(&"}") | Some(&"]")) {
                    depth += 1;
                    newline(&mut out, depth);
                }
            }
            "}" | "]" => {
                if !matches!(toks.get(i.wrapping_sub(1)), Some(&"{") | Some(&"[")) {
                    depth = depth.saturating_sub(1);
                    newline(&mut out, depth);
                }
                out.push_str(tok);
            }
            "," => {
                out.push(',');
                newline(&mut out, depth);
            }
            ":" => out.push_str(": "),
            _ => out.push_str(tok),
        }
    }
    Some(out)
}

/// `s` on one line, with a space after each `,` and `:` when `spaced`
/// (MySQL writes JSON so) and none otherwise. Anything not JSON is left as
/// it is.
pub(crate) fn compact(s: &str, spaced: bool) -> String {
    if !is_json(s) {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    for tok in tokens(s.trim()) {
        out.push_str(tok);
        if spaced && (tok == "," || tok == ":") {
            out.push(' ');
        }
    }
    out
}

/// Whether `s` is JSON.
pub(crate) fn is_json(s: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(s).is_ok()
}

/// The JSON text `s` split into its tokens, whitespace between them
/// dropped: punctuation, strings (escapes and all) and other literals.
fn tokens(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b' ' | b'\t' | b'\n' | b'\r' => i += 1,
            b'{' | b'}' | b'[' | b']' | b',' | b':' => {
                out.push(&s[i..i + 1]);
                i += 1;
            }
            b'"' => {
                let start = i;
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
                i = (i + 1).min(b.len());
                out.push(&s[start..i]);
            }
            _ => {
                let start = i;
                while i < b.len() && !b" \t\n\r{}[],:\"".contains(&b[i]) {
                    i += 1;
                }
                out.push(&s[start..i]);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pretty_keeps_key_order_and_exact_values() {
        let s = r#"{"z": 1.50, "a": [1, {"k": "x, y: \"q\""}], "e": {}, "f": []}"#;
        assert_eq!(
            pretty(s).unwrap(),
            "{\n  \"z\": 1.50,\n  \"a\": [\n    1,\n    {\n      \"k\": \"x, y: \\\"q\\\"\"\n    }\n  ],\n  \"e\": {},\n  \"f\": []\n}"
        );
        // Not an object or array, or not JSON: left alone.
        assert_eq!(pretty("42"), None);
        assert_eq!(pretty("{nope}"), None);
    }

    #[test]
    fn compact_undoes_pretty_in_either_style() {
        let mysql = r#"{"b": [1, 2.0], "a": "x y", "c": null}"#;
        assert_eq!(compact(&pretty(mysql).unwrap(), true), mysql);
        let tight = r#"{"b":[1,2.0],"a":"x y","c":null}"#;
        assert_eq!(compact(&pretty(tight).unwrap(), false), tight);
        assert_eq!(compact("[ ]", false), "[]");
    }

    #[test]
    fn multibyte_text_survives() {
        let s = r#"{"navn":"Ærlig ø å — ✓"}"#;
        assert_eq!(compact(&pretty(s).unwrap(), false), s);
    }
}
