//! Eloquent relationship graph: parse relationships from model classes and
//! cross-check them against the foreign keys in the live database. Together with
//! the schema diff, this shows the whole truth — code, migrations, and the
//! actual database — in one picture.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use e_db::ForeignKey;

use crate::eloquent::{pluralize, snake_case};

/// Relationship methods we recognise on a model.
const KINDS: &[&str] = &[
    "hasMany",
    "hasOne",
    "belongsToMany",
    "belongsTo",
    "hasManyThrough",
    "hasOneThrough",
    "morphMany",
    "morphOne",
    "morphToMany",
    "morphedByMany",
    "morphTo",
];

#[derive(Clone)]
pub struct Relation {
    pub kind: String,
    pub method: String,
    pub target: String,
    pub target_file: Option<PathBuf>,
    pub line: usize,
    /// `true` when a matching foreign key exists (or none is expected).
    pub ok: bool,
}

#[derive(Clone)]
pub struct ModelNode {
    pub name: String,
    pub table: String,
    pub file: PathBuf,
    pub relations: Vec<Relation>,
}

/// A relationship as parsed from source, before cross-checking.
struct RawRel {
    kind: String,
    method: String,
    target: String,
    line: usize,
}

struct ParsedModel {
    name: String,
    table: String,
    file: PathBuf,
    rels: Vec<RawRel>,
}

