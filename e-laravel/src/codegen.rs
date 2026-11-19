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
        return format!("{base}y");
    }
    // `classes`, `boxes`: the `es` is the plural.
    for suffix in ["sses", "shes", "ches", "xes", "zes"] {
        if s.ends_with(suffix) {
            return s[..s.len() - 2].to_string();
        }
    }
    // `statuses`, `buses` (a consonant before `us`) — but `houses`, `causes`.
    if let Some(stem) = s.strip_suffix("uses") {
        if stem.chars().last().is_some_and(|c| !"aeiou".contains(c)) {
            return s[..s.len() - 2].to_string();
        }
    }
    // `class` is already singular; `houses`, `databases` drop only the `s`.
    if s.ends_with("ss") {
        return s.to_string();
    }
    s.strip_suffix('s').unwrap_or(s).to_string()
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

/// What a factory fills a column with: a relation's factory for a foreign
/// key, else a guess from its name, else from its type. `None` leaves the
/// column out (the key, timestamps, soft deletes).
fn fake_for(col: &ColumnInfo, fk: Option<&ForeignKey>) -> Option<String> {
    let name = col.name.to_ascii_lowercase();
    let t = col.data_type.to_ascii_lowercase();
    let auto_key =
        col.key == "PRI" && (t.contains("int") || t.contains("serial")) && !name.ends_with("_id");
    if auto_key || matches!(name.as_str(), "created_at" | "updated_at" | "deleted_at") {
        return None;
    }
    if let Some(fk) = fk {
        return Some(format!("{}::factory()", model_name(&fk.ref_table)));
    }
    // MySQL's enum('draft','published'): one of its values, as written —
    // `ENUM('PENDING')` is `PENDING`, not `pending`.
    if t.starts_with("enum(") {
        if let Some(values) = enum_values(col.data_type.trim()) {
            let php: Vec<String> = values
                .iter()
                .map(|v| format!("'{}'", v.replace('\\', "\\\\").replace('\'', "\\'")))
                .collect();
            return Some(format!("fake()->randomElement([{}])", php.join(", ")));
        }
    }
    let by_name = match name.as_str() {
        "email" | "email_address" => Some("fake()->unique()->safeEmail()"),
        "name" | "full_name" => Some("fake()->name()"),
        "first_name" | "firstname" => Some("fake()->firstName()"),
        "last_name" | "lastname" | "surname" => Some("fake()->lastName()"),
        "username" | "user_name" | "login" => Some("fake()->unique()->userName()"),
        "title" | "subject" | "headline" => Some("fake()->sentence(4)"),
        "slug" => Some("fake()->unique()->slug()"),
        "status" | "state" | "type" | "kind" | "role" | "category" | "level" => {
            Some("fake()->word()")
        }
        "description" | "body" | "content" | "bio" | "notes" | "summary" | "excerpt"
        | "message" | "comment" => Some("fake()->paragraph()"),
        "phone" | "phone_number" | "mobile" | "telephone" => Some("fake()->phoneNumber()"),
        "address" | "street" | "street_address" => Some("fake()->streetAddress()"),
        "city" | "town" => Some("fake()->city()"),
        "country" => Some("fake()->country()"),
        "country_code" => Some("fake()->countryCode()"),
        "zip" | "zipcode" | "zip_code" | "postal_code" | "postcode" => Some("fake()->postcode()"),
        "url" | "website" | "homepage" | "link" => Some("fake()->url()"),
        "uuid" => Some("fake()->uuid()"),
        "ip" | "ip_address" => Some("fake()->ipv4()"),
        "color" | "colour" => Some("fake()->hexColor()"),
        "company" | "company_name" => Some("fake()->company()"),
        "job_title" => Some("fake()->jobTitle()"),
        "latitude" | "lat" => Some("fake()->latitude()"),
        "longitude" | "lng" | "lon" => Some("fake()->longitude()"),
        "price" | "amount" | "total" | "cost" | "subtotal" | "balance" => {
            Some("fake()->randomFloat(2, 1, 1000)")
        }
        "quantity" | "qty" | "stock" => Some("fake()->numberBetween(1, 100)"),
        "image" | "avatar" | "photo" | "picture" | "image_url" | "avatar_url" => {
            Some("fake()->imageUrl()")
        }
        "password" => Some("Hash::make('password')"),
        "remember_token" => Some("Str::random(10)"),
        "email_verified_at" => Some("now()"),
        _ => None,
    };
    if let Some(f) = by_name {
        return Some(f.to_string());
    }
    let word = t
        .split(|c: char| c == '(' || c.is_whitespace())
        .next()
        .unwrap_or("");
    Some(
        match word {
            _ if t == "tinyint(1)" || word.starts_with("bool") => "fake()->boolean()",
            "int" | "integer" | "bigint" | "smallint" | "mediumint" | "tinyint" | "int2"
            | "int4" | "int8" => "fake()->numberBetween(0, 1000)",
            "decimal" | "numeric" | "float" | "double" | "real" | "float4" | "float8" => {
                "fake()->randomFloat(2, 0, 1000)"
            }
            "date" => "fake()->date()",
            "datetime" | "timestamp" | "timestamptz" => "fake()->dateTime()",
            "time" => "fake()->time()",
            "year" => "fake()->year()",
            "json" | "jsonb" => "[]",
            "uuid" => "fake()->uuid()",
            "text" | "mediumtext" | "longtext" => "fake()->paragraph()",
            _ => "fake()->word()",
        }
        .to_string(),
    )
}

