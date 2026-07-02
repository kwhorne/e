//! The props contract between a Laravel controller and an Inertia page.
//!
//! The controller sends `Inertia::render('Users/Index', ['users' => …])`; the
//! page component just hopes the shape is right. Because `e` understands PHP,
//! the database schema, *and* the JS component, it can reconcile the two:
//! infer prop types from the render call (a `User::paginate()` becomes
//! `User[]`, whose fields come from the live schema), flag props sent but never
//! used and props used but never sent, and generate TypeScript interfaces.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use e_db::ColumnInfo;

use crate::eloquent::{pluralize, snake_case};
use crate::inertia::page_roots;

#[derive(Clone)]
pub struct Prop {
    pub key: String,
    pub ty: String,
    /// Model class the type is based on, if any (for TS generation).
    pub model: Option<String>,
    /// Sent by the controller but never referenced in the component.
    pub unused: bool,
}

#[derive(Clone)]
pub struct Contract {
    pub page: String,
    pub controller: PathBuf,
    pub props: Vec<Prop>,
    /// Props the component uses/declares that the controller never sends.
    pub missing: Vec<String>,
}

fn is_ident(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// The Inertia page name for a component path (`…/Pages/Users/Index.vue` →
/// `Users/Index`).
pub fn page_name_of(root: &Path, path: &Path) -> Option<String> {
    for base in page_roots(root) {
        if let Ok(rel) = path.strip_prefix(&base) {
            return Some(rel.with_extension("").to_string_lossy().replace('\\', "/"));
        }
    }
    None
}

/// Infer a TS type (and the backing model) from a PHP prop value expression.
fn infer_type(expr: &str) -> (String, Option<String>) {
    let e = expr.trim();
    // `Model::…` or `$var->…` chains — look for a leading Model class.
    if let Some(model) = leading_model(e) {
        let collection = e.contains("paginate")
            || e.contains("->get(")
            || e.contains("::all(")
            || e.contains("->all(")
            || e.contains("::collection(")
            || e.contains("->cursor(");
        let ty = if collection {
            format!("{model}[]")
        } else {
            model.clone()
        };
        return (ty, Some(model));
    }
    if e.starts_with('[') {
        ("unknown[]".to_string(), None)
    } else if e == "true" || e == "false" || e.starts_with("(bool)") {
        ("boolean".to_string(), None)
    } else if e.starts_with('\'') || e.starts_with('"') {
        ("string".to_string(), None)
    } else if e
        .chars()
        .next()
        .map(|c| c.is_ascii_digit())
        .unwrap_or(false)
    {
        ("number".to_string(), None)
    } else {
        ("unknown".to_string(), None)
    }
}

/// The first `Model::` class name in an expression, if it looks like a model.
fn leading_model(expr: &str) -> Option<String> {
    let idx = expr.find("::")?;
    let before = &expr[..idx];
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
        && cls != "Inertia"
        && cls != "Response"
    {
        Some(cls)
    } else {
        None
    }
}

/// Extract `'key' => expr` entries at the top level of an array's inner text.
fn top_entries(inner: &str) -> Vec<(String, String)> {
    let bytes = inner.as_bytes();
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut i = 0;
    while i < inner.len() {
        match bytes[i] as char {
            '[' | '(' | '{' => {
                depth += 1;
                i += 1;
            }
            ']' | ')' | '}' => {
                depth -= 1;
                i += 1;
            }
            c @ ('\'' | '"') if depth == 0 => {
                let mut j = i + 1;
                while j < inner.len() && bytes[j] as char != c {
                    j += 1;
                }
                let key = inner[i + 1..j.min(inner.len())].to_string();
                let mut k = j + 1;
                while k < inner.len() && (bytes[k] as char).is_whitespace() {
                    k += 1;
                }
                if inner[k..].starts_with("=>") {
                    // Capture the value up to the next top-level comma.
                    let vstart = k + 2;
                    let mut m = vstart;
                    let mut d = 0i32;
                    while m < inner.len() {
                        match bytes[m] as char {
                            '[' | '(' | '{' => d += 1,
                            ']' | ')' | '}' => d -= 1,
                            ',' if d == 0 => break,
                            _ => {}
                        }
                        m += 1;
                    }
                    let expr = inner[vstart..m].trim().to_string();
                    if key.chars().all(|c| is_ident(c) || c == '-') && !key.is_empty() {
                        out.push((key, expr));
                    }
                    i = m;
                } else {
                    i = j + 1;
                }
            }
            _ => i += 1,
        }
    }
    out
}

/// The balanced `[ … ]` array immediately following `from` in `src`.
fn array_inner_after(src: &str, from: usize) -> Option<String> {
    let open = src[from..].find('[')? + from;
    let bytes = src.as_bytes();
    let mut depth = 0i32;
    let mut i = open;
    while i < src.len() {
        match bytes[i] as char {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(src[open + 1..i].to_string());
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Parse the props sent to `page` from a controller's source.
pub fn render_props(controller_src: &str, page: &str) -> Option<Vec<(String, String)>> {
    for needle in ["Inertia::render(", "inertia("] {
        let mut search = 0;
        while let Some(rel) = controller_src[search..].find(needle) {
            let open = search + rel + needle.len();
            search = open;
            let after = controller_src[open..].trim_start();
            let ws = controller_src[open..].len() - after.len();
            let Some(quote) = after.chars().next() else {
                continue;
            };
            if quote != '\'' && quote != '"' {
                continue;
            }
            let vstart = open + ws + 1;
            let Some(close) = controller_src[vstart..].find(quote) else {
                continue;
            };
            let name = &controller_src[vstart..vstart + close];
            if name == page {
                let rest = vstart + close + 1;
                return array_inner_after(controller_src, rest).map(|inner| top_entries(&inner));
            }
        }
    }
    None
}

/// Prop names the component declares (`defineProps`) or references (`props.x`).
pub fn component_props(src: &str) -> (Vec<String>, std::collections::HashSet<String>) {
    let mut declared = Vec::new();
    if let Some(at) = src.find("defineProps") {
        let rest = &src[at + "defineProps".len()..];
        // Generic `<{ … }>` or call `({ … })`.
        let inner = rest
            .find('<')
            .and_then(|i| brace_inner(&rest[i..]))
            .or_else(|| rest.find('(').and_then(|i| brace_inner(&rest[i..])));
        if let Some(inner) = inner {
            for (i, line) in inner.split([',', '\n', ';']).enumerate() {
                let _ = i;
                let key: String = line.trim().chars().take_while(|c| is_ident(*c)).collect();
                if !key.is_empty() && !declared.contains(&key) {
                    declared.push(key);
                }
            }
        }
    }
    // Every `props.X` / `.props.X` reference and bare identifiers.
    let mut used = std::collections::HashSet::new();
    let mut search = 0;
    while let Some(rel) = src[search..].find("props.") {
        let at = search + rel + "props.".len();
        let name: String = src[at..].chars().take_while(|c| is_ident(*c)).collect();
        if !name.is_empty() {
            used.insert(name);
        }
        search = at;
    }
    (declared, used)
}

fn brace_inner(s: &str) -> Option<String> {
    let (open_ch, close_ch) = ('{', '}');
    let open = s.find(open_ch)?;
    let bytes = s.as_bytes();
    let mut depth = 0i32;
    let mut i = open;
    while i < s.len() {
        let c = bytes[i] as char;
        if c == open_ch {
            depth += 1;
        } else if c == close_ch {
            depth -= 1;
            if depth == 0 {
                return Some(s[open + 1..i].to_string());
            }
        }
        i += 1;
    }
    None
}

/// Map a database column type to a TypeScript type.
fn ts_type(sql: &str) -> &'static str {
    let s = sql.to_lowercase();
    if s.contains("int")
        || s.contains("decimal")
        || s.contains("float")
        || s.contains("double")
        || s.contains("numeric")
    {
        "number"
    } else if s.contains("bool") || s == "tinyint(1)" {
        "boolean"
    } else if s.contains("json") {
        "any"
    } else {
        "string"
    }
}

/// Build the full contract for a page component.
pub fn build(
    root: &Path,
    page: &str,
    component_src: &str,
    schema: &HashMap<String, Vec<ColumnInfo>>,
    shared: &[String],
) -> Option<Contract> {
    // Find the controller that renders this page.
    let controllers = collect_controllers(root);
    let (controller, entries) = controllers.iter().find_map(|c| {
        let src = std::fs::read_to_string(c).ok()?;
        render_props(&src, page).map(|e| (c.clone(), e))
    })?;

    let (declared, used) = component_props(component_src);
    let used_word = |w: &str| component_src.contains(w);

    let props: Vec<Prop> = entries
        .iter()
        .map(|(key, expr)| {
            let (ty, model) = infer_type(expr);
            let referenced =
                used.contains(key) || declared.iter().any(|d| d == key) || used_word(key);
            Prop {
                key: key.clone(),
                ty,
                model,
                unused: !referenced,
            }
        })
        .collect();

    let sent_keys: std::collections::HashSet<&str> =
        entries.iter().map(|(k, _)| k.as_str()).collect();
    let shared_top: std::collections::HashSet<&str> =
        shared.iter().filter_map(|s| s.split('.').next()).collect();
    let mut missing: Vec<String> = declared
        .iter()
        .chain(used.iter())
        .filter(|k| !sent_keys.contains(k.as_str()) && !shared_top.contains(k.as_str()))
        .cloned()
        .collect();
    missing.sort();
    missing.dedup();

    // Note the schema so TS generation can expand model fields.
    let _ = (schema, root);
    Some(Contract {
        page: page.to_string(),
        controller,
        props,
        missing,
    })
}

/// Generate TypeScript interfaces for a contract, expanding model fields from
/// the live database schema.
pub fn generate_ts(contract: &Contract, schema: &HashMap<String, Vec<ColumnInfo>>) -> String {
    let iface = contract.page.replace('/', "");
    let mut out = format!(
        "// Generated by e from {}\n\nexport interface {iface}Props {{\n",
        contract.page
    );
    for p in &contract.props {
        out.push_str(&format!("  {}: {};\n", p.key, p.ty));
    }
    out.push_str("}\n");

    // Emit an interface for each referenced model, from the schema.
    let mut seen = std::collections::HashSet::new();
    for p in &contract.props {
        if let Some(model) = &p.model {
            if !seen.insert(model.clone()) {
                continue;
            }
            let table = pluralize(&snake_case(model));
            if let Some(cols) = schema.get(&table) {
                out.push_str(&format!("\nexport interface {model} {{\n"));
                for c in cols {
                    let opt = if c.nullable { "?" } else { "" };
                    out.push_str(&format!(
                        "  {}{}: {};\n",
                        c.name,
                        opt,
                        ts_type(&c.data_type)
                    ));
                }
                out.push_str("}\n");
            }
        }
    }
    out
}

fn collect_controllers(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect_php(&root.join("app/Http/Controllers"), &mut out, 0);
    out
}

fn collect_php(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > 4 {
        return;
    }
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for e in read.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_php(&p, out, depth + 1);
        } else if p.extension().and_then(|x| x.to_str()) == Some("php") {
            out.push(p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infers_types() {
        assert_eq!(infer_type("User::paginate()").0, "User[]");
        assert_eq!(infer_type("User::find($id)").0, "User");
        assert_eq!(infer_type("$user->posts()->get()").0, "unknown"); // no leading Model
        assert_eq!(infer_type("['a' => 1]").0, "unknown[]");
        assert_eq!(infer_type("true").0, "boolean");
        assert_eq!(infer_type("'hello'").0, "string");
    }

    #[test]
    fn parses_render_props() {
        let ctrl = r#"
        public function index() {
            return Inertia::render('Users/Index', [
                'users' => User::paginate(15),
                'filters' => $request->only(['search']),
                'canEdit' => true,
            ]);
        }"#;
        let props = render_props(ctrl, "Users/Index").unwrap();
        let keys: Vec<&str> = props.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, vec!["users", "filters", "canEdit"]);
        assert_eq!(infer_type(&props[0].1).0, "User[]");
    }

    #[test]
    fn generates_typescript() {
        let contract = Contract {
            page: "Users/Index".to_string(),
            controller: PathBuf::from("x"),
            props: vec![
                Prop {
                    key: "users".into(),
                    ty: "User[]".into(),
                    model: Some("User".into()),
                    unused: false,
                },
                Prop {
                    key: "canEdit".into(),
                    ty: "boolean".into(),
                    model: None,
                    unused: false,
                },
            ],
            missing: vec![],
        };
        let mut schema = HashMap::new();
        schema.insert(
            "users".to_string(),
            vec![
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
                    name: "bio".into(),
                    data_type: "text".into(),
                    nullable: true,
                    key: String::new(),
                },
            ],
        );
        let ts = generate_ts(&contract, &schema);
        assert!(ts.contains("export interface UsersIndexProps {"));
        assert!(ts.contains("users: User[];"));
        assert!(ts.contains("canEdit: boolean;"));
        assert!(ts.contains("export interface User {"));
        assert!(ts.contains("id: number;"));
        assert!(ts.contains("bio?: string;")); // nullable → optional
    }

    #[test]
    fn reads_component_props() {
        let vue = r#"<script setup lang="ts">
        const props = defineProps<{ users: Array<User>; filters: object }>();
        console.log(props.users, props.role);
        </script>"#;
        let (declared, used) = component_props(vue);
        assert!(declared.contains(&"users".to_string()));
        assert!(declared.contains(&"filters".to_string()));
        assert!(used.contains("users"));
        assert!(used.contains("role"));
    }
}
