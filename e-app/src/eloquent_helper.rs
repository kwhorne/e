//! Eloquent helper code: phpDocs for every model, generated from the live
//! database schema and the relationships in the models' source, written to
//! `_ide_helper_models.php` in the project root.
//!
//! Intelephense indexes that file like any other, so `$user->posts`,
//! `$order->total` and `User::active()` complete and type-check *everywhere* —
//! inside closures, on collections, in Blade — not only where `e`'s own
//! heuristics reach. It is what `barryvdh/laravel-ide-helper`'s
//! `ide-helper:models --nowrite` produces, without installing the package or
//! booting the app. Regenerated whenever the schema is reloaded, once it exists.

use std::collections::HashMap;
use std::path::Path;

use e_db::ColumnInfo;
use floem::reactive::SignalGet;

use crate::relations::ModelNode;
use crate::state::AppState;

pub const FILE_NAME: &str = "_ide_helper_models.php";

/// One model as the helper describes it.
#[derive(Clone, Debug)]
pub struct HelperModel {
    pub namespace: String,
    pub class: String,
    pub columns: Vec<ColumnInfo>,
    /// `(relation kind, method name, target FQN)`.
    pub relations: Vec<(String, String, String)>,
    /// Query scopes, as their query-builder method names (`active`, not `scopeActive`).
    pub scopes: Vec<String>,
}

/// The PHP type for a column, in phpDoc form.
pub fn php_type(col: &ColumnInfo) -> String {
    let t = col.data_type.to_lowercase();
    let base = if t == "tinyint(1)" || t.contains("bool") {
        "bool"
    } else if t.contains("int") || t == "serial" || t == "bigserial" {
        "int"
    } else if t.contains("decimal")
        || t.contains("numeric")
        || t.contains("float")
        || t.contains("double")
        || t.contains("real")
    {
        "float"
    } else if t.contains("json") {
        "array"
    } else if t.contains("timestamp") || t.contains("datetime") || t == "date" {
        "\\Illuminate\\Support\\Carbon"
    } else {
        "string"
    };
    if col.nullable {
        format!("{base}|null")
    } else {
        base.to_string()
    }
}

/// The `namespace …;` a PHP file declares.
pub fn namespace_of(src: &str) -> Option<String> {
    let i = src.find("namespace ")?;
    let rest = &src[i + "namespace ".len()..];
    let end = rest.find(';')?;
    let ns = rest[..end].trim();
    (!ns.is_empty()).then(|| ns.to_string())
}