fn is_ident(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Read `protected $table = '...'`, else snake_case + pluralise the class.
fn model_table(src: &str, class: &str) -> String {
    if let Some(idx) = src.find("$table") {
        let rest = &src[idx..];
        if let Some(eq) = rest.find('=') {
            let after = rest[eq + 1..].trim_start();
            if let Some(q) = after.chars().next() {
                if q == '\'' || q == '"' {
                    if let Some(end) = after[1..].find(q) {
                        return after[1..1 + end].to_string();
                    }
                }
            }
        }
    }
    pluralize(&snake_case(class))
}

/// Resolve the related class from a relationship call's arguments.
fn relation_target(args: &str) -> Option<String> {
    if let Some(i) = args.find("::class") {
        let before = &args[..i];
        let cls: String = before
            .chars()
            .rev()
            .take_while(|c| is_ident(*c) || *c == '\\')
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        let cls = cls.rsplit('\\').next().unwrap_or(&cls).to_string();
        return cls.chars().next().filter(|c| c.is_uppercase()).map(|_| cls);
    }
    // Quoted class string: 'App\Models\Post'
    let a = args.trim_start();
    let q = a.chars().next()?;
    if q == '\'' || q == '"' {
        let inner = &a[1..];
        let end = inner.find(q)?;
        let s = &inner[..end];
        let base = s.rsplit('\\').next().unwrap_or(s);
        return base
            .chars()
            .next()
            .filter(|c| c.is_uppercase())
            .map(|_| base.to_string());
    }
    None
}

fn parse_model(src: &str, file: &Path) -> Option<ParsedModel> {
    // Class name.
    let ci = src.find("class ")?;
    let after = &src[ci + 6..];
    let name: String = after.chars().take_while(|c| is_ident(*c)).collect();
    if name.is_empty() {
        return None;
    }
    let table = model_table(src, &name);

    // Line-start offsets for line numbers.
    let mut rels = Vec::new();
    let bytes = src.as_bytes();
    let mut search = 0;
    while let Some(rel) = src[search..].find("$this->") {
        let pos = search + rel;
        search = pos + 7;
        let ident: String = src[pos + 7..]
            .chars()
            .take_while(|c| is_ident(*c))
            .collect();
        if ident.is_empty() || !KINDS.contains(&ident.as_str()) {
            continue;
        }
        // Arguments between the parens.
        let open = pos + 7 + ident.len();
        if bytes.get(open) != Some(&b'(') {
            continue;
        }
        let args_end = src[open + 1..]
            .find(')')
            .map(|i| open + 1 + i)
            .unwrap_or(src.len());
        let args = &src[open + 1..args_end];
        let target = relation_target(args).unwrap_or_default();
        // Enclosing method name.
        let method = src[..pos]
            .rfind("function ")
            .map(|f| {
                src[f + 9..]
                    .chars()
                    .take_while(|c| is_ident(*c))
                    .collect::<String>()
            })
            .unwrap_or_default();
        let line = src[..pos].bytes().filter(|b| *b == b'\n').count() + 1;
        rels.push(RawRel {
            kind: ident,
            method,
            target,
            line,
        });
    }
    Some(ParsedModel {
        name,
        table,
        file: file.to_path_buf(),
        rels,
    })
}

/// Does the live schema contain a foreign key backing this relationship?
fn fk_ok(kind: &str, m_table: &str, t_table: &str, fks: &[ForeignKey]) -> bool {
    match kind {
        "belongsTo" => fks
            .iter()
            .any(|f| f.table == m_table && f.ref_table == t_table),
        "hasMany" | "hasOne" => fks
            .iter()
            .any(|f| f.table == t_table && f.ref_table == m_table),
        "belongsToMany" | "morphToMany" | "morphedByMany" => {
            // A pivot table referencing both sides.
            fks.iter().any(|f| f.ref_table == m_table) && fks.iter().any(|f| f.ref_table == t_table)
        }
        // Polymorphic / through relations have no direct verifiable FK.
        _ => true,
    }
}

/// Parse every model under `app/` and cross-check against `fks`.
pub fn build_graph(root: &Path, fks: &[ForeignKey]) -> Vec<ModelNode> {
    let mut files = Vec::new();
    for dir in [root.join("app/Models"), root.join("app")] {
        collect_php(&dir, &mut files, 0);
    }
    files.sort();
    files.dedup();

    let mut parsed: Vec<ParsedModel> = files
        .iter()
        .filter_map(|f| {
            std::fs::read_to_string(f)
                .ok()
                .and_then(|s| parse_model(&s, f))
        })
        .filter(|m| !m.rels.is_empty() || looks_like_model(&m.file))
        .collect();
    parsed.sort_by(|a, b| a.name.cmp(&b.name));
    parsed.dedup_by(|a, b| a.name == b.name);

    // class -> (table, file)
    let index: HashMap<String, (String, PathBuf)> = parsed
        .iter()
        .map(|m| (m.name.clone(), (m.table.clone(), m.file.clone())))
        .collect();

    parsed
        .into_iter()
        .map(|m| {
            let relations = m
                .rels
                .into_iter()
                .map(|r| {
                    let (t_table, t_file) = index
                        .get(&r.target)
                        .map(|(t, f)| (t.clone(), Some(f.clone())))
                        .unwrap_or_else(|| {
                            if r.target.is_empty() {
                                (String::new(), None)
                            } else {
                                (pluralize(&snake_case(&r.target)), None)
                            }
                        });
                    let ok = t_table.is_empty() || fk_ok(&r.kind, &m.table, &t_table, fks);
                    Relation {
                        kind: r.kind,
                        method: r.method,
                        target: r.target,
                        target_file: t_file,
                        line: r.line,
                        ok,
                    }
                })
                .collect();
            ModelNode {
                name: m.name,
                table: m.table,
                file: m.file,
                relations,
            }
        })
        .filter(|n| !n.relations.is_empty())
        .collect()
}

/// Relationship method names declared on a single model class (for query
/// completion in `with()`, `whereHas()`, …).
pub fn relation_names(root: &Path, class: &str) -> Vec<String> {
    for cand in [
        root.join(format!("app/Models/{class}.php")),
        root.join(format!("app/{class}.php")),
    ] {
        if let Ok(src) = std::fs::read_to_string(&cand) {
            if let Some(pm) = parse_model(&src, &cand) {
                let mut out: Vec<String> = pm
                    .rels
                    .into_iter()
                    .map(|r| r.method)
                    .filter(|m| !m.is_empty())
                    .collect();
                out.sort();
                out.dedup();
                return out;
            }
        }
    }
    Vec::new()
}

fn looks_like_model(file: &Path) -> bool {
    file.components().any(|c| c.as_os_str() == "Models")
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
    fn parses_relationships() {
        let src = r#"<?php
        class User extends Model {
            public function articles() { return $this->hasMany(Article::class); }
            public function team() { return $this->belongsTo(Team::class); }
        }"#;
        let m = parse_model(src, Path::new("User.php")).unwrap();
        assert_eq!(m.name, "User");
        assert_eq!(m.table, "users");
        assert_eq!(m.rels.len(), 2);
        assert_eq!(m.rels[0].kind, "hasMany");
        assert_eq!(m.rels[0].target, "Article");
        assert_eq!(m.rels[0].method, "articles");
    }

    #[test]
    fn cross_checks_foreign_keys() {
        let fks = vec![ForeignKey {
            table: "articles".into(),
            column: "user_id".into(),
            ref_table: "users".into(),
            ref_column: "id".into(),
        }];
        // users hasMany articles -> FK on articles.user_id exists → ok
        assert!(fk_ok("hasMany", "users", "articles", &fks));
        // articles belongsTo users -> FK on articles referencing users → ok
        assert!(fk_ok("belongsTo", "articles", "users", &fks));
        // users belongsTo something with no FK → flagged
        assert!(!fk_ok("belongsTo", "users", "teams", &fks));
    }
}
