//! New Eloquent Model: a spec you type, and every file it implies.
//!
//! Laravel Idea does this with a dialog of fields, types and checkboxes. `e`
//! does it with text — a small spec in an editor panel (fields with types and
//! defaults, relations, options, what to generate) and a live preview of the
//! files that will be created. ⌘↵ writes them all: the model with fillable,
//! casts and relations; the migration; a factory with sensible fakes; a seeder;
//! Store/Update form requests with rules from the fields; a resource or API
//! controller wired to them; a JSON resource; a policy. A pivot table is the
//! same panel with a `pivot:` line.
//!
//! The parser and generators are pure and tested; the panel is a thin layer.

use std::rc::Rc;

use floem::peniko::Color;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use floem::views::editor::text::Document;
use floem::views::{container, dyn_stack, label, scroll, stack, text_editor, Decorators};
use floem::IntoView;

use crate::eloquent::{pluralize, snake_case};
use crate::state::AppState;
use crate::theme;

pub const TEMPLATE: &str = "\
# New Eloquent Model — edit the spec, then ⌘↵ (or Create).
#
# fields:     name  type[?]  [= default]        ? makes it nullable
#   types: string string(80) text integer bigInteger unsignedInteger boolean
#          decimal(10,2) float date dateTime timestamp json uuid enum(a,b,c)
#          foreignId Model            (a foreign key + belongsTo relation)
# relations:  hasMany Model | hasOne Model | belongsTo Model | belongsToMany Model | morphMany Model
# options:    id timestamps softDeletes fillable      (-timestamps turns one off)
# generate:   migration factory seeder controller api-controller request resource policy

model: Order
table:
fields:
  customer_id   foreignId Customer
  status        string = 'pending'
  total         decimal(10,2)
  notes         text?
  paid_at       timestamp?
relations:
  hasMany OrderLine
options: id timestamps fillable
generate: migration factory request controller
";

pub const PIVOT_TEMPLATE: &str = "\
# New pivot table — the two models it joins, then ⌘↵ (or Create).
# The table is named the Laravel way (post_tag), with both foreign keys,
# a composite primary key, and timestamps when asked for.

pivot: Post Tag
options: timestamps
";

/// One column of the model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    /// The type keyword as written, without `?` or arguments: `string`, `decimal`…
    pub kind: String,
    /// `string(80)` → `["80"]`, `decimal(10,2)` → `["10", "2"]`, `enum(a,b)` → `["a", "b"]`.
    pub args: Vec<String>,
    pub nullable: bool,
    /// The default as written (`'pending'`, `0`, `true`).
    pub default: Option<String>,
    /// For `foreignId`: the related model.
    pub related: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Relation {
    pub kind: String,
    pub model: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spec {
    pub model: String,
    pub table: String,
    pub fields: Vec<Field>,
    pub relations: Vec<Relation>,
    pub id: bool,
    pub timestamps: bool,
    pub soft_deletes: bool,
    pub fillable: bool,
    pub generate: Vec<String>,
    /// `pivot: A B` — a pivot migration instead of a model.
    pub pivot: Option<(String, String)>,
}

/// A file to write, relative to the project root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedFile {
    pub path: String,
    pub content: String,
}

const FIELD_TYPES: &[&str] = &[
    "string",
    "text",
    "integer",
    "bigInteger",
    "unsignedInteger",
    "boolean",
    "decimal",
    "float",
    "date",
    "dateTime",
    "timestamp",
    "json",
    "uuid",
    "enum",
    "foreignId",
];
const RELATION_KINDS: &[&str] = &[
    "hasMany",
    "hasOne",
    "belongsTo",
    "belongsToMany",
    "morphMany",
];
const GENERATORS: &[&str] = &[
    "migration",
    "factory",
    "seeder",
    "controller",
    "api-controller",
    "request",
    "resource",
    "policy",
];