/// The values of MySQL's `enum('a','it''s')`, unquoted: `''` is a quote
/// and `\` escapes, as MySQL writes them in `COLUMN_TYPE`.
fn enum_values(ty: &str) -> Option<Vec<String>> {
    let open = ty.find('(')?;
    let body = ty.get(open + 1..ty.rfind(')')?)?;
    let mut values = Vec::new();
    let mut chars = body.chars().peekable();
    loop {
        while chars.peek().is_some_and(|c| c.is_whitespace() || *c == ',') {
            chars.next();
        }
        match chars.next() {
            None => return Some(values),
            Some('\'') => {}
            Some(_) => return None,
        }
        let mut v = String::new();
        loop {
            match chars.next()? {
                '\\' => v.push(chars.next()?),
                '\'' if chars.peek() == Some(&'\'') => {
                    chars.next();
                    v.push('\'');
                }
                '\'' => break,
                c => v.push(c),
            }
        }
        values.push(v);
    }
}

/// A model factory for `table` from its columns and foreign keys, for
/// `database/factories/{Model}Factory.php`: each column filled by name or
/// type, each foreign key by its model's factory.
pub fn generate_factory(table: &str, cols: &[ColumnInfo], fks: &[ForeignKey]) -> String {
    let model = model_name(table);
    let mut uses: Vec<String> = vec![
        format!("App\\Models\\{model}"),
        "Illuminate\\Database\\Eloquent\\Factories\\Factory".into(),
    ];
    let mut fields = Vec::new();
    // Only plain names go into the PHP: one with a quote in it would end
    // the string it's written in.
    for c in cols.iter().filter(|c| valid_table(&c.name)) {
        let fk = fks.iter().find(|f| f.table == table && f.column == c.name);
        let Some(value) = fake_for(c, fk) else {
            continue;
        };
        if let Some(fk) = fk {
            let related = format!("App\\Models\\{}", model_name(&fk.ref_table));
            if related != uses[0] {
                uses.push(related);
            }
        }
        if value.starts_with("Hash::") {
            uses.push("Illuminate\\Support\\Facades\\Hash".into());
        }
        if value.starts_with("Str::") {
            uses.push("Illuminate\\Support\\Str".into());
        }
        fields.push(format!("            '{}' => {value},", c.name));
    }
    uses.sort();
    uses.dedup();
    let uses: String = uses.iter().map(|u| format!("use {u};\n")).collect();
    format!(
        "<?php\n\nnamespace Database\\Factories;\n\n{uses}\n/**\n * @extends Factory<{model}>\n */\nclass {model}Factory extends Factory\n{{\n    /**\n     * Define the model's default state.\n     *\n     * @return array<string, mixed>\n     */\n    public function definition(): array\n    {{\n        return [\n{}\n        ];\n    }}\n}}\n",
        fields.join("\n")
    )
}

/// Whether a string is a plausible table identifier (guards the command).
pub fn valid_table(t: &str) -> bool {
    !t.is_empty() && t.chars().all(is_ident)
}

/// Convert Unix seconds (UTC) to `(year, month, day, hour, min, sec)` using
/// Howard Hinnant's civil-from-days algorithm (no date crate needed).
fn ymd_hms_from_unix(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, mi, s) = (
        (rem / 3600) as u32,
        ((rem % 3600) / 60) as u32,
        (rem % 60) as u32,
    );
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = y + if m <= 2 { 1 } else { 0 };
    (y, m, d, h, mi, s)
}

/// A Laravel-style migration filename timestamp (`YYYY_MM_DD_HHMMSS`) for now.
pub fn migration_timestamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (y, mo, d, h, mi, s) = ymd_hms_from_unix(secs);
    format!("{y:04}_{mo:02}_{d:02}_{h:02}{mi:02}{s:02}")
}

