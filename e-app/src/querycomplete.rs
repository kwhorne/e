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

/// Builder methods whose closure argument queries a *relation*:
/// `whereHas('posts', fn ($q) => …)`, `with(['posts' => fn ($q) => …])`.
const RELATION_CLOSURES: &[&str] = &[
    "wherehas",
    "orwherehas",
    "wheredoesnthave",
    "orwheredoesnthave",
    "has",
    "orhas",
    "doesnthave",
    "ordoesnthave",
    "withwherehas",
    "with",
    "withcount",
    "load",
    "loadcount",
];

/// Builder methods whose closure argument queries the *same* table:
/// `where(fn ($q) => …)` groups conditions, `when($x, fn ($q) => …)`.
const SAME_TABLE_CLOSURES: &[&str] = &[
    "where",
    "orwhere",
    "wherenot",
    "orwherenot",
    "when",
    "unless",
    "tap",
    "having",
    "orhaving",
];

/// The closure the caret is in, if any, by the call it is an argument of.
#[derive(Debug, PartialEq, Eq)]
enum Closure {
    /// A relation's query: `relation` is its name (maybe dotted), `call` the
    /// offset of the call's `(`.
    Relation { call: usize, relation: String },
    /// The same query, grouped.
    SameTable { call: usize },
    /// Some other closure: what `$q` is there isn't known.
    Other,
}

/// Find the innermost closure argument around `upto`, looking back over a
/// bounded window and skipping strings and `//` comments.
fn enclosing_closure(text: &str, upto: usize) -> Option<Closure> {
    let mut from = upto.saturating_sub(4000);
    while !text.is_char_boundary(from) {
        from += 1;
    }
    let w = &text[from..upto];
    let b = w.as_bytes();
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
                b'(' | b'[' | b'{' => open.push(i),
                b')' | b']' | b'}' => {
                    open.pop();
                }
                _ => {}
            },
        }
        i += 1;
    }
    // From the innermost open call outwards: is a closure argument of it
    // open around the caret? Its text then holds `fn (…) =>` or `function`.
    for (k, &at) in open.iter().enumerate().rev() {
        if b[at] != b'(' {
            continue;
        }
        // Up to the next call opened inside it — past any array or closure
        // body (`with(['posts' => function ($q) {`) — or the caret.
        let seg_end = open[k + 1..]
            .iter()
            .copied()
            .find(|&o| b[o] == b'(')
            .unwrap_or(w.len());
        let seg = &w[at + 1..seg_end];
        let closure_at = [seg.rfind("fn ("), seg.rfind("fn("), seg.rfind("function")]
            .into_iter()
            .flatten()
            .max();
        let Some(closure_at) = closure_at else {
            continue;
        };
        let body = &seg[closure_at..];
        // An arrow function ends at the next top-level `,`: after
        // `fn ($x) => 1, User::where('` the caret is no longer in it.
        let is_closure = body.starts_with("function")
            || body
                .find("=>")
                .is_some_and(|arrow| !has_top_level_comma(&body[arrow..]));
        if !is_closure {
            continue;
        }
        let method: String = w[..at]
            .chars()
            .rev()
            .take_while(|c| is_ident(*c))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<String>()
            .to_lowercase();
        let call = from + at;
        if RELATION_CLOSURES.contains(&method.as_str()) {
            let relation = last_string(&seg[..closure_at]);
            return Some(match relation {
                Some(relation) => Closure::Relation { call, relation },
                None => Closure::Other,
            });
        }
        if SAME_TABLE_CLOSURES.contains(&method.as_str()) {
            return Some(Closure::SameTable { call });
        }
        return Some(Closure::Other);
    }
    None
}

/// Whether `s` has a `,` outside any brackets or strings.
fn has_top_level_comma(s: &str) -> bool {
    let (mut depth, mut quote) = (0i32, None);
    for c in s.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None => match c {
                '\'' | '"' => quote = Some(c),
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth -= 1,
                ',' if depth == 0 => return true,
                _ => {}
            },
        }
    }
    false
}

/// The contents of the last quoted string in `s`.
fn last_string(s: &str) -> Option<String> {
    let end = s.rfind(['\'', '"'])?;
    let q = s.as_bytes()[end] as char;
    let start = s[..end].rfind(q)?;
    Some(s[start + 1..end].to_string()).filter(|r| !r.is_empty())
}

/// Resolve the model/table the current query statement operates on.
pub fn resolve_target(text: &str, offset: usize, root: &Path) -> Option<QueryTarget> {
    let upto = offset.min(text.len());
    // Inside a closure the query is another one: a relation's, or the same
    // query grouped — or one that can't be told, which is better left alone
    // than checked against the wrong table.
    match enclosing_closure(text, upto) {
        Some(Closure::Relation { call, relation }) => {
            let mut model = resolve_target(text, call, root)?.model?;
            for name in relation.split('.') {
                model = crate::relations::related_model(root, &model, name)?;
            }
            return Some(QueryTarget {
                table: model_table(root, &model),
                model: Some(model),
            });
        }
        Some(Closure::SameTable { call }) => return resolve_target(text, call, root),
        Some(Closure::Other) => return None,
        None => {}
    }
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

    fn project() -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("e-qc-{}", std::process::id()));
        let models = root.join("app/Models");
        std::fs::create_dir_all(&models).unwrap();
        std::fs::write(
            models.join("User.php"),
            "<?php\nclass User extends Model {\n    public function posts() { return $this->hasMany(Post::class); }\n}\n",
        )
        .unwrap();
        std::fs::write(
            models.join("Post.php"),
            "<?php\nclass Post extends Model {\n    public function comments() { return $this->hasMany(Comment::class); }\n}\n",
        )
        .unwrap();
        root
    }

    fn table_at(src: &str, root: &Path) -> Option<String> {
        resolve_target(src, src.len(), root).map(|t| t.table)
    }

    #[test]
    fn a_relation_closure_queries_the_related_table() {
        let root = project();
        let t = |s: &str| table_at(s, &root);
        assert_eq!(
            t("User::whereHas('posts', fn ($q) => $q->where('").as_deref(),
            Some("posts")
        );
        assert_eq!(
            t("User::with(['posts' => function ($q) {\n    $q->where('").as_deref(),
            Some("posts")
        );
        assert_eq!(
            t("User::whereHas('posts.comments', fn ($q) => $q->where('").as_deref(),
            Some("comments")
        );
        // Grouped conditions stay on the outer table.
        assert_eq!(
            t("User::where(fn ($q) => $q->where('").as_deref(),
            Some("users")
        );
        // Outside any closure, as before.
        assert_eq!(t("User::where('").as_deref(), Some("users"));
        // An unknown relation or closure: no target rather than a wrong one.
        assert_eq!(t("User::whereHas('teams', fn ($q) => $q->where('"), None);
        assert_eq!(t("collect($xs)->map(fn ($q) => $q->where('"), None);
        // An arrow function that already ended doesn't count.
        assert_eq!(
            t("$x = tap(fn ($u) => 1, User::where('").as_deref(),
            Some("users")
        );
    }
}
