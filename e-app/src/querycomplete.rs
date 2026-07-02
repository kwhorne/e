//! Query-builder string completion: column names inside `where('…')`,
//! `orderBy()`, `select()`, `pluck()`, … and relationship names inside
//! `with('…')`, `load()`, `whereHas()`. Reuses the model→table resolution and
//! live schema behind Eloquent completion.

use std::path::Path;

use crate::eloquent::{model_table, resolve_model};

/// Methods whose *first* string argument is a column.
const COL_FIRST: &[&str] = &[
    "where",
    "orwhere",
    "wherenot",
    "wherein",
    "wherenotin",
    "wheredate",
    "whereday",
    "wheremonth",
    "whereyear",
    "wheretime",
    "wherecolumn",
    "wherenull",
    "wherenotnull",
    "having",
    "orhaving",
    "firstwhere",
    "value",
    "min",
    "max",
    "sum",
    "avg",
    "increment",
    "decrement",
    "wherebetween",
    "wherenotbetween",
];

/// Methods where *any* string argument is a column.
const COL_ANY: &[&str] = &[
    "select",
    "addselect",
    "pluck",
    "orderby",
    "orderbydesc",
    "groupby",
];

/// Methods whose string arguments are relationship names.
const REL_ANY: &[&str] = &[
    "with",
    "load",
    "loadmissing",
    "wherehas",
    "orwherehas",
    "wheredoesnthave",
    "orwheredoesnthave",
    "has",
    "doesnthave",
    "withcount",
    "withsum",
    "withavg",
    "withmax",
    "withmin",
    "withexists",
];

pub struct QueryCtx {
    pub partial: String,
    /// `true` for relationship completion, `false` for column completion.
    pub relation: bool,
}

pub struct QueryTarget {
    pub model: Option<String>,
    pub table: String,
}

