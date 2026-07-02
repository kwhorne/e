//! Code generation from the live database. "Generate model from table" fills in
//! fillable, casts, and relationships from the real schema and foreign keys —
//! PhpStorm has to guess; `e` reads the database.

use e_db::{ColumnInfo, ForeignKey};

fn is_ident(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn pascal(s: &str) -> String {
    s.split('_')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let mut c = p.chars();
            c.next()
                .map(|f| f.to_ascii_uppercase().to_string() + c.as_str())
                .unwrap_or_default()
        })
        .collect()
}

fn camel(s: &str) -> String {
    let p = pascal(s);
    let mut c = p.chars();
    c.next()
        .map(|f| f.to_ascii_lowercase().to_string() + c.as_str())
        .unwrap_or_default()
}

pub fn singularize(s: &str) -> String {
    if let Some(base) = s.strip_suffix("ies") {
        format!("{base}y")
    } else if let Some(base) = s.strip_suffix("es") {
        if base.ends_with('s')
            || base.ends_with('x')
            || base.ends_with("ch")
            || base.ends_with("sh")
        {
            base.to_string()
        } else {
            format!("{base}e")
        }
    } else if let Some(base) = s.strip_suffix('s') {
        base.to_string()
    } else {
        s.to_string()
    }
}

/// Model class name for a table (`order_items` → `OrderItem`).
pub fn model_name(table: &str) -> String {
    pascal(&singularize(table))
}

fn cast_for(col: &ColumnInfo) -> Option<&'static str> {
    let t = col.data_type.to_lowercase();
    if t.contains("bool") || t == "tinyint(1)" {
        Some("boolean")
    } else if t.contains("json") {
        Some("array")
    } else if t.contains("datetime") || t.contains("timestamp") {
        Some("datetime")
    } else if t == "date" {
        Some("date")
    } else {
        None
    }
}

/// Generate an Eloquent model class for `table` from its columns and the
/// project's foreign keys.
pub fn generate_model(table: &str, cols: &[ColumnInfo], fks: &[ForeignKey]) -> String {
    let name = model_name(table);
    let convention_table = format!("{}s", singularize(table)); // rough plural check
    let skip = ["id", "created_at", "updated_at", "deleted_at"];

    let fillable: Vec<String> = cols
        .iter()
        .filter(|c| !skip.contains(&c.name.as_str()))
        .map(|c| format!("'{}'", c.name))
        .collect();

    let casts: Vec<String> = cols
        .iter()
        .filter_map(|c| cast_for(c).map(|t| format!("        '{}' => '{}',", c.name, t)))
        .collect();

    // belongsTo for this table's own foreign keys.
    let mut relations = String::new();
    for fk in fks.iter().filter(|f| f.table == table) {
        let method = camel(fk.column.strip_suffix("_id").unwrap_or(&fk.column));
        let related = model_name(&fk.ref_table);
        relations.push_str(&format!(
            "\n    public function {method}()\n    {{\n        return $this->belongsTo({related}::class);\n    }}\n"
        ));
    }
    // hasMany for tables that reference this one.
    for fk in fks.iter().filter(|f| f.ref_table == table) {
        let method = camel(&fk.table);
        let related = model_name(&fk.table);
        relations.push_str(&format!(
            "\n    public function {method}()\n    {{\n        return $this->hasMany({related}::class);\n    }}\n"
        ));
    }

    let mut out = String::from("<?php\n\nnamespace App\\Models;\n\n");
    out.push_str("use Illuminate\\Database\\Eloquent\\Factories\\HasFactory;\n");
    out.push_str("use Illuminate\\Database\\Eloquent\\Model;\n\n");
    out.push_str(&format!(
        "class {name} extends Model\n{{\n    use HasFactory;\n"
    ));

    // Non-conventional table name.
    if table != convention_table && table != format!("{}s", singularize(table)) {
        out.push_str(&format!("\n    protected $table = '{table}';\n"));
    }

    out.push_str(&format!(
        "\n    protected $fillable = [\n        {}\n    ];\n",
        fillable.join(",\n        ")
    ));

    if !casts.is_empty() {
        out.push_str(&format!(
            "\n    protected $casts = [\n{}\n    ];\n",
            casts.join("\n")
        ));
    }

    out.push_str(&relations);
    out.push_str("}\n");
    out
}

/// Whether a string is a plausible table identifier (guards the command).
pub fn valid_table(t: &str) -> bool {
    !t.is_empty() && t.chars().all(is_ident)
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
    fn generates_model_with_relations() {
        let cols = vec![
            col("id", "bigint", false),
            col("user_id", "bigint", false),
            col("title", "varchar(255)", false),
            col("published", "tinyint(1)", false),
            col("meta", "json", true),
            col("created_at", "timestamp", true),
        ];
        let fks = vec![
            ForeignKey {
                table: "posts".into(),
                column: "user_id".into(),
                ref_table: "users".into(),
                ref_column: "id".into(),
            },
            ForeignKey {
                table: "comments".into(),
                column: "post_id".into(),
                ref_table: "posts".into(),
                ref_column: "id".into(),
            },
        ];
        let m = generate_model("posts", &cols, &fks);
        assert!(m.contains("class Post extends Model"));
        assert!(m.contains("'title'"));
        assert!(!m.contains("'id'")); // id not fillable
        assert!(m.contains("'published' => 'boolean'"));
        assert!(m.contains("'meta' => 'array'"));
        assert!(m.contains("public function user()")); // belongsTo
        assert!(m.contains("return $this->belongsTo(User::class)"));
        assert!(m.contains("public function comments()")); // hasMany
        assert!(m.contains("return $this->hasMany(Comment::class)"));
    }

    #[test]
    fn names_models() {
        assert_eq!(model_name("order_items"), "OrderItem");
        assert_eq!(model_name("categories"), "Category");
    }
}