fn is_ident(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .next()
            .map(|c| c.is_ascii_alphabetic() || c == '_')
            .unwrap_or(false)
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn is_class(s: &str) -> bool {
    is_ident(s)
        && s.chars()
            .next()
            .map(|c| c.is_ascii_uppercase())
            .unwrap_or(false)
}

fn lcfirst(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_lowercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// `Customer` → `customers`.
fn table_of(model: &str) -> String {
    pluralize(&snake_case(model))
}

/// Parse a spec. Errors carry the 1-based line they refer to.
pub fn parse_spec(text: &str) -> Result<Spec, Vec<String>> {
    let mut spec = Spec {
        model: String::new(),
        table: String::new(),
        fields: Vec::new(),
        relations: Vec::new(),
        id: true,
        timestamps: true,
        soft_deletes: false,
        fillable: true,
        generate: Vec::new(),
        pivot: None,
    };
    let mut errors = Vec::new();
    #[derive(PartialEq)]
    enum Section {
        None,
        Fields,
        Relations,
    }
    let mut section = Section::None;
    for (i, raw) in text.lines().enumerate() {
        let n = i + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once(':').filter(|(k, _)| is_ident(k.trim())) {
            let key = key.trim();
            let value = value.trim();
            match key {
                "model" => {
                    if !is_class(value) {
                        errors.push(format!(
                            "line {n}: model name must be a class name, got `{value}`"
                        ));
                    }
                    spec.model = value.to_string();
                    section = Section::None;
                    continue;
                }
                "table" => {
                    if !value.is_empty() && !is_ident(value) {
                        errors.push(format!(
                            "line {n}: table name `{value}` isn't a valid identifier"
                        ));
                    }
                    spec.table = value.to_string();
                    section = Section::None;
                    continue;
                }
                "fields" => {
                    section = Section::Fields;
                    continue;
                }
                "relations" => {
                    section = Section::Relations;
                    continue;
                }
                "options" => {
                    section = Section::None;
                    for opt in value.split_whitespace() {
                        let (on, name) = match opt.strip_prefix('-') {
                            Some(rest) => (false, rest),
                            None => (true, opt),
                        };
                        match name {
                            "id" => spec.id = on,
                            "timestamps" => spec.timestamps = on,
                            "softDeletes" => spec.soft_deletes = on,
                            "fillable" => spec.fillable = on,
                            other => errors.push(format!("line {n}: unknown option `{other}`")),
                        }
                    }
                    continue;
                }
                "generate" => {
                    section = Section::None;
                    for g in value.split_whitespace() {
                        if GENERATORS.contains(&g) {
                            if !spec.generate.iter().any(|x| x == g) {
                                spec.generate.push(g.to_string());
                            }
                        } else {
                            errors.push(format!(
                                "line {n}: unknown generator `{g}` (one of {})",
                                GENERATORS.join(" ")
                            ));
                        }
                    }
                    continue;
                }
                "pivot" => {
                    section = Section::None;
                    let parts: Vec<&str> = value.split_whitespace().collect();
                    if parts.len() == 2 && parts.iter().all(|p| is_class(p)) {
                        spec.pivot = Some((parts[0].to_string(), parts[1].to_string()));
                    } else {
                        errors.push(format!(
                            "line {n}: pivot needs two model names, e.g. `pivot: Post Tag`"
                        ));
                    }
                    continue;
                }
                _ => {} // a field like `name: string`? fall through to the section parser
            }
        }
        match section {
            Section::Fields => match parse_field(line) {
                Ok(f) => spec.fields.push(f),
                Err(e) => errors.push(format!("line {n}: {e}")),
            },
            Section::Relations => {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() == 2 && RELATION_KINDS.contains(&parts[0]) && is_class(parts[1]) {
                    spec.relations.push(Relation {
                        kind: parts[0].to_string(),
                        model: parts[1].to_string(),
                    });
                } else {
                    errors.push(format!(
                        "line {n}: expected `<kind> Model` with kind one of {}",
                        RELATION_KINDS.join(" ")
                    ));
                }
            }
            Section::None => errors.push(format!("line {n}: unexpected `{line}`")),
        }
    }
    if spec.pivot.is_none() {
        if spec.model.is_empty() {
            errors.push("no `model:` line".to_string());
        }
        if spec.table.is_empty() && !spec.model.is_empty() {
            spec.table = table_of(&spec.model);
        }
        if spec.generate.is_empty() {
            spec.generate.push("migration".to_string());
        }
    }
    if errors.is_empty() {
        Ok(spec)
    } else {
        Err(errors)
    }
}

/// `customer_id   foreignId Customer` / `status string = 'pending'` / `notes text?`.
fn parse_field(line: &str) -> Result<Field, String> {
    let (decl, default) = match line.split_once('=') {
        Some((d, def)) => (
            d.trim(),
            Some(def.trim().to_string()).filter(|s| !s.is_empty()),
        ),
        None => (line, None),
    };
    let mut parts = decl.split_whitespace();
    let name = parts.next().ok_or("empty field")?;
    if !is_ident(name) {
        return Err(format!("`{name}` isn't a valid column name"));
    }
    let ty = parts
        .next()
        .ok_or_else(|| format!("`{name}` needs a type"))?;
    let (ty, nullable) = match ty.strip_suffix('?') {
        Some(t) => (t, true),
        None => (ty, false),
    };
    let (kind, args) = match ty.split_once('(') {
        Some((k, rest)) => (
            k,
            rest.trim_end_matches(')')
                .split(',')
                .map(|a| a.trim().to_string())
                .filter(|a| !a.is_empty())
                .collect::<Vec<_>>(),
        ),
        None => (ty, Vec::new()),
    };
    if !FIELD_TYPES.contains(&kind) {
        return Err(format!(
            "unknown type `{kind}` (one of {})",
            FIELD_TYPES.join(" ")
        ));
    }
    let related = if kind == "foreignId" {
        let m = parts.next().ok_or_else(|| {
            format!("`{name}` foreignId needs the related model, e.g. `foreignId Customer`")
        })?;
        if !is_class(m) {
            return Err(format!("`{m}` isn't a model name"));
        }
        Some(m.to_string())
    } else {
        None
    };
    if let Some(extra) = parts.next() {
        return Err(format!("unexpected `{extra}` after the type of `{name}`"));
    }
    Ok(Field {
        name: name.to_string(),
        kind: kind.to_string(),
        args,
        nullable,
        default,
        related,
    })
}

// ---- Generators ---------------------------------------------------------------------

fn cast_of(f: &Field) -> Option<String> {
    Some(match f.kind.as_str() {
        "boolean" => "boolean".into(),
        "json" => "array".into(),
        "date" => "date".into(),
        "dateTime" | "timestamp" => "datetime".into(),
        "integer" | "bigInteger" | "unsignedInteger" => "integer".into(),
        "float" => "float".into(),
        "decimal" => format!(
            "decimal:{}",
            f.args.get(1).map(String::as_str).unwrap_or("2")
        ),
        _ => return None,
    })
}

fn relation_return(kind: &str) -> &'static str {
    match kind {
        "hasMany" => "HasMany",
        "hasOne" => "HasOne",
        "belongsTo" => "BelongsTo",
        "belongsToMany" => "BelongsToMany",
        "morphMany" => "MorphMany",
        _ => "Relation",
    }
}

fn model_file(spec: &Spec) -> GeneratedFile {
    let name = &spec.model;
    // belongsTo for every foreign key, then the explicit relations.
    let mut relations: Vec<Relation> = spec
        .fields
        .iter()
        .filter_map(|f| {
            f.related.as_ref().map(|m| Relation {
                kind: "belongsTo".into(),
                model: m.clone(),
            })
        })
        .collect();
    for r in &spec.relations {
        if !relations
            .iter()
            .any(|x| x.kind == r.kind && x.model == r.model)
        {
            relations.push(r.clone());
        }
    }
    let mut uses: Vec<String> = vec![
        "Illuminate\\Database\\Eloquent\\Factories\\HasFactory".into(),
        "Illuminate\\Database\\Eloquent\\Model".into(),
    ];
    for r in &relations {
        uses.push(format!(
            "Illuminate\\Database\\Eloquent\\Relations\\{}",
            relation_return(&r.kind)
        ));
    }
    if spec.soft_deletes {
        uses.push("Illuminate\\Database\\Eloquent\\SoftDeletes".into());
    }
    uses.sort();
    uses.dedup();

    let mut out = String::from("<?php\n\nnamespace App\\Models;\n\n");
    for u in &uses {
        out.push_str(&format!("use {u};\n"));
    }
    out.push_str(&format!(
        "\nclass {name} extends Model\n{{\n    use HasFactory;\n"
    ));
    if spec.soft_deletes {
        out.push_str("    use SoftDeletes;\n");
    }
    if spec.table != table_of(name) {
        out.push_str(&format!("\n    protected $table = '{}';\n", spec.table));
    }
    if spec.fillable && !spec.fields.is_empty() {
        out.push_str("\n    protected $fillable = [\n");
        for f in &spec.fields {
            out.push_str(&format!("        '{}',\n", f.name));
        }
        out.push_str("    ];\n");
    }
    let casts: Vec<String> = spec
        .fields
        .iter()
        .filter_map(|f| cast_of(f).map(|c| format!("            '{}' => '{c}',", f.name)))
        .collect();
    if !casts.is_empty() {
        out.push_str("\n    protected function casts(): array\n    {\n        return [\n");
        out.push_str(&casts.join("\n"));
        out.push_str("\n        ];\n    }\n");
    }
    for r in &relations {
        let method = match r.kind.as_str() {
            "hasMany" | "belongsToMany" | "morphMany" => lcfirst(&pluralize(&r.model)),
            _ => lcfirst(&r.model),
        };
        out.push_str(&format!(
            "\n    public function {method}(): {}\n    {{\n        return $this->{}({}::class);\n    }}\n",
            relation_return(&r.kind),
            r.kind,
            r.model
        ));
    }
    out.push_str("}\n");
    GeneratedFile {
        path: format!("app/Models/{name}.php"),
        content: out,
    }
}

fn column_line(f: &Field) -> String {
    let n = &f.name;
    let mut s = match f.kind.as_str() {
        "string" => match f.args.first() {
            Some(len) => format!("$table->string('{n}', {len})"),
            None => format!("$table->string('{n}')"),
        },
        "decimal" => format!(
            "$table->decimal('{n}', {}, {})",
            f.args.first().map(String::as_str).unwrap_or("8"),
            f.args.get(1).map(String::as_str).unwrap_or("2")
        ),
        "enum" => format!(
            "$table->enum('{n}', [{}])",
            f.args
                .iter()
                .map(|a| format!("'{a}'"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        "foreignId" => format!("$table->foreignId('{n}')->constrained()->cascadeOnDelete()"),
        other => format!("$table->{other}('{n}')"),
    };
    if f.nullable {
        s.push_str("->nullable()");
    }
    if let Some(d) = &f.default {
        s.push_str(&format!("->default({d})"));
    }
    s.push(';');
    s
}

fn migration_file(spec: &Spec, timestamp: &str) -> GeneratedFile {
    let table = &spec.table;
    let mut body = String::new();
    if spec.id {
        body.push_str("            $table->id();\n");
    }
    for f in &spec.fields {
        body.push_str(&format!("            {}\n", column_line(f)));
    }
    if spec.timestamps {
        body.push_str("            $table->timestamps();\n");
    }
    if spec.soft_deletes {
        body.push_str("            $table->softDeletes();\n");
    }
    GeneratedFile {
        path: format!("database/migrations/{timestamp}_create_{table}_table.php"),
        content: migration_wrapper(table, &body),
    }
}

fn migration_wrapper(table: &str, body: &str) -> String {
    format!(
        "<?php\n\nuse Illuminate\\Database\\Migrations\\Migration;\nuse Illuminate\\Database\\Schema\\Blueprint;\nuse Illuminate\\Support\\Facades\\Schema;\n\n\
return new class extends Migration\n{{\n    /**\n     * Run the migrations.\n     */\n    public function up(): void\n    {{\n        Schema::create('{table}', function (Blueprint $table) {{\n{body}        }});\n    }}\n\n\
    /**\n     * Reverse the migrations.\n     */\n    public function down(): void\n    {{\n        Schema::dropIfExists('{table}');\n    }}\n}};\n"
    )
}

fn pivot_migration(a: &str, b: &str, timestamps: bool, timestamp: &str) -> GeneratedFile {
    let mut names = [snake_case(a), snake_case(b)];
    names.sort();
    let table = format!("{}_{}", names[0], names[1]);
    let mut body = String::new();
    for n in &names {
        body.push_str(&format!(
            "            $table->foreignId('{n}_id')->constrained()->cascadeOnDelete();\n"
        ));
    }
    body.push_str(&format!(
        "            $table->primary(['{}_id', '{}_id']);\n",
        names[0], names[1]
    ));
    if timestamps {
        body.push_str("            $table->timestamps();\n");
    }
    GeneratedFile {
        path: format!("database/migrations/{timestamp}_create_{table}_table.php"),
        content: migration_wrapper(&table, &body),
    }
}

fn fake_for(f: &Field) -> String {
    if let Some(d) = &f.default {
        return d.clone();
    }
    if let Some(m) = &f.related {
        return format!("{m}::factory()");
    }
    let n = f.name.to_lowercase();
    let by_name = if n.contains("email") {
        Some("fake()->safeEmail()")
    } else if n == "name" || n.ends_with("_name") {
        Some("fake()->name()")
    } else if n.contains("title") {
        Some("fake()->sentence(3)")
    } else if n.contains("slug") {
        Some("fake()->slug()")
    } else if n.contains("phone") {
        Some("fake()->phoneNumber()")
    } else if n.contains("url") {
        Some("fake()->url()")
    } else if n.contains("password") {
        Some("bcrypt('password')")
    } else {
        None
    };
    if let Some(v) = by_name {
        if f.kind == "string" || f.kind == "text" {
            return v.to_string();
        }
    }
    match f.kind.as_str() {
        "string" => "fake()->word()".into(),
        "text" => "fake()->paragraph()".into(),
        "integer" | "bigInteger" | "unsignedInteger" => "fake()->numberBetween(1, 100)".into(),
        "boolean" => "fake()->boolean()".into(),
        "decimal" | "float" => "fake()->randomFloat(2, 0, 1000)".into(),
        "date" => "fake()->date()".into(),
        "dateTime" | "timestamp" => "fake()->dateTime()".into(),
        "json" => "[]".into(),
        "uuid" => "fake()->uuid()".into(),
        "enum" => format!(
            "fake()->randomElement([{}])",
            f.args
                .iter()
                .map(|a| format!("'{a}'"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        _ => "null".into(),
    }
}

fn factory_file(spec: &Spec) -> GeneratedFile {
    let name = &spec.model;
    let mut uses: Vec<String> = vec![format!("App\\Models\\{name}")];
    for f in &spec.fields {
        if let Some(m) = &f.related {
            uses.push(format!("App\\Models\\{m}"));
        }
    }
    uses.push("Illuminate\\Database\\Eloquent\\Factories\\Factory".into());
    uses.sort();
    uses.dedup();
    let mut out = String::from("<?php\n\nnamespace Database\\Factories;\n\n");
    for u in &uses {
        out.push_str(&format!("use {u};\n"));
    }
    out.push_str(&format!(
        "\n/**\n * @extends Factory<{name}>\n */\nclass {name}Factory extends Factory\n{{\n    protected $model = {name}::class;\n\n    public function definition(): array\n    {{\n        return [\n"
    ));
    for f in &spec.fields {
        out.push_str(&format!("            '{}' => {},\n", f.name, fake_for(f)));
    }
    out.push_str("        ];\n    }\n}\n");
    GeneratedFile {
        path: format!("database/factories/{name}Factory.php"),
        content: out,
    }
}

fn seeder_file(spec: &Spec) -> GeneratedFile {
    let name = &spec.model;
    GeneratedFile {
        path: format!("database/seeders/{name}Seeder.php"),
        content: format!(
            "<?php\n\nnamespace Database\\Seeders;\n\nuse App\\Models\\{name};\nuse Illuminate\\Database\\Seeder;\n\nclass {name}Seeder extends Seeder\n{{\n    public function run(): void\n    {{\n        {name}::factory()->count(10)->create();\n    }}\n}}\n"
        ),
    }
}

fn rule_for(f: &Field, update: bool) -> String {
    let mut rules: Vec<String> = Vec::new();
    rules.push(if f.nullable {
        "nullable".into()
    } else if update {
        "sometimes".into()
    } else {
        "required".into()
    });
    match f.kind.as_str() {
        "string" => {
            rules.push("string".into());
            rules.push(format!(
                "max:{}",
                f.args.first().map(String::as_str).unwrap_or("255")
            ));
        }
        "text" => rules.push("string".into()),
        "integer" | "bigInteger" | "unsignedInteger" => rules.push("integer".into()),
        "boolean" => rules.push("boolean".into()),
        "decimal" | "float" => rules.push("numeric".into()),
        "date" | "dateTime" | "timestamp" => rules.push("date".into()),
        "json" => rules.push("array".into()),
        "uuid" => rules.push("uuid".into()),
        "enum" => rules.push(format!("in:{}", f.args.join(","))),
        "foreignId" => {
            rules.push("integer".into());
            if let Some(m) = &f.related {
                rules.push(format!("exists:{},id", table_of(m)));
            }
        }
        _ => {}
    }
    rules.join("|")
}

fn request_file(spec: &Spec, update: bool) -> GeneratedFile {
    let name = &spec.model;
    let class = format!("{}{name}Request", if update { "Update" } else { "Store" });
    let mut rules = String::new();
    for f in &spec.fields {
        rules.push_str(&format!(
            "            '{}' => '{}',\n",
            f.name,
            rule_for(f, update)
        ));
    }
    GeneratedFile {
        path: format!("app/Http/Requests/{class}.php"),
        content: format!(
            "<?php\n\nnamespace App\\Http\\Requests;\n\nuse Illuminate\\Foundation\\Http\\FormRequest;\n\nclass {class} extends FormRequest\n{{\n    public function authorize(): bool\n    {{\n        return true;\n    }}\n\n    /**\n     * @return array<string, \\Illuminate\\Contracts\\Validation\\ValidationRule|array<mixed>|string>\n     */\n    public function rules(): array\n    {{\n        return [\n{rules}        ];\n    }}\n}}\n"
        ),
    }
}

fn controller_file(spec: &Spec, api: bool) -> GeneratedFile {
    let name = &spec.model;
    let var = lcfirst(name);
    let plural = lcfirst(&pluralize(name));
    let views = snake_case(&pluralize(name));
    let with_requests = spec.generate.iter().any(|g| g == "request");
    let with_resource = spec.generate.iter().any(|g| g == "resource");
    let (store_req, update_req) = if with_requests {
        (
            format!("Store{name}Request"),
            format!("Update{name}Request"),
        )
    } else {
        ("Request".into(), "Request".into())
    };
    let data = |req: &str| {
        if req == "Request" {
            "$request->all()".to_string()
        } else {
            "$request->validated()".to_string()
        }
    };
    let mut uses: Vec<String> = vec![format!("App\\Models\\{name}")];
    if with_requests {
        uses.push(format!("App\\Http\\Requests\\Store{name}Request"));
        uses.push(format!("App\\Http\\Requests\\Update{name}Request"));
    } else {
        uses.push("Illuminate\\Http\\Request".into());
    }
    if api && with_resource {
        uses.push(format!("App\\Http\\Resources\\{name}Resource"));
    }
    let (namespace, dir) = if api {
        uses.push("App\\Http\\Controllers\\Controller".into());
        ("App\\Http\\Controllers\\Api", "app/Http/Controllers/Api")
    } else {
        ("App\\Http\\Controllers", "app/Http/Controllers")
    };
    uses.sort();
    let mut out = format!("<?php\n\nnamespace {namespace};\n\n");
    for u in &uses {
        out.push_str(&format!("use {u};\n"));
    }
    out.push_str(&format!(
        "\nclass {name}Controller extends Controller\n{{\n"
    ));
    if api {
        let wrap = |expr: &str, many: bool| {
            if with_resource {
                if many {
                    format!("{name}Resource::collection({expr})")
                } else {
                    format!("new {name}Resource({expr})")
                }
            } else {
                expr.to_string()
            }
        };
        out.push_str(&format!(
            "    public function index()\n    {{\n        return {};\n    }}\n\n",
            wrap(&format!("{name}::latest()->paginate()"), true)
        ));
        out.push_str(&format!(
            "    public function store({store_req} $request)\n    {{\n        ${var} = {name}::create({});\n\n        return {};\n    }}\n\n",
            data(&store_req),
            wrap(&format!("${var}"), false)
        ));
        out.push_str(&format!(
            "    public function show({name} ${var})\n    {{\n        return {};\n    }}\n\n",
            wrap(&format!("${var}"), false)
        ));
        out.push_str(&format!(
            "    public function update({update_req} $request, {name} ${var})\n    {{\n        ${var}->update({});\n\n        return {};\n    }}\n\n",
            data(&update_req),
            wrap(&format!("${var}"), false)
        ));
        out.push_str(&format!(
            "    public function destroy({name} ${var})\n    {{\n        ${var}->delete();\n\n        return response()->noContent();\n    }}\n"
        ));
    } else {
        out.push_str(&format!(
            "    public function index()\n    {{\n        return view('{views}.index', ['{plural}' => {name}::latest()->paginate()]);\n    }}\n\n\
    public function create()\n    {{\n        return view('{views}.create');\n    }}\n\n\
    public function store({store_req} $request)\n    {{\n        ${var} = {name}::create({});\n\n        return redirect()->route('{views}.show', ${var});\n    }}\n\n\
    public function show({name} ${var})\n    {{\n        return view('{views}.show', ['{var}' => ${var}]);\n    }}\n\n\
    public function edit({name} ${var})\n    {{\n        return view('{views}.edit', ['{var}' => ${var}]);\n    }}\n\n\
    public function update({update_req} $request, {name} ${var})\n    {{\n        ${var}->update({});\n\n        return redirect()->route('{views}.show', ${var});\n    }}\n\n\
    public function destroy({name} ${var})\n    {{\n        ${var}->delete();\n\n        return redirect()->route('{views}.index');\n    }}\n",
            data(&store_req),
            data(&update_req)
        ));
    }
    out.push_str("}\n");
    GeneratedFile {
        path: format!("{dir}/{name}Controller.php"),
        content: out,
    }
}

fn resource_file(spec: &Spec) -> GeneratedFile {
    let name = &spec.model;
    let mut fields = String::new();
    if spec.id {
        fields.push_str("            'id' => $this->id,\n");
    }
    for f in &spec.fields {
        fields.push_str(&format!("            '{0}' => $this->{0},\n", f.name));
    }
    if spec.timestamps {
        fields.push_str("            'created_at' => $this->created_at,\n            'updated_at' => $this->updated_at,\n");
    }
    GeneratedFile {
        path: format!("app/Http/Resources/{name}Resource.php"),
        content: format!(
            "<?php\n\nnamespace App\\Http\\Resources;\n\nuse Illuminate\\Http\\Request;\nuse Illuminate\\Http\\Resources\\Json\\JsonResource;\n\nclass {name}Resource extends JsonResource\n{{\n    /**\n     * @return array<string, mixed>\n     */\n    public function toArray(Request $request): array\n    {{\n        return [\n{fields}        ];\n    }}\n}}\n"
        ),
    }
}

fn policy_file(spec: &Spec) -> GeneratedFile {
    let name = &spec.model;
    let var = lcfirst(name);
    let mut out = format!(
        "<?php\n\nnamespace App\\Policies;\n\nuse App\\Models\\{name};\nuse App\\Models\\User;\n\nclass {name}Policy\n{{\n"
    );
    for (method, with_model) in [
        ("viewAny", false),
        ("view", true),
        ("create", false),
        ("update", true),
        ("delete", true),
        ("restore", true),
        ("forceDelete", true),
    ] {
        let params = if with_model {
            format!("User $user, {name} ${var}")
        } else {
            "User $user".to_string()
        };
        out.push_str(&format!(
            "    public function {method}({params}): bool\n    {{\n        return false;\n    }}\n\n"
        ));
    }
    out.truncate(out.trim_end().len());
    out.push_str("\n}\n");
    GeneratedFile {
        path: format!("app/Policies/{name}Policy.php"),
        content: out,
    }
}

/// Every file the spec implies. `timestamp` is the migration prefix.
pub fn plan(spec: &Spec, timestamp: &str) -> Vec<GeneratedFile> {
    if let Some((a, b)) = &spec.pivot {
        return vec![pivot_migration(a, b, spec.timestamps, timestamp)];
    }
    let mut out = vec![model_file(spec)];
    let wants = |g: &str| spec.generate.iter().any(|x| x == g);
    if wants("migration") {
        out.push(migration_file(spec, timestamp));
    }
    if wants("factory") {
        out.push(factory_file(spec));
    }
    if wants("seeder") {
        out.push(seeder_file(spec));
    }
    if wants("request") {
        out.push(request_file(spec, false));
        out.push(request_file(spec, true));
    }
    if wants("resource") {
        out.push(resource_file(spec));
    }
    if wants("controller") {
        out.push(controller_file(spec, false));
    }
    if wants("api-controller") {
        out.push(controller_file(spec, true));
    }
    if wants("policy") {
        out.push(policy_file(spec));
    }
    out
}

// ---- The panel --------------------------------------------------------------------------

impl AppState {
    pub fn open_model_wizard(&self, pivot: bool) {
        self.model_wizard_pivot.set(pivot);
        self.model_wizard_reset.update(|n| *n += 1);
        self.model_wizard_open.set(true);
    }

    /// Re-read the spec and refresh the preview. Called on the idle tick while
    /// the panel is open, so the file list follows what is typed.
    pub fn refresh_model_wizard_preview(&self) {
        let Some(doc) = self.model_wizard_doc.get_untracked() else {
            return;
        };
        let text = doc.text().to_string();
        let (files, errors) = match parse_spec(&text) {
            Ok(spec) => (
                plan(&spec, "YYYY_MM_DD_HHMMSS")
                    .into_iter()
                    .map(|f| f.path)
                    .collect(),
                Vec::new(),
            ),
            Err(errs) => (Vec::new(), errs),
        };
        if self.model_wizard_files.with_untracked(|f| *f != files) {
            self.model_wizard_files.set(files);
        }
        if self.model_wizard_errors.with_untracked(|e| *e != errors) {
            self.model_wizard_errors.set(errors);
        }
    }

    /// Write every file the spec implies. Existing files are left alone and
    /// reported; the model (or the migration, for a pivot) opens afterwards.
    pub fn model_wizard_create(&self) {
        let Some(doc) = self.model_wizard_doc.get_untracked() else {
            return;
        };
        let text = doc.text().to_string();
        let spec = match parse_spec(&text) {
            Ok(s) => s,
            Err(errs) => {
                self.model_wizard_errors.set(errs);
                return;
            }
        };
        let root = self.root.get_untracked();
        let files = plan(&spec, &crate::codegen::migration_timestamp());
        let mut written = Vec::new();
        let mut skipped = Vec::new();
        for f in &files {
            let path = root.join(&f.path);
            if path.exists() {
                skipped.push(f.path.clone());
                continue;
            }
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            match std::fs::write(&path, &f.content) {
                Ok(()) => written.push(path),
                Err(e) => skipped.push(format!("{} ({e})", f.path)),
            }
        }
        // The servers index the new classes now, not at their next start.
        let uris: Vec<(String, u32)> = written.iter().map(|p| (e_lsp::path_to_uri(p), 1)).collect();
        if !uris.is_empty() {
            for client in self.lsp_clients_for(e_core::language::Language::Php) {
                client.did_change_watched_files(&uris);
            }
        }
        self.fs_rev.update(|r| *r += 1);
        let summary = if skipped.is_empty() {
            format!("Created {} file(s)", written.len())
        } else {
            format!(
                "Created {} file(s); left alone (already exist): {}",
                written.len(),
                skipped.join(", ")
            )
        };
        eprintln!("e: model wizard: {summary}");
        Self::notify(&summary);
        self.model_wizard_open.set(false);
        if let Some(first) = written.first() {
            self.open_path(first.clone());
        }
        // A new migration shows up in the Migrations panel and the data refresh.
        self.load_laravel();
    }
}

pub fn model_wizard_panel(state: AppState) -> impl IntoView {
    let editor = text_editor(TEMPLATE);
    let doc: Rc<dyn Document> = editor.doc();
    state.model_wizard_doc.set(Some(doc.clone()));
    // Swap the template when the panel is opened for a model or a pivot.
    {
        let doc = doc.clone();
        floem::reactive::create_effect(move |_| {
            if state.model_wizard_reset.get() == 0 {
                return;
            }
            let template = if state.model_wizard_pivot.get_untracked() {
                PIVOT_TEMPLATE
            } else {
                TEMPLATE
            };
            let len = doc.text().len();
            doc.edit_single(
                floem::views::editor::core::selection::Selection::region(0, len),
                template,
                floem::views::editor::core::editor::EditType::Other,
            );
        });
    }

    let title = label(move || {
        if state.model_wizard_pivot.get() {
            "New Pivot Table".to_string()
        } else {
            "New Eloquent Model".to_string()
        }
    })
    .style(|s| {
        s.flex_grow(1.0_f32)
            .font_size(13.0)
            .font_bold()
            .color(theme::fg())
    });
    let create = label(|| "Create  ⌘↵".to_string())
        .style(move |s| {
            let base = s
                .padding_horiz(12.0)
                .height(26.0)
                .items_center()
                .border_radius(5.0)
                .font_size(12.0)
                .cursor(floem::style::CursorStyle::Pointer);
            if state.model_wizard_errors.with(|e| e.is_empty()) {
                base.background(theme::accent())
                    .color(Color::from_rgb8(0x14, 0x16, 0x1b))
            } else {
                base.background(theme::bg_hover()).color(theme::fg_dim())
            }
        })
        .on_click_stop(move |_| state.model_wizard_create());
    let close = label(|| "✕".to_string())
        .style(|s| {
            s.padding_horiz(8.0)
                .color(theme::fg_dim())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.color(theme::fg()))
        })
        .on_click_stop(move |_| state.model_wizard_open.set(false));
    let header = stack((title, create, close)).style(|s| {
        s.flex_row()
            .items_center()
            .gap(10.0)
            .padding_horiz(12.0)
            .padding_vert(8.0)
            .width_full()
            .border_bottom(1.0)
            .border_color(theme::border())
    });

    let code = editor
        .style(|s| {
            s.flex_grow(1.0_f32)
                .min_width(0.0)
                .height_full()
                .font_family("monospace".to_string())
                .font_size(13.0)
                .padding(8.0)
        })
        .on_key_down(
            floem::keyboard::Key::Named(floem::keyboard::NamedKey::Enter),
            |m| m.meta() || m.control(),
            move |_| state.model_wizard_create(),
        );

    // Preview: the files that will be created, or what's wrong with the spec.
    let preview_title = label(move || {
        if state.model_wizard_errors.with(|e| !e.is_empty()) {
            "Fix the spec".to_string()
        } else {
            let n = state.model_wizard_files.with(|f| f.len());
            format!("Will create {n} file(s)")
        }
    })
    .style(|s| {
        s.font_size(12.0)
            .font_bold()
            .color(theme::fg())
            .padding(10.0)
    });
    let lines = dyn_stack(
        move || {
            let errors = state.model_wizard_errors.get();
            let items: Vec<(bool, String)> = if errors.is_empty() {
                state
                    .model_wizard_files
                    .get()
                    .into_iter()
                    .map(|f| (false, f))
                    .collect()
            } else {
                errors.into_iter().map(|e| (true, e)).collect()
            };
            items.into_iter().enumerate().collect::<Vec<_>>()
        },
        |(i, _)| *i,
        move |(_, (is_error, text))| {
            label(move || text.clone()).style(move |s| {
                s.font_family("monospace".to_string())
                    .font_size(11.5)
                    .padding_horiz(10.0)
                    .padding_vert(2.0)
                    .width_full()
                    .color(if is_error {
                        Color::from_rgb8(0xe5, 0xc0, 0x7b)
                    } else {
                        theme::fg_dim()
                    })
            })
        },
    )
    .style(|s| s.flex_col().width_full());
    let preview = stack((
        preview_title,
        scroll(lines).style(|s| s.flex_grow(1.0_f32).width_full()),
    ))
    .style(|s| {
        s.flex_col()
            .width(360.0)
            .height_full()
            .border_left(1.0)
            .border_color(theme::border())
            .background(theme::bg_panel())
    });

    let body = stack((code, preview)).style(|s| s.flex_row().width_full().flex_grow(1.0_f32));

    let card = stack((header, body)).style(|s| {
        s.flex_col()
            .width(1000.0)
            .height(620.0)
            .border(1.0)
            .border_color(theme::border())
            .border_radius(10.0)
            .background(theme::bg())
    });

    container(card).style(move |s| {
        let s = s
            .absolute()
            .inset(0.0)
            .size_full()
            .items_center()
            .justify_center()
            .background(Color::from_rgba8(0, 0, 0, 120));
        if state.model_wizard_open.get() {
            s
        } else {
            s.hide()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> Spec {
        parse_spec(TEMPLATE).expect("the template parses")
    }

    #[test]
    fn parses_the_template() {
        let s = spec();
        assert_eq!(s.model, "Order");
        assert_eq!(s.table, "orders");
        assert_eq!(s.fields.len(), 5);
        let cust = &s.fields[0];
        assert_eq!(
            (cust.name.as_str(), cust.kind.as_str()),
            ("customer_id", "foreignId")
        );
        assert_eq!(cust.related.as_deref(), Some("Customer"));
        let status = &s.fields[1];
        assert_eq!(status.default.as_deref(), Some("'pending'"));
        let total = &s.fields[2];
        assert_eq!(total.args, vec!["10", "2"]);
        assert!(s.fields[3].nullable && s.fields[3].kind == "text");
        assert_eq!(
            s.relations,
            vec![Relation {
                kind: "hasMany".into(),
                model: "OrderLine".into()
            }]
        );
        assert!(s.id && s.timestamps && s.fillable && !s.soft_deletes);
        assert_eq!(
            s.generate,
            vec!["migration", "factory", "request", "controller"]
        );
    }

    #[test]
    fn reports_errors_with_line_numbers() {
        let errs = parse_spec("model: order\nfields:\n  name strng\n  x string extra\noptions: -timestamps wat\ngenerate: nope\n")
            .unwrap_err();
        assert!(errs
            .iter()
            .any(|e| e.starts_with("line 1:") && e.contains("class name")));
        assert!(errs
            .iter()
            .any(|e| e.starts_with("line 3:") && e.contains("unknown type `strng`")));
        assert!(errs
            .iter()
            .any(|e| e.starts_with("line 4:") && e.contains("unexpected `extra`")));
        assert!(errs
            .iter()
            .any(|e| e.starts_with("line 5:") && e.contains("unknown option `wat`")));
        assert!(errs
            .iter()
            .any(|e| e.starts_with("line 6:") && e.contains("unknown generator `nope`")));
        // `-timestamps` itself is fine.
        let s = parse_spec("model: A\noptions: -timestamps softDeletes\n").unwrap();
        assert!(!s.timestamps && s.soft_deletes);
        assert_eq!(s.generate, vec!["migration"]);
    }

    #[test]
    fn generates_every_requested_file() {
        let mut s = spec();
        s.generate = GENERATORS.iter().map(|g| g.to_string()).collect();
        s.soft_deletes = true;
        let files = plan(&s, "2026_09_05_120000");
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            paths,
            vec![
                "app/Models/Order.php",
                "database/migrations/2026_09_05_120000_create_orders_table.php",
                "database/factories/OrderFactory.php",
                "database/seeders/OrderSeeder.php",
                "app/Http/Requests/StoreOrderRequest.php",
                "app/Http/Requests/UpdateOrderRequest.php",
                "app/Http/Resources/OrderResource.php",
                "app/Http/Controllers/OrderController.php",
                "app/Http/Controllers/Api/OrderController.php",
                "app/Policies/OrderPolicy.php",
            ]
        );
        let model = &files[0].content;
        assert!(model.contains("use SoftDeletes;"));
        assert!(
            model.contains("protected $fillable = [\n        'customer_id',\n        'status',")
        );
        assert!(model.contains("'total' => 'decimal:2',"));
        assert!(model.contains("'paid_at' => 'datetime',"));
        assert!(model.contains("public function customer(): BelongsTo\n    {\n        return $this->belongsTo(Customer::class);"));
        assert!(model.contains("public function orderLines(): HasMany\n    {\n        return $this->hasMany(OrderLine::class);"));
        assert!(
            !model.contains("protected $table"),
            "conventional table name isn't spelled out"
        );

        let migration = &files[1].content;
        assert!(migration.contains("Schema::create('orders'"));
        assert!(migration
            .contains("$table->foreignId('customer_id')->constrained()->cascadeOnDelete();"));
        assert!(migration.contains("$table->string('status')->default('pending');"));
        assert!(migration.contains("$table->decimal('total', 10, 2);"));
        assert!(migration.contains("$table->text('notes')->nullable();"));
        assert!(migration.contains("$table->timestamps();\n            $table->softDeletes();"));

        let factory = &files[2].content;
        assert!(factory.contains("'customer_id' => Customer::factory(),"));
        assert!(factory.contains("'status' => 'pending',"));
        assert!(factory.contains("'total' => fake()->randomFloat(2, 0, 1000),"));

        let store = &files[4].content;
        assert!(store.contains("'customer_id' => 'required|integer|exists:customers,id',"));
        assert!(store.contains("'status' => 'required|string|max:255',"));
        assert!(store.contains("'notes' => 'nullable|string',"));
        let update = &files[5].content;
        assert!(update.contains("'status' => 'sometimes|string|max:255',"));

        let controller = &files[7].content;
        assert!(controller.contains("public function store(StoreOrderRequest $request)"));
        assert!(controller.contains("Order::create($request->validated())"));
        assert!(controller.contains("return view('orders.index'"));
        let api = &files[8].content;
        assert!(api.contains("namespace App\\Http\\Controllers\\Api;"));
        assert!(api.contains("use App\\Http\\Controllers\\Controller;"));
        assert!(!api.contains("view("));

        let policy = &files[9].content;
        assert!(policy.contains("public function update(User $user, Order $order): bool"));
    }

    #[test]
    fn api_controller_returns_resources() {
        let mut s = spec();
        s.generate = vec!["api-controller".into(), "resource".into()];
        let files = plan(&s, "t");
        let controller = &files
            .iter()
            .find(|f| f.path.ends_with("OrderController.php"))
            .unwrap()
            .content;
        assert!(
            controller.contains("return OrderResource::collection(Order::latest()->paginate());")
        );
        assert!(controller.contains("return new OrderResource($order);"));
        assert!(controller.contains("response()->noContent()"));
        assert!(!controller.contains("view("));
    }

    #[test]
    fn pivot_spec_makes_one_migration() {
        let s = parse_spec(PIVOT_TEMPLATE).unwrap();
        assert_eq!(s.pivot, Some(("Post".into(), "Tag".into())));
        let files = plan(&s, "2026_09_05_120000");
        assert_eq!(files.len(), 1);
        assert_eq!(
            files[0].path,
            "database/migrations/2026_09_05_120000_create_post_tag_table.php"
        );
        let m = &files[0].content;
        assert!(m.contains("$table->foreignId('post_id')->constrained()->cascadeOnDelete();"));
        assert!(m.contains("$table->foreignId('tag_id')->constrained()->cascadeOnDelete();"));
        assert!(m.contains("$table->primary(['post_id', 'tag_id']);"));
        assert!(m.contains("$table->timestamps();"));
    }

    /// Every generated file is valid PHP. Skips when `php` isn't on PATH.
    #[test]
    fn generated_php_lints_clean() {
        if std::process::Command::new("php")
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("skipping: php not on PATH");
            return;
        }
        let mut s = spec();
        s.generate = GENERATORS.iter().map(|g| g.to_string()).collect();
        s.soft_deletes = true;
        let mut files = plan(&s, "2026_09_05_120000");
        files.extend(plan(
            &parse_spec(PIVOT_TEMPLATE).unwrap(),
            "2026_09_05_120001",
        ));
        let dir = std::env::temp_dir().join(format!("e-model-wizard-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for f in &files {
            let path = dir.join(f.path.replace('/', "_"));
            std::fs::write(&path, &f.content).unwrap();
            let out = std::process::Command::new("php")
                .arg("-l")
                .arg(&path)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}: {}{}",
                f.path,
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