fn is_ident(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Detect whether the cursor sits in a query-builder string argument.
pub fn context(line_before: &str) -> Option<QueryCtx> {
    // 1. The unterminated string the cursor is inside.
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
    let (_, content_start) = in_str?;
    let partial = line_before[content_start..].to_string();

    // 2. Walk back from the quote to the enclosing `(`, counting top-level commas.
    let pre = &line_before[..content_start - 1];
    let pb = pre.as_bytes();
    let mut depth = 0i32;
    let mut commas = 0usize;
    let mut idx = pre.len();
    let mut open = None;
    while idx > 0 {
        idx -= 1;
        match pb[idx] as char {
            ')' | ']' | '}' => depth += 1,
            '(' | '[' | '{' if depth > 0 => depth -= 1,
            '(' => {
                open = Some(idx);
                break;
            }
            '[' | '{' => return None,
            ',' if depth == 0 => commas += 1,
            _ => {}
        }
    }
    let open = open?;
    let method: String = pre[..open]
        .chars()
        .rev()
        .take_while(|c| is_ident(*c))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let m = method.to_lowercase();

    let m = m.as_str();
    if REL_ANY.contains(&m) {
        Some(QueryCtx {
            partial,
            relation: true,
        })
    } else if COL_ANY.contains(&m) || (COL_FIRST.contains(&m) && commas == 0) {
        Some(QueryCtx {
            partial,
            relation: false,
        })
    } else {
        None
    }
}

/// Resolve the model/table the current query statement operates on.
pub fn resolve_target(text: &str, offset: usize, root: &Path) -> Option<QueryTarget> {
    let upto = offset.min(text.len());
    let start = text[..upto]
        .rfind([';', '{', '}'])
        .map(|i| i + 1)
        .unwrap_or(0);
    let stmt = &text[start..upto];

    // `DB::table('name')`
    if let Some(p) = stmt.find("DB::table(") {
        let after = stmt[p + "DB::table(".len()..].trim_start();
        if let Some(q) = after.chars().next() {
            if q == '\'' || q == '"' {
                if let Some(end) = after[1..].find(q) {
                    return Some(QueryTarget {
                        model: None,
                        table: after[1..1 + end].to_string(),
                    });
                }
            }
        }
    }

    // First `Model::` in the statement.
    if let Some(model) = first_static_model(stmt) {
        return Some(QueryTarget {
            table: model_table(root, &model),
            model: Some(model),
        });
    }

    // First `$var` → resolve its model.
    if let Some(var) = first_var(stmt) {
        if let Some(model) = resolve_model(&text[..upto], &var) {
            return Some(QueryTarget {
                table: model_table(root, &model),
                model: Some(model),
            });
        }
    }
    None
}

fn first_static_model(stmt: &str) -> Option<String> {
    let idx = stmt.find("::")?;
    let before = &stmt[..idx];
    let cls: String = before
        .chars()
        .rev()
        .take_while(|c| is_ident(*c) || *c == '\\')
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let cls = cls.rsplit('\\').next().unwrap_or(&cls).to_string();
    if cls
        .chars()
        .next()
        .map(|c| c.is_uppercase())
        .unwrap_or(false)
        && !matches!(cls.as_str(), "DB" | "Str" | "Arr" | "Cache" | "Auth")
    {
        Some(cls)
    } else {
        None
    }
}

fn first_var(stmt: &str) -> Option<String> {
    let at = stmt.find('$')?;
    let name: String = stmt[at + 1..]
        .chars()
        .take_while(|c| is_ident(*c))
        .collect();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

/// Find the first column string argument of every column method call in the
/// file, as `(start, end, column)` byte ranges — for linting unknown columns.
pub fn column_args(text: &str) -> Vec<(usize, usize, String)> {
    let mut out = Vec::new();
    for (i, c) in text.char_indices() {
        if c != '(' {
            continue;
        }
        // Method identifier immediately before the `(` (case-insensitive).
        let ident: String = text[..i]
            .chars()
            .rev()
            .take_while(|c| is_ident(*c))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        let m = ident.to_lowercase();
        if !COL_FIRST.contains(&m.as_str()) && !COL_ANY.contains(&m.as_str()) {
            continue;
        }
        let after = &text[i + 1..];
        let a = after.trim_start();
        let ws = after.len() - a.len();
        let Some(q) = a.chars().next() else { continue };
        if q != '\'' && q != '"' {
            continue;
        }
        let vstart = i + 1 + ws + 1;
        let Some(er) = text[vstart..].find(q) else {
            continue;
        };
        let vend = vstart + er;
        let col = text[vstart..vend].to_string();
        // Only simple identifiers (skip `*`, `table.col`, expressions, aliases).
        if !col.is_empty() && col.chars().all(is_ident) {
            out.push((vstart, vend, col));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_column_args() {
        let src = "User::where('email', 1)->orderBy('created_at'); DB::raw('*');";
        let cols: Vec<String> = column_args(src).into_iter().map(|(_, _, c)| c).collect();
        assert!(cols.contains(&"email".to_string()));
        assert!(cols.contains(&"created_at".to_string()));
    }

    #[test]
    fn detects_column_context() {
        let c = context("User::where('ema").unwrap();
        assert!(!c.relation);
        assert_eq!(c.partial, "ema");
        // Second argument of where() is a value, not a column.
        assert!(context("User::where('email', 'ad").is_none());
        // select() completes any argument.
        assert!(context("$q->select('id', 'na").is_some());
    }

    #[test]
    fn detects_relation_context() {
        let c = context("$user->load('po").unwrap();
        assert!(c.relation);
        assert_eq!(c.partial, "po");
        assert!(context("User::with('posts', 'com").unwrap().relation);
    }

    #[test]
    fn resolves_target() {
        let t = resolve_target("User::where('", 12, Path::new("/tmp")).unwrap();
        assert_eq!(t.model.as_deref(), Some("User"));
        assert_eq!(t.table, "users");
        let d = resolve_target("DB::table('orders')->where('", 28, Path::new("/tmp")).unwrap();
        assert_eq!(d.table, "orders");
        assert_eq!(d.model, None);
    }
}