/// A Laravel migration scaffold that alters `table` — offered instead of running
/// DDL directly, so structure changes go through version control (DB-803).
pub fn change_table_migration(table: &str) -> String {
    format!(
        "<?php\n\n\
         use Illuminate\\Database\\Migrations\\Migration;\n\
         use Illuminate\\Database\\Schema\\Blueprint;\n\
         use Illuminate\\Support\\Facades\\Schema;\n\n\
         return new class extends Migration\n{{\n\
         \x20   public function up(): void\n    {{\n\
         \x20       Schema::table('{table}', function (Blueprint $table) {{\n\
         \x20           // Describe your change, e.g.:\n\
         \x20           // $table->string('new_column')->nullable();\n\
         \x20           // $table->index('some_column');\n\
         \x20       }});\n    }}\n\n\
         \x20   public function down(): void\n    {{\n\
         \x20       Schema::table('{table}', function (Blueprint $table) {{\n\
         \x20           // Reverse the change above.\n\
         \x20       }});\n    }}\n}};\n"
    )
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_factory_fills_columns_by_name_type_and_relation() {
        let col = |name: &str, ty: &str, key: &str| ColumnInfo {
            name: name.into(),
            data_type: ty.into(),
            nullable: false,
            key: key.into(),
        };
        let cols = vec![
            col("id", "bigint unsigned", "PRI"),
            col("user_id", "bigint unsigned", ""),
            col("title", "varchar(255)", ""),
            col("status", "enum('draft','published')", ""),
            col("is_pinned", "tinyint(1)", ""),
            col("views", "int", ""),
            col("meta", "json", ""),
            col("published_at", "timestamp", ""),
            col("created_at", "timestamp", ""),
            col("updated_at", "timestamp", ""),
        ];
        let fks = vec![ForeignKey {
            table: "blog_posts".into(),
            column: "user_id".into(),
            ref_table: "users".into(),
            ref_column: "id".into(),
        }];
        assert_eq!(
            generate_factory("blog_posts", &cols, &fks),
            "<?php\n\nnamespace Database\\Factories;\n\nuse App\\Models\\BlogPost;\nuse App\\Models\\User;\nuse Illuminate\\Database\\Eloquent\\Factories\\Factory;\n\n/**\n * @extends Factory<BlogPost>\n */\nclass BlogPostFactory extends Factory\n{\n    /**\n     * Define the model's default state.\n     *\n     * @return array<string, mixed>\n     */\n    public function definition(): array\n    {\n        return [\n            'user_id' => User::factory(),\n            'title' => fake()->sentence(4),\n            'status' => fake()->randomElement(['draft', 'published']),\n            'is_pinned' => fake()->boolean(),\n            'views' => fake()->numberBetween(0, 1000),\n            'meta' => [],\n            'published_at' => fake()->dateTime(),\n        ];\n    }\n}\n"
        );
    }

    #[test]
    fn an_enum_keeps_its_values_as_written() {
        let col = |name: &str, ty: &str| ColumnInfo {
            name: name.into(),
            data_type: ty.into(),
            nullable: false,
            key: String::new(),
        };
        let f = generate_factory(
            "orders",
            &[
                col("state", "ENUM('PENDING','Paid','it''s')"),
                col("bad'name", "varchar(10)"),
            ],
            &[],
        );
        assert!(
            f.contains("'state' => fake()->randomElement(['PENDING', 'Paid', 'it\\'s']),"),
            "{f}"
        );
        assert!(!f.contains("bad'name"), "{f}");
        assert_eq!(
            enum_values("enum('a\\'b', 'c')"),
            Some(vec!["a'b".to_string(), "c".to_string()])
        );
        assert_eq!(enum_values("enum(a)"), None);
    }

    #[test]
    fn a_users_factory_hashes_its_password() {
        let col = |name: &str, ty: &str| ColumnInfo {
            name: name.into(),
            data_type: ty.into(),
            nullable: true,
            key: String::new(),
        };
        let f = generate_factory(
            "users",
            &[
                col("email", "varchar(255)"),
                col("password", "varchar(255)"),
                col("remember_token", "varchar(100)"),
            ],
            &[],
        );
        assert!(
            f.contains("use Illuminate\\Support\\Facades\\Hash;\nuse Illuminate\\Support\\Str;\n")
        );
        assert!(f.contains("'email' => fake()->unique()->safeEmail(),"));
        assert!(f.contains("'password' => Hash::make('password'),"));
        assert!(f.contains("'remember_token' => Str::random(10),"));
    }

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
    fn civil_date_conversion() {
        // 1700000000 = 2023-11-14 22:13:20 UTC.
        assert_eq!(ymd_hms_from_unix(1_700_000_000), (2023, 11, 14, 22, 13, 20));
        // Unix epoch.
        assert_eq!(ymd_hms_from_unix(0), (1970, 1, 1, 0, 0, 0));
    }

    #[test]
    fn change_migration_scaffold() {
        let m = change_table_migration("orders");
        assert!(m.contains("extends Migration"));
        assert!(m.contains("Schema::table('orders'"));
        assert!(m.contains("public function up"));
        assert!(m.contains("public function down"));
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