/// Query scopes a model declares: `scopeActive()` (classic) and methods under a
/// `#[Scope]` attribute (Laravel 12.6+), as their builder method names.
pub fn scopes_of(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut prev_line_is_scope_attr = false;
    for line in src.lines() {
        let t = line.trim();
        if let Some(pos) = t.find("function ") {
            let name: String = t[pos + "function ".len()..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if prev_line_is_scope_attr && !name.is_empty() {
                out.push(name);
            } else if let Some(rest) = name.strip_prefix("scope") {
                if rest
                    .chars()
                    .next()
                    .map(|c| c.is_ascii_uppercase())
                    .unwrap_or(false)
                {
                    let mut c = rest.chars();
                    let first = c.next().unwrap().to_lowercase().collect::<String>();
                    out.push(first + c.as_str());
                }
            }
        }
        prev_line_is_scope_attr = t.starts_with("#[Scope");
    }
    out.sort();
    out.dedup();
    out
}

/// Whether a relation returns one model or a collection of them.
fn is_many(kind: &str) -> bool {
    matches!(
        kind,
        "hasMany"
            | "belongsToMany"
            | "morphMany"
            | "morphToMany"
            | "morphedByMany"
            | "hasManyThrough"
    )
}

/// Describe every model: its table's columns from `schema`, its relations from
/// the parsed graph, its scopes and namespace from its source file.
pub fn build(nodes: &[ModelNode], schema: &HashMap<String, Vec<ColumnInfo>>) -> Vec<HelperModel> {
    let mut ns_cache: HashMap<std::path::PathBuf, String> = HashMap::new();
    let mut namespace_for = |file: &Path| -> String {
        if let Some(ns) = ns_cache.get(file) {
            return ns.clone();
        }
        let ns = std::fs::read_to_string(file)
            .ok()
            .and_then(|s| namespace_of(&s))
            .unwrap_or_else(|| "App\\Models".to_string());
        ns_cache.insert(file.to_path_buf(), ns.clone());
        ns
    };
    let mut out = Vec::new();
    for node in nodes {
        let Ok(src) = std::fs::read_to_string(&node.file) else {
            continue;
        };
        let namespace = namespace_of(&src).unwrap_or_else(|| "App\\Models".to_string());
        let relations = node
            .relations
            .iter()
            .filter(|r| !r.method.is_empty() && !r.target.is_empty())
            .map(|r| {
                let target_ns = r
                    .target_file
                    .as_ref()
                    .map(|f| namespace_for(f))
                    .unwrap_or_else(|| namespace.clone());
                (
                    r.kind.clone(),
                    r.method.clone(),
                    format!("\\{target_ns}\\{}", r.target),
                )
            })
            .collect();
        out.push(HelperModel {
            namespace,
            class: node.name.clone(),
            columns: schema.get(&node.table).cloned().unwrap_or_default(),
            relations,
            scopes: scopes_of(&src),
        });
    }
    out.sort_by(|a, b| (&a.namespace, &a.class).cmp(&(&b.namespace, &b.class)));
    out
}

/// The helper file's text.
pub fn render(models: &[HelperModel]) -> String {
    let mut out = String::new();
    out.push_str("<?php\n\n");
    out.push_str("// @formatter:off\n// phpcs:ignoreFile\n");
    out.push_str(
        "/**\n * Eloquent helper, generated by e from the live database schema and the\n \
         * models' relationships so the language server knows every attribute,\n \
         * relation and scope everywhere. Regenerated when the schema reloads;\n \
         * `Laravel: Generate Eloquent Helper` does it on demand. Not for git.\n */\n\n",
    );
    for m in models {
        out.push_str(&format!("namespace {} {{\n", m.namespace));
        out.push_str("    /**\n");
        for c in &m.columns {
            out.push_str(&format!("     * @property {} ${}\n", php_type(c), c.name));
        }
        for (kind, method, target) in &m.relations {
            if is_many(kind) {
                out.push_str(&format!(
                    "     * @property-read \\Illuminate\\Database\\Eloquent\\Collection<int, {target}> ${method}\n"
                ));
                out.push_str(&format!("     * @property-read int|null ${method}_count\n"));
            } else {
                out.push_str(&format!("     * @property-read {target}|null ${method}\n"));
            }
        }
        for s in &m.scopes {
            out.push_str(&format!(
                "     * @method static \\Illuminate\\Database\\Eloquent\\Builder<static> {s}()\n"
            ));
        }
        out.push_str(&format!(
            "     * @method static \\Illuminate\\Database\\Eloquent\\Builder<static> query()\n     */\n    class {} extends \\Illuminate\\Database\\Eloquent\\Model {{}}\n}}\n\n",
            m.class
        ));
    }
    out
}

impl AppState {
    /// **Laravel: Generate Eloquent Helper** — write the helper and say so.
    pub fn generate_eloquent_helper(&self) {
        self.regenerate_eloquent_helper(true);
    }

    /// After a schema reload: keep the helper current, if the project has one.
    /// Its presence is the opt-in; nothing is written to a project that never
    /// asked for it.
    pub fn maybe_regenerate_eloquent_helper(&self) {
        if self.root.get_untracked().join(FILE_NAME).is_file() {
            self.regenerate_eloquent_helper(false);
        }
    }

    fn regenerate_eloquent_helper(&self, announce: bool) {
        let root = self.root.get_untracked();
        if !crate::laravel::is_laravel(&root) {
            if announce {
                Self::notify("Not a Laravel project");
            }
            return;
        }
        let cached = self.db.schema_cache.get_untracked();
        let app = *self;
        self.spawn_bg(
            move || -> Result<(std::path::PathBuf, usize), String> {
                // Relations are cross-checked against the foreign keys, and the
                // schema comes from the connection when the cache is still cold.
                let conn = e_db::from_env(&root).and_then(|c| e_db::connect(&c).ok());
                let fks = conn
                    .as_ref()
                    .and_then(|c| e_db::foreign_keys(c).ok())
                    .unwrap_or_default();
                let nodes = crate::relations::build_graph(&root, &fks);
                let fresh: HashMap<String, Vec<ColumnInfo>>;
                let schema: &HashMap<String, Vec<ColumnInfo>> = if cached.is_empty() {
                    let mut map = HashMap::new();
                    if let Some(c) = &conn {
                        for t in e_db::tables(c).unwrap_or_default() {
                            if let Ok(cols) = e_db::columns(c, &t) {
                                map.insert(t, cols);
                            }
                        }
                    }
                    fresh = map;
                    &fresh
                } else {
                    &cached
                };
                let models = build(&nodes, schema);
                if models.is_empty() {
                    return Err("no Eloquent models found under app/Models".into());
                }
                let path = root.join(FILE_NAME);
                std::fs::write(&path, render(&models)).map_err(|e| e.to_string())?;
                Ok((path, models.len()))
            },
            move |res: Result<(std::path::PathBuf, usize), String>| match res {
                Ok((path, n)) => {
                    // The servers index it now, not at their next start.
                    let uri = e_lsp::path_to_uri(&path);
                    for client in app.lsp_clients_for(e_core::language::Language::Php) {
                        client.did_change_watched_files(&[(uri.clone(), 2)]);
                    }
                    eprintln!("e: wrote {FILE_NAME} for {n} models");
                    if announce {
                        Self::notify(&format!(
                            "Eloquent helper written for {n} models ({FILE_NAME}) — add it to .gitignore"
                        ));
                    }
                }
                Err(e) => {
                    eprintln!("e: Eloquent helper: {e}");
                    if announce {
                        Self::notify(&format!("Eloquent helper: {e}"));
                    }
                }
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(name: &str, ty: &str, nullable: bool) -> ColumnInfo {
        ColumnInfo {
            name: name.into(),
            data_type: ty.into(),
            nullable,
            key: String::new(),
        }
    }

    #[test]
    fn maps_column_types_to_php() {
        assert_eq!(php_type(&col("id", "bigint unsigned", false)), "int");
        assert_eq!(php_type(&col("active", "tinyint(1)", false)), "bool");
        assert_eq!(php_type(&col("price", "decimal(8,2)", true)), "float|null");
        assert_eq!(php_type(&col("meta", "json", true)), "array|null");
        assert_eq!(
            php_type(&col("created_at", "timestamp", true)),
            "\\Illuminate\\Support\\Carbon|null"
        );
        assert_eq!(php_type(&col("name", "varchar(255)", false)), "string");
    }

    #[test]
    fn finds_namespace_and_scopes_of_both_styles() {
        let src = "<?php\n\nnamespace App\\Models;\n\nclass Order\n{\n    public function scopeActive($q) {}\n    #[Scope]\n    protected function recent(Builder $q) {}\n    public function scopes() {}\n}\n";
        assert_eq!(namespace_of(src).as_deref(), Some("App\\Models"));
        assert_eq!(scopes_of(src), vec!["active", "recent"]);
    }

    #[test]
    fn renders_properties_relations_and_scopes() {
        let m = HelperModel {
            namespace: "App\\Models".into(),
            class: "User".into(),
            columns: vec![
                col("id", "bigint", false),
                col("name", "varchar(255)", true),
            ],
            relations: vec![
                (
                    "hasMany".into(),
                    "posts".into(),
                    "\\App\\Models\\Post".into(),
                ),
                (
                    "belongsTo".into(),
                    "team".into(),
                    "\\App\\Models\\Team".into(),
                ),
            ],
            scopes: vec!["active".into()],
        };
        let php = render(&[m]);
        assert!(php.starts_with("<?php\n"));
        assert!(php.contains("namespace App\\Models {"));
        assert!(php.contains("     * @property int $id\n"));
        assert!(php.contains("     * @property string|null $name\n"));
        assert!(php.contains("@property-read \\Illuminate\\Database\\Eloquent\\Collection<int, \\App\\Models\\Post> $posts"));
        assert!(php.contains("@property-read int|null $posts_count"));
        assert!(php.contains("@property-read \\App\\Models\\Team|null $team"));
        assert!(php
            .contains("@method static \\Illuminate\\Database\\Eloquent\\Builder<static> active()"));
        assert!(php.contains("class User extends \\Illuminate\\Database\\Eloquent\\Model {}"));
    }
}
