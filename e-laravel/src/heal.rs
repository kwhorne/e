//! Contract Heal: one form field, carried through the stack.
//!
//! A field added to a form — `v-model="form.phone"` on an Inertia page,
//! `name="phone"` in a Blade form, `wire:model="phone"` in a Livewire view —
//! means the same field in four other places: the form's own state, the
//! validation rules, the model's `$fillable` (and cast), and a column. Each is
//! easy to forget, and the request fails, or silently drops the value, until
//! the last one is done. This follows the form to where it is submitted —
//! route, controller or component, FormRequest, the model it writes — and
//! works out every edit, plus the migration when the live schema lacks the
//! column. Nothing is written here: the editor shows the plan first.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use e_db::ColumnInfo;

use crate::authz::{matching, methods, typed_params};
use crate::eloquent::{pluralize, snake_case};
use crate::laravel::{route_for, RouteInfo};

// ---- The field ------------------------------------------------------------

/// Where the field was found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    /// An Inertia `useForm` object, by its variable (`form`).
    Inertia { form: String },
    /// An input in a Blade `<form>`.
    Blade,
    /// A Livewire `wire:model`, by its full path (`form.phone`).
    Livewire { path: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldRef {
    pub field: String,
    pub origin: Origin,
}

fn is_ident(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// The value of an attribute starting with `prefix` (`wire:model` matches
/// `wire:model.live`) whose value holds `offset`.
fn attr_value_at(text: &str, offset: usize, prefix: &str) -> Option<String> {
    let start = text[..offset.min(text.len())]
        .rfind('\n')
        .map_or(0, |i| i + 1);
    let end = text[start..].find('\n').map_or(text.len(), |i| start + i);
    let line = &text[start..end];
    let mut from = 0;
    while let Some(at) = line[from..].find(prefix) {
        let at = from + at;
        from = at + prefix.len();
        if at > 0 && is_ident(line.as_bytes()[at - 1] as char) {
            continue;
        }
        let rest = &line[from..];
        let mods = rest
            .find(|c: char| !(is_ident(c) || c == '.' || c == '-'))
            .unwrap_or(rest.len());
        // `name=`: the quote follows directly; `wire:model.live=` after the
        // modifiers.
        let after = if prefix.ends_with('=') {
            rest
        } else if rest[mods..].starts_with('=') {
            &rest[mods + 1..]
        } else {
            continue;
        };
        let Some(q) = after.chars().next().filter(|c| *c == '"' || *c == '\'') else {
            continue;
        };
        let vstart = start + (line.len() - after.len()) + 1;
        let Some(len) = text[vstart..end].find(q) else {
            continue;
        };
        if (vstart..=vstart + len).contains(&offset) {
            return Some(text[vstart..vstart + len].to_string());
        }
    }
    None
}

/// The variables a component assigns `useForm(…)` to.
fn form_vars(text: &str) -> Vec<String> {
    text.match_indices("useForm")
        .filter_map(|(i, _)| {
            let before = text[..i].trim_end().strip_suffix('=')?.trim_end();
            let var: String = before
                .chars()
                .rev()
                .take_while(|c| is_ident(*c) || *c == '$')
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            (!var.is_empty()).then_some(var)
        })
        .collect()
}

/// The `{…}` object a form variable's `useForm(…)` starts with: the index of
/// its `{`.
fn form_object(text: &str, var: &str) -> Option<usize> {
    let decl = text.match_indices("useForm").find(|(i, _)| {
        text[..*i]
            .trim_end()
            .strip_suffix('=')
            .is_some_and(|b| b.trim_end().ends_with(var))
    })?;
    let paren = text[decl.0..].find('(')? + decl.0;
    text[paren..].find('{').map(|i| paren + i)
}

/// The form field under the cursor, if the cursor is on one.
pub fn field_at(text: &str, offset: usize, blade: bool) -> Option<FieldRef> {
    if blade {
        if let Some(path) = attr_value_at(text, offset, "wire:model") {
            let field = path.rsplit('.').next()?.to_string();
            return (!field.is_empty()).then_some(FieldRef {
                field,
                origin: Origin::Livewire { path },
            });
        }
        let name = attr_value_at(text, offset, "name=")?;
        let field = name.trim_end_matches("[]").to_string();
        return (!field.is_empty() && field.chars().all(is_ident) && !field.starts_with('_'))
            .then_some(FieldRef {
                field,
                origin: Origin::Blade,
            });
    }
    // `form.phone`, `form.errors.phone`.
    let b = text.as_bytes();
    let at = offset.min(text.len());
    let mut s = at;
    while s > 0 && (is_ident(b[s - 1] as char) || b[s - 1] == b'.') {
        s -= 1;
    }
    let mut e = at;
    while e < b.len() && is_ident(b[e] as char) {
        e += 1;
    }
    let token = &text[s..e];
    let vars = form_vars(text);
    let mut parts = token.split('.');
    if let Some(var) = parts.next().filter(|v| vars.iter().any(|x| x == v)) {
        let rest: Vec<&str> = parts.collect();
        let field = match rest.as_slice() {
            [f] => *f,
            ["errors", f] => *f,
            _ => "",
        };
        // Not one of the form's own methods.
        let methods = [
            "post",
            "put",
            "patch",
            "delete",
            "get",
            "submit",
            "reset",
            "errors",
            "processing",
            "progress",
            "hasErrors",
            "isDirty",
            "transform",
            "defaults",
            "clearErrors",
            "setError",
            "data",
            "wasSuccessful",
            "recentlySuccessful",
            "cancel",
        ];
        if !field.is_empty() && !methods.contains(&field) {
            return Some(FieldRef {
                field: field.to_string(),
                origin: Origin::Inertia {
                    form: var.to_string(),
                },
            });
        }
    }
    // A key of the `useForm({ … })` object itself.
    for var in vars {
        let Some(open) = form_object(text, &var) else {
            continue;
        };
        let Some(close) = matching(text, open, (b'{', b'}')) else {
            continue;
        };
        if !(open..close).contains(&at) {
            continue;
        }
        let line_start = text[..at].rfind(['\n', '{', ',']).map_or(0, |i| i + 1);
        let key: String = text[line_start..]
            .trim_start()
            .chars()
            .take_while(|c| is_ident(*c))
            .collect();
        if !key.is_empty() {
            return Some(FieldRef {
                field: key,
                origin: Origin::Inertia { form: var },
            });
        }
    }
    None
}

// ---- What kind of field --------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    String,
    Text,
    Email,
    Integer,
    Decimal,
    Boolean,
    Date,
    DateTime,
    Json,
    ForeignId,
}

/// The kind of a field, from its name and the value the form starts it with.
pub fn kind_of(field: &str, init: Option<&str>) -> Kind {
    let f = field.to_ascii_lowercase();
    let init = init.map(str::trim);
    let any = |words: &[&str]| {
        words
            .iter()
            .any(|w| f == *w || f.ends_with(&format!("_{w}")))
    };
    if f.ends_with("_id") {
        Kind::ForeignId
    } else if matches!(init, Some("true" | "false"))
        || ["is_", "has_", "can_", "should_"]
            .iter()
            .any(|p| f.starts_with(p))
        || any(&[
            "enabled",
            "active",
            "visible",
            "published",
            "verified",
            "confirmed",
        ])
    {
        Kind::Boolean
    } else if f.ends_with("_at") {
        Kind::DateTime
    } else if any(&["date", "on", "birthday", "dob"]) {
        Kind::Date
    } else if f.contains("email") {
        Kind::Email
    } else if any(&[
        "price", "amount", "total", "cost", "rate", "balance", "fee", "salary", "vat",
    ]) || init.is_some_and(|i| i.contains('.') && i.parse::<f64>().is_ok())
    {
        Kind::Decimal
    } else if any(&[
        "count", "quantity", "qty", "age", "year", "position", "order", "number", "stock",
    ]) || init.is_some_and(|i| i.parse::<i64>().is_ok())
    {
        Kind::Integer
    } else if any(&[
        "description",
        "notes",
        "note",
        "body",
        "content",
        "bio",
        "message",
        "comment",
        "summary",
        "text",
        "about",
    ]) {
        Kind::Text
    } else if matches!(init, Some(i) if i.starts_with('[') || i.starts_with('{')) {
        Kind::Json
    } else {
        Kind::String
    }
}

/// `customer_id` → `customers`.
fn table_of_key(field: &str) -> String {
    pluralize(field.trim_end_matches("_id"))
}

impl Kind {
    /// The validation rule, as its parts.
    pub fn rule(self, field: &str) -> Vec<String> {
        let parts: Vec<String> = match self {
            Kind::String => vec!["nullable", "string", "max:255"]
                .into_iter()
                .map(String::from)
                .collect(),
            Kind::Text => vec!["nullable".into(), "string".into()],
            Kind::Email => vec!["nullable".into(), "email".into(), "max:255".into()],
            Kind::Integer => vec!["nullable".into(), "integer".into()],
            Kind::Decimal => vec!["nullable".into(), "numeric".into()],
            Kind::Boolean => vec!["boolean".into()],
            Kind::Date | Kind::DateTime => vec!["nullable".into(), "date".into()],
            Kind::Json => vec!["nullable".into(), "array".into()],
            Kind::ForeignId => vec![
                "nullable".into(),
                format!("exists:{},id", table_of_key(field)),
            ],
        };
        parts
    }

    /// The migration's column.
    pub fn column(self, field: &str) -> String {
        match self {
            Kind::String | Kind::Email => format!("$table->string('{field}')->nullable();"),
            Kind::Text => format!("$table->text('{field}')->nullable();"),
            Kind::Integer => format!("$table->integer('{field}')->nullable();"),
            Kind::Decimal => format!("$table->decimal('{field}', 10, 2)->nullable();"),
            Kind::Boolean => format!("$table->boolean('{field}')->default(false);"),
            Kind::Date => format!("$table->date('{field}')->nullable();"),
            Kind::DateTime => format!("$table->timestamp('{field}')->nullable();"),
            Kind::Json => format!("$table->json('{field}')->nullable();"),
            Kind::ForeignId => {
                format!("$table->foreignId('{field}')->nullable()->constrained()->nullOnDelete();")
            }
        }
    }

    fn drop(self, field: &str) -> String {
        match self {
            Kind::ForeignId => format!("$table->dropConstrainedForeignId('{field}');"),
            _ => format!("$table->dropColumn('{field}');"),
        }
    }

    pub fn cast(self) -> Option<&'static str> {
        match self {
            Kind::Boolean => Some("boolean"),
            Kind::Date => Some("date"),
            Kind::DateTime => Some("datetime"),
            Kind::Json => Some("array"),
            Kind::Decimal => Some("decimal:2"),
            _ => None,
        }
    }

    fn ts(self) -> &'static str {
        match self {
            Kind::Integer | Kind::Decimal => "number | null",
            Kind::ForeignId => "number | null",
            Kind::Boolean => "boolean",
            Kind::Json => "unknown[]",
            _ => "string",
        }
    }

    fn js_init(self) -> &'static str {
        match self {
            Kind::Boolean => "false",
            Kind::Integer | Kind::Decimal | Kind::ForeignId => "null",
            Kind::Json => "[]",
            _ => "''",
        }
    }

    /// A Livewire property's type and starting value.
    fn php_property(self) -> (&'static str, &'static str) {
        match self {
            Kind::Boolean => ("bool", "false"),
            Kind::Integer | Kind::ForeignId => ("?int", "null"),
            Kind::Decimal => ("?float", "null"),
            Kind::Json => ("array", "[]"),
            Kind::Date | Kind::DateTime => ("?string", "null"),
            _ => ("string", "''"),
        }
    }
}

// ---- Text edits -------------------------------------------------------------

/// The indentation of the line `at` is on.
fn indent_at(src: &str, at: usize) -> String {
    let start = src[..at].rfind('\n').map_or(0, |i| i + 1);
    src[start..]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect()
}

/// Whether a list's inner text has an entry keyed `key` (`'key' =>`,
/// `key:`, or the bare `'key'` of a list).
fn has_key(inner: &str, key: &str) -> bool {
    [format!("'{key}'"), format!("\"{key}\"")]
        .iter()
        .any(|k| inner.contains(k.as_str()))
        || inner.match_indices(key).any(|(i, _)| {
            let before = inner[..i].chars().last();
            let after = inner[i + key.len()..].trim_start();
            !before.is_some_and(|c| is_ident(c) || c == '.' || c == '$')
                && (after.starts_with(':') || after.starts_with("?:"))
        })
}

/// `entry` added at the end of the list or object whose bracket opens at
/// `open`, laid out as its other entries are; `sep` ends an entry (`,`, or
/// `;` in a TypeScript interface). `None` when `key` is there already.
fn insert_entry(src: &str, open: usize, key: &str, entry: &str, sep: char) -> Option<String> {
    let (o, c) = match src.as_bytes()[open] {
        b'[' => (b'[', b']'),
        _ => (b'{', b'}'),
    };
    let close = matching(src, open, (o, c))?;
    let inner = &src[open + 1..close];
    if has_key(inner, key) {
        return None;
    }
    let mut out = String::with_capacity(src.len() + entry.len() + 16);
    if inner.trim().is_empty() {
        let base = indent_at(src, open);
        out.push_str(&src[..=open]);
        out.push_str(&format!("\n{base}    {entry}{sep}\n{base}"));
        out.push_str(&src[close..]);
        return Some(out);
    }
    if !inner.contains('\n') {
        let trimmed = inner.trim_end();
        let lead = if trimmed.ends_with(sep) {
            " ".to_string()
        } else {
            format!("{sep} ")
        };
        out.push_str(&src[..open + 1 + trimmed.len()]);
        out.push_str(&lead);
        out.push_str(entry);
        out.push_str(&src[open + 1 + trimmed.len()..]);
        return Some(out);
    }
    // One per line: after the last line that is code, not a comment.
    let lines: Vec<(usize, &str)> = inner
        .split_inclusive('\n')
        .scan(open + 1, |pos, l| {
            let at = *pos;
            *pos += l.len();
            Some((at, l))
        })
        .collect();
    let (at, last) = lines.iter().rev().find(|(_, l)| {
        let t = l.trim();
        !t.is_empty() && !t.starts_with("//") && !t.starts_with('#') && !t.starts_with('*')
    })?;
    let code = last.trim_end();
    let end = at + code.len();
    let indent: String = last
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();
    out.push_str(&src[..end]);
    if !code.ends_with(sep) && !code.ends_with([',', ';', '[', '{']) {
        out.push(sep);
    }
    out.push_str(&format!("\n{indent}{entry}{sep}"));
    out.push_str(&src[end..]);
    Some(out)
}

/// A validation rule entry, in the style of the rules beside it: an array
/// (`['nullable', 'string']`) or a pipe string (`'nullable|string'`).
fn rule_entry(inner: &str, field: &str, rule: &[String]) -> String {
    let piped = !inner.contains("=> [") && inner.contains("=> '");
    if piped {
        format!("'{field}' => '{}'", rule.join("|"))
    } else {
        format!(
            "'{field}' => [{}]",
            rule.iter()
                .map(|r| format!("'{r}'"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

/// The `[` of the array a `rules()` method returns.
fn rules_array(src: &str) -> Option<usize> {
    let m = methods(src).into_iter().find(|m| m.name == "rules")?;
    let body_at = src.find(m.body)?;
    let ret = m.body.rfind("return")? + 6;
    let rest = &m.body[ret..];
    let t = rest.trim_start();
    if t.starts_with('[') {
        return Some(body_at + ret + (rest.len() - t.len()));
    }
    // `$rules = […]; … return $rules;`
    let var: String = t
        .strip_prefix('$')?
        .chars()
        .take_while(|c| is_ident(*c))
        .collect();
    let assign = format!("${var} = [");
    m.body.find(&assign).map(|i| body_at + i + assign.len() - 1)
}

/// The `[` of the rules in a `validate([…])` call within `body` (at `base`).
fn validate_array(body: &str, base: usize) -> Option<usize> {
    ["validate(", "Validator::make("].iter().find_map(|call| {
        body.match_indices(call).find_map(|(i, _)| {
            let at = i + call.len();
            let args = &body[at..];
            // `$request->validate([…])`, `Validator::make($input, […])`.
            let open = args.find('[')?;
            let before = &args[..open];
            (before.trim().is_empty() || before.trim_end().ends_with(','))
                .then_some(base + at + open)
        })
    })
}

/// `'field' => $input['field'] ?? null` added to the arrays a Fortify action
/// fills the user from (`forceFill([…])`, `User::create([…])`) — the ones
/// built from `$input`.
fn add_input(src: &str, field: &str) -> Option<String> {
    let mut out = src.to_string();
    let mut changed = false;
    let mut from = 0;
    while let Some(i) = ["forceFill([", "::create(["]
        .iter()
        .filter_map(|c| out[from..].find(c).map(|j| from + j + c.len() - 1))
        .min()
    {
        from = i + 1;
        let Some(close) = matching(&out, i, (b'[', b']')) else {
            continue;
        };
        if !out[i..close].contains("$input[") {
            continue;
        }
        let entry = format!("'{field}' => $input['{field}'] ?? null");
        if let Some(new) = insert_entry(&out, i, field, &entry, ',') {
            out = new;
            changed = true;
        }
    }
    changed.then_some(out)
}

/// A rule for `field` added to wherever `src` validates — a `rules()`
/// method, or a `validate([…])` call in `method` (the whole class when
/// `None`).
fn add_rule(
    src: &str,
    method: Option<&str>,
    field: &str,
    rule: &[String],
) -> Result<Option<String>, ()> {
    let open = rules_array(src).or_else(|| match method {
        Some(name) => {
            let m = methods(src).into_iter().find(|m| m.name == name)?;
            validate_array(m.body, src.find(m.body)?)
        }
        None => validate_array(src, 0),
    });
    let Some(open) = open else {
        return Err(());
    };
    let close = matching(src, open, (b'[', b']')).ok_or(())?;
    let entry = rule_entry(&src[open + 1..close], field, rule);
    Ok(insert_entry(src, open, field, &entry, ','))
}

/// What a model says about filling `field`.
#[derive(Debug, PartialEq)]
pub enum Fillable {
    /// Added to `$fillable`.
    Added(String),
    /// Already fillable, or everything is (`$guarded = []`).
    Already,
    /// The model declares neither.
    Undeclared,
}

pub fn add_fillable(src: &str, field: &str) -> Fillable {
    let open = src
        .find("$fillable")
        .and_then(|i| src[i..].find('[').map(|j| i + j))
        .or_else(|| {
            src.find("#[Fillable(")
                .and_then(|i| src[i..].find('[').map(|j| i + j))
        });
    match open {
        Some(open) => match insert_entry(src, open, field, &format!("'{field}'"), ',') {
            Some(s) => Fillable::Added(s),
            None => Fillable::Already,
        },
        None if src.contains("$guarded") || src.contains("#[Unguarded") => Fillable::Already,
        None => Fillable::Undeclared,
    }
}

/// `field` cast as `cast` in the model's `casts()` (or `$casts`), adding
/// the method when it has neither. `None` when it is cast already.
pub fn add_cast(src: &str, field: &str, cast: &str) -> Option<String> {
    let entry = format!("'{field}' => '{cast}'");
    let open = methods(src)
        .into_iter()
        .find(|m| m.name == "casts")
        .and_then(|m| {
            let at = src.find(m.body)?;
            m.body.find('[').map(|i| at + i)
        })
        .or_else(|| {
            src.find("$casts")
                .and_then(|i| src[i..].find('[').map(|j| i + j))
        });
    if let Some(open) = open {
        return insert_entry(src, open, field, &entry, ',');
    }
    let close = src.rfind('}')?;
    let head = src[..close].trim_end();
    Some(format!(
        "{head}\n\n    protected function casts(): array\n    {{\n        return [\n            {entry},\n        ];\n    }}\n{}",
        &src[close..]
    ))
}

/// A public property for `field` on a Livewire component or form, after the
/// last one there, with a `#[Validate]` when its neighbours have them.
fn add_property(src: &str, field: &str, kind: Kind, rule: &[String]) -> Option<String> {
    if src.contains(&format!("${field};")) || src.contains(&format!("${field} =")) {
        return None;
    }
    let (ty, init) = kind.php_property();
    // The last `public … $x …;` that is a property, not a method.
    let mut last_end = None;
    let mut indent = "    ".to_string();
    let mut from = 0;
    while let Some(i) = src[from..].find("public ") {
        let at = from + i;
        from = at + 7;
        let stmt_end = match src[at..].find([';', '(', '{']) {
            Some(j) if src.as_bytes()[at + j] == b';' => at + j,
            _ => continue,
        };
        if !src[at..stmt_end].contains('$') || src[at..stmt_end].contains("function") {
            continue;
        }
        last_end = Some(stmt_end + 1);
        indent = indent_at(src, at);
    }
    let at = match last_end {
        Some(at) => at,
        None => {
            let class = src.find("class ")?;
            src[class..].find('{').map(|i| class + i + 1)?
        }
    };
    let attribute = if src.contains("#[Validate(") {
        format!("{indent}#[Validate('{}')]\n", rule.join("|"))
    } else {
        String::new()
    };
    // A property with an attribute stands apart; a bare one joins the list.
    let gap = if attribute.is_empty() && last_end.is_some() {
        "\n"
    } else {
        "\n\n"
    };
    let property = format!("{attribute}{indent}public {ty} ${field} = {init};");
    Some(match last_end {
        Some(at) => format!("{}{gap}{property}{}", &src[..at], &src[at..]),
        None => format!("{}\n{property}\n{}", &src[..at], &src[at..]),
    })
}

/// The model a piece of code writes: `Order::create(…)`, `$order->update(…)`
/// on a typed `$order`, `$request->user()->update(…)`, `->orders()->create(…)`.
pub fn written_model(
    code: &str,
    typed: &[(String, String)],
    is_model: &dyn Fn(&str) -> bool,
) -> Option<String> {
    let mut found: Vec<(usize, String)> = Vec::new();
    for verb in [
        "::create(",
        "::updateOrCreate(",
        "::firstOrCreate(",
        "::forceCreate(",
        "::query()->create(",
    ] {
        for (i, _) in code.match_indices(verb) {
            let name: String = code[..i]
                .chars()
                .rev()
                .take_while(|c| is_ident(*c))
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            if is_model(&name) {
                found.push((i, name));
            }
        }
    }
    for (i, _) in code.match_indices("new ") {
        let name: String = code[i + 4..].chars().take_while(|c| is_ident(*c)).collect();
        if is_model(&name) {
            found.push((i, name));
        }
    }
    for verb in ["->update(", "->fill(", "->save(", "->forceFill("] {
        for (i, _) in code.match_indices(verb) {
            let before = &code[..i];
            if before.ends_with("user()") || before.ends_with("->user") {
                if is_model("User") {
                    found.push((i, "User".into()));
                }
                continue;
            }
            let var: String = before
                .chars()
                .rev()
                .take_while(|c| is_ident(*c))
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            if let Some((ty, _)) = typed.iter().find(|(_, v)| *v == var) {
                if is_model(ty) {
                    found.push((i, ty.clone()));
                }
            }
        }
    }
    for (i, _) in code.match_indices("()->create(") {
        let rel: String = code[..i]
            .chars()
            .rev()
            .take_while(|c| is_ident(*c))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        let model = crate::codegen::model_name(&snake_case(&rel));
        if is_model(&model) {
            found.push((i, model));
        }
    }
    found.sort_by_key(|(i, _)| *i);
    found.into_iter().next().map(|(_, m)| m)
}

/// `(type, var)` for each typed property of a class: `public Order $order`.
fn typed_properties(src: &str) -> Vec<(String, String)> {
    src.match_indices("public ")
        .filter_map(|(i, _)| {
            let decl = &src[i + 7..];
            let end = decl.find([';', '=', '('])?;
            let decl = &decl[..end];
            let dollar = decl.find('$')?;
            let ty = decl[..dollar]
                .split_whitespace()
                .last()?
                .trim_start_matches('?');
            let var: String = decl[dollar + 1..]
                .chars()
                .take_while(|c| is_ident(*c))
                .collect();
            Some((ty.rsplit('\\').next()?.to_string(), var))
        })
        .collect()
}

/// A migration adding the column.
pub fn migration(table: &str, field: &str, kind: Kind) -> String {
    format!(
        "<?php\n\n\
         use Illuminate\\Database\\Migrations\\Migration;\n\
         use Illuminate\\Database\\Schema\\Blueprint;\n\
         use Illuminate\\Support\\Facades\\Schema;\n\n\
         return new class extends Migration\n{{\n\
         \x20   public function up(): void\n    {{\n\
         \x20       Schema::table('{table}', function (Blueprint $table) {{\n\
         \x20           {}\n\
         \x20       }});\n    }}\n\n\
         \x20   public function down(): void\n    {{\n\
         \x20       Schema::table('{table}', function (Blueprint $table) {{\n\
         \x20           {}\n\
         \x20       }});\n    }}\n}};\n",
        kind.column(field),
        kind.drop(field)
    )
}

// ---- The plan ---------------------------------------------------------------

/// What carrying a field through the stack takes.
#[derive(Debug, Default)]
pub struct Heal {
    pub field: String,
    /// Files to change: `(path, text now, text after)`.
    pub changes: Vec<(PathBuf, String, String)>,
    /// Files to create: `(path, content)`.
    pub creates: Vec<(PathBuf, String)>,
    /// What it found already done, or couldn't do.
    pub notes: Vec<String>,
}

impl Heal {
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty() && self.creates.is_empty()
    }
}

/// What the plan reads.
pub struct Project<'a> {
    pub root: &'a Path,
    pub routes: &'a [RouteInfo],
    /// The live schema, by table (empty without a database).
    pub schema: &'a HashMap<String, Vec<ColumnInfo>>,
    /// A file's text: the open buffer's when it is open.
    pub read: &'a dyn Fn(&Path) -> Option<String>,
    /// `YYYY_MM_DD_HHMMSS` for a new migration.
    pub timestamp: &'a str,
}

struct Planner<'a> {
    p: &'a Project<'a>,
    heal: Heal,
}

impl Planner<'_> {
    fn rel(&self, path: &Path) -> String {
        path.strip_prefix(self.p.root)
            .unwrap_or(path)
            .display()
            .to_string()
    }

    /// The text of `path` as the plan has left it so far.
    fn text(&self, path: &Path) -> Option<String> {
        self.heal
            .changes
            .iter()
            .find(|(p, _, _)| p == path)
            .map(|(_, _, new)| new.clone())
            .or_else(|| (self.p.read)(path))
    }

    fn change(&mut self, path: &Path, new: String) {
        match self.heal.changes.iter_mut().find(|(p, _, _)| p == path) {
            Some(c) => c.2 = new,
            None => {
                let old = (self.p.read)(path).unwrap_or_default();
                self.heal.changes.push((path.to_path_buf(), old, new));
            }
        }
    }

    fn note(&mut self, n: String) {
        self.heal.notes.push(n);
    }

    /// An app class's file by its short name, under `dir` (or anywhere in
    /// `app/`).
    fn class_file(&self, dir: &str, name: &str) -> Option<PathBuf> {
        let mut stack = vec![self.p.root.join(dir), self.p.root.join("app")];
        while let Some(d) = stack.pop() {
            let Ok(read) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in read.flatten() {
                let path = e.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.file_stem().is_some_and(|s| s == name)
                    && path.extension().is_some_and(|x| x == "php")
                {
                    return Some(path);
                }
            }
        }
        None
    }

    fn model_file(&self, model: &str) -> Option<PathBuf> {
        [
            self.p.root.join("app/Models").join(format!("{model}.php")),
            self.p.root.join("app").join(format!("{model}.php")),
        ]
        .into_iter()
        .find(|p| p.is_file())
    }

    fn is_model(&self, name: &str) -> bool {
        name.chars().next().is_some_and(char::is_uppercase) && self.model_file(name).is_some()
    }

    /// The rule, in the class at `path` (in `method`).
    fn validation(&mut self, path: &Path, method: Option<&str>, field: &str, kind: Kind) {
        let Some(src) = self.text(path) else {
            return;
        };
        let rel = self.rel(path);
        match add_rule(&src, method, field, &kind.rule(field)) {
            Ok(Some(new)) => self.change(path, new),
            Ok(None) => self.note(format!("{rel} validates {field} already")),
            Err(()) => self.note(format!("{rel} has no rules to add {field} to")),
        }
    }

    /// `$fillable`, the cast, and the column, for the model the form writes.
    fn model(&mut self, model: Option<String>, field: &str, kind: Kind) {
        let Some(model) = model else {
            self.note(format!(
                "Couldn't tell which model this form writes, so {field} isn't made fillable or given a column"
            ));
            return;
        };
        let Some(path) = self.model_file(&model) else {
            return;
        };
        let Some(src) = self.text(&path) else {
            return;
        };
        match add_fillable(&src, field) {
            Fillable::Added(new) => self.change(&path, new),
            Fillable::Already => {}
            Fillable::Undeclared => {
                let unguarded = std::fs::read_to_string(
                    self.p.root.join("app/Providers/AppServiceProvider.php"),
                )
                .is_ok_and(|s| s.contains("unguard"));
                if !unguarded {
                    self.note(format!(
                        "{model} declares no $fillable: add {field} to it, or it won't be saved"
                    ));
                }
            }
        }
        if let Some(cast) = kind.cast() {
            if let Some(src) = self.text(&path) {
                if let Some(new) = add_cast(&src, field, cast) {
                    self.change(&path, new);
                }
            }
        }
        let src = self.text(&path).unwrap_or_default();
        let table = src
            .find("$table")
            .and_then(|i| {
                let rest = &src[i..];
                let q = rest.find(['\'', '"'])?;
                let quote = rest.as_bytes()[q] as char;
                let name = &rest[q + 1..];
                name.find(quote).map(|e| name[..e].to_string())
            })
            .filter(|t| !t.contains(' '))
            .unwrap_or_else(|| pluralize(&snake_case(&model)));
        match self.has_column(&table, field) {
            Column::Live | Column::Migrated => {}
            Column::Pending(file) => self.note(format!(
                "{file} adds the {field} column, but hasn't run here yet: php artisan migrate"
            )),
            Column::Missing => {
                let path = self.p.root.join("database/migrations").join(format!(
                    "{}_add_{field}_to_{table}_table.php",
                    self.p.timestamp
                ));
                self.heal.creates.push((path, migration(&table, field, kind)));
            }
            Column::Unknown => self.note(format!(
                "Couldn't tell whether {table} has a {field} column: connect the database, or add the migration yourself"
            )),
        }
    }

    /// Whether `table` has the column. The live schema says when it knows the
    /// table — but a migration that adds the column and hasn't run yet (one
    /// this made a moment ago) counts too, or every run would add another.
    fn has_column(&self, table: &str, field: &str) -> Column {
        let live = self.p.schema.get(table);
        if live.is_some_and(|cols| cols.iter().any(|c| c.name == field)) {
            return Column::Live;
        }
        let dir = self.p.root.join("database/migrations");
        let mut mentioned = false;
        let mut files: Vec<_> = std::fs::read_dir(dir)
            .map(|r| r.flatten().map(|e| e.path()).collect())
            .unwrap_or_default();
        files.sort();
        for path in files {
            let Ok(src) = std::fs::read_to_string(&path) else {
                continue;
            };
            if src.contains(&format!("'{table}'")) || src.contains(&format!("\"{table}\"")) {
                mentioned = true;
                if src.contains(&format!("('{field}'")) || src.contains(&format!("(\"{field}\"")) {
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    return if live.is_some() {
                        Column::Pending(name)
                    } else {
                        Column::Migrated
                    };
                }
            }
        }
        match (live, mentioned) {
            (Some(_), _) | (None, true) => Column::Missing,
            (None, false) => Column::Unknown,
        }
    }
}

/// What the plan knows of a column.
enum Column {
    /// The live schema has it.
    Live,
    /// No database to ask, but a migration makes it.
    Migrated,
    /// The database lacks it, but this migration adds it and hasn't run.
    Pending(String),
    Missing,
    Unknown,
}

impl Planner<'_> {
    /// The server side of a submitted form: the route's action, its
    /// validation and the model it writes.
    fn submitted(&mut self, route: &RouteInfo, field: &str, kind: Kind) {
        let Some((class, method)) = route.action.split_once('@') else {
            self.note(format!(
                "{} isn't handled by a controller method",
                route.uri
            ));
            return;
        };
        // Fortify's controllers hand the input to the app's own actions.
        let fortify = match class.trim_start_matches('\\') {
            "Laravel\\Fortify\\Http\\Controllers\\ProfileInformationController" => {
                Some("UpdateUserProfileInformation")
            }
            "Laravel\\Fortify\\Http\\Controllers\\RegisteredUserController" => {
                Some("CreateNewUser")
            }
            _ => None,
        };
        if let Some(action) = fortify {
            let path = self
                .p
                .root
                .join(format!("app/Actions/Fortify/{action}.php"));
            if path.is_file() {
                self.validation(&path, None, field, kind);
                if let Some(new) = self.text(&path).and_then(|s| add_input(&s, field)) {
                    self.change(&path, new);
                }
                self.model(Some("User".into()), field, kind);
                return;
            }
        }
        let Some(rel) = class.trim_start_matches('\\').strip_prefix("App\\") else {
            self.note(format!(
                "{} is handled by {class}, which isn't the app's",
                route.uri
            ));
            return;
        };
        let ctrl = self
            .p
            .root
            .join("app")
            .join(format!("{}.php", rel.replace('\\', "/")));
        let Some(src) = self.text(&ctrl) else {
            return;
        };
        let Some(m) = methods(&src).into_iter().find(|m| m.name == method) else {
            return;
        };
        let typed = typed_params(m.params);
        let request = typed
            .iter()
            .find(|(ty, _)| ty.ends_with("Request") && ty != "Request")
            .and_then(|(ty, _)| self.class_file("app/Http/Requests", ty));
        match request {
            Some(req) => self.validation(&req, None, field, kind),
            None => self.validation(&ctrl, Some(method), field, kind),
        }
        let model = written_model(m.body, &typed, &|n| self.is_model(n));
        self.model(model, field, kind);
    }
}

/// The route a form submits to: by name (`route('orders.store')`), or by
/// the path it posts to.
fn find_route<'r>(routes: &'r [RouteInfo], method: &str, target: &str) -> Option<&'r RouteInfo> {
    if let Some(at) = target.find("route(") {
        let rest = target[at + 6..].trim_start();
        let q = rest.chars().next().filter(|c| *c == '\'' || *c == '"')?;
        let name = &rest[1..];
        let name = &name[..name.find(q)?];
        return routes.iter().find(|r| r.name == name);
    }
    let path = target.trim().trim_matches(['\'', '"', '`']);
    let path = path
        .strip_prefix("{{ url(")
        .and_then(|p| p.strip_suffix(") }}"))
        .map(|p| p.trim_matches(['\'', '"']))
        .unwrap_or(path);
    path.starts_with('/')
        .then(|| route_for(routes, method, path))
        .flatten()
}

/// Everything carrying `f` through the stack takes, starting in `file`
/// (whose current text is `text`). Blocking: reads the project's files.
pub fn plan(p: &Project, file: &Path, text: &str, f: &FieldRef) -> Result<Heal, String> {
    let mut pl = Planner {
        p,
        heal: Heal {
            field: f.field.clone(),
            ..Heal::default()
        },
    };
    let field = f.field.as_str();
    match &f.origin {
        Origin::Inertia { form } => {
            let open = form_object(text, form).ok_or("couldn't find the form's useForm({…})")?;
            let close =
                matching(text, open, (b'{', b'}')).ok_or("the form's useForm({…}) isn't closed")?;
            let inner = &text[open + 1..close];
            // Its starting value says what it is, when the form has it.
            let init = inner.find(&format!("{field}:")).map(|i| {
                let v = &inner[i + field.len() + 1..];
                v[..v.find([',', '\n']).unwrap_or(v.len())]
                    .trim()
                    .to_string()
            });
            let kind = kind_of(field, init.as_deref());
            let mut src = text.to_string();
            if let Some(new) = insert_entry(
                &src,
                open,
                field,
                &format!("{field}: {}", kind.js_init()),
                ',',
            ) {
                src = new;
            }
            // `useForm<OrderForm>(…)`: its interface, in this file.
            let decl = src
                .find(&format!("{form} = useForm<"))
                .or_else(|| src.find(&format!("{form}= useForm<")));
            if let Some(at) = decl {
                let ty: String = src[at..]
                    .split_once('<')
                    .map(|(_, r)| r.chars().take_while(|c| is_ident(*c)).collect())
                    .unwrap_or_default();
                let def = src
                    .find(&format!("interface {ty} "))
                    .or_else(|| src.find(&format!("type {ty} =")));
                if let Some(brace) = def.and_then(|d| src[d..].find('{').map(|i| d + i)) {
                    let close = matching(&src, brace, (b'{', b'}')).unwrap_or(brace);
                    let sep = if src[brace..close].contains(',') {
                        ','
                    } else {
                        ';'
                    };
                    if let Some(new) =
                        insert_entry(&src, brace, field, &format!("{field}: {}", kind.ts()), sep)
                    {
                        src = new;
                    }
                }
            }
            if src != text {
                pl.change(file, src.clone());
            }
            // Where it is sent: `form.post(route('orders.store'))`.
            let send = [".post(", ".put(", ".patch(", ".delete(", ".submit("]
                .iter()
                .filter_map(|v| src.find(&format!("{form}{v}")).map(|i| (i, *v)))
                .min_by_key(|(i, _)| *i)
                .ok_or(format!("{form} is never submitted in this file"))?;
            let args = &src[send.0 + form.len() + send.1.len()..];
            let (method, target) = match send.1 {
                ".submit(" => {
                    let (m, rest) = args.split_once(',').unwrap_or(("post", args));
                    (
                        m.trim().trim_matches(['\'', '"']).to_ascii_uppercase(),
                        rest,
                    )
                }
                v => (v.trim_matches(['.', '(']).to_ascii_uppercase(), args),
            };
            let target = &target[..target.find(['\n', ';']).unwrap_or(target.len())];
            let route = find_route(p.routes, &method, target).ok_or_else(|| {
                format!(
                    "couldn't tell which route {form}.{} sends to",
                    method.to_lowercase()
                )
            })?;
            let route = route.clone();
            pl.submitted(&route, field, kind);
        }
        Origin::Blade => {
            let at = text
                .find(&format!("name=\"{field}"))
                .or_else(|| text.find(&format!("name='{field}")));
            let at = at.ok_or("couldn't find the input")?;
            let open = text[..at]
                .rfind("<form")
                .ok_or(format!("{field} isn't inside a <form>"))?;
            if text[open..at].contains("</form") {
                return Err(format!("{field} isn't inside a <form>"));
            }
            let tag_end = text[open..].find('>').map_or(text.len(), |i| open + i);
            let tag = &text[open..tag_end];
            let attr = |name: &str| {
                let i = tag.find(&format!("{name}=\""))? + name.len() + 2;
                tag[i..].find('"').map(|e| tag[i..i + e].to_string())
            };
            let form_end = text[open..].find("</form").map_or(text.len(), |i| open + i);
            let body = &text[open..form_end];
            let spoofed = ["@method('", "@method(\"", "name=\"_method\" value=\""]
                .iter()
                .find_map(|m| {
                    let i = body.find(m)? + m.len();
                    body[i..]
                        .find(['\'', '"'])
                        .map(|e| body[i..i + e].to_string())
                });
            let method = spoofed
                .or_else(|| attr("method"))
                .unwrap_or_else(|| "GET".into())
                .to_ascii_uppercase();
            let action = attr("action").ok_or("the form has no action")?;
            let route = find_route(p.routes, &method, &action)
                .ok_or_else(|| format!("couldn't tell which route the form sends to ({action})"))?;
            let route = route.clone();
            let kind = kind_of(field, None);
            pl.submitted(&route, field, kind);
        }
        Origin::Livewire { path } => {
            let comp = crate::livewire::resolve(p.root, file)
                .ok_or("couldn't find the Livewire component")?;
            let class_src = pl
                .text(&comp.class_file)
                .ok_or("couldn't read the component")?;
            let kind = kind_of(field, None);
            // `form.phone`: a Livewire form object holds it.
            let (target, target_src) = match path.split_once('.') {
                Some((prop, _)) => {
                    let ty = typed_properties(crate::livewire::class_part(&class_src))
                        .into_iter()
                        .find(|(_, v)| v == prop)
                        .map(|(t, _)| t)
                        .ok_or(format!("couldn't find the component's {prop} property"))?;
                    let file = pl
                        .class_file("app/Livewire/Forms", &ty)
                        .ok_or(format!("couldn't find {ty}"))?;
                    let src = pl.text(&file).ok_or(format!("couldn't read {ty}"))?;
                    (file, src)
                }
                None => (comp.class_file.clone(), class_src.clone()),
            };
            let class = crate::livewire::class_part(&target_src).to_string();
            let rule = kind.rule(field);
            let mut new_class = class.clone();
            if let Some(n) = add_property(&new_class, field, kind, &rule) {
                new_class = n;
            }
            let attribute =
                new_class.contains(&format!("${field} =")) && new_class.contains("#[Validate(");
            if !attribute {
                match add_rule(&new_class, None, field, &rule) {
                    Ok(Some(n)) => new_class = n,
                    Ok(None) => {}
                    Err(()) => pl.note(format!(
                        "{} has no rules to add {field} to",
                        pl.rel(&target)
                    )),
                }
            }
            if new_class != class {
                pl.change(&target, target_src.replacen(&class, &new_class, 1));
            }
            // The model: written by the form object or the component.
            let model = [&target_src, &class_src].iter().find_map(|src| {
                let part = crate::livewire::class_part(src);
                written_model(part, &typed_properties(part), &|n| pl.is_model(n))
            });
            pl.model(model, field, kind);
        }
    }
    Ok(pl.heal)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_field_under_the_cursor() {
        let vue = "<script setup>\nconst form = useForm({\n  name: '',\n})\n</script>\n<input v-model=\"form.phone\">\n<p>{{ form.errors.email }}</p>\n";
        let at = |s: &str, n: &str| s.find(n).unwrap() + 2;
        assert_eq!(
            field_at(vue, at(vue, "phone"), false),
            Some(FieldRef {
                field: "phone".into(),
                origin: Origin::Inertia {
                    form: "form".into()
                }
            })
        );
        assert_eq!(
            field_at(vue, at(vue, "email"), false).unwrap().field,
            "email"
        );
        assert_eq!(
            field_at(vue, at(vue, "name:"), false).unwrap().field,
            "name"
        );
        assert_eq!(
            field_at(vue, vue.find("form.phone").unwrap() + 1, false),
            None
        );

        let blade = "<form method=\"POST\" action=\"{{ route('orders.store') }}\">\n<input name=\"phone\" wire:model.live=\"form.email\">\n";
        assert_eq!(
            field_at(blade, at(blade, "phone"), true),
            Some(FieldRef {
                field: "phone".into(),
                origin: Origin::Blade
            })
        );
        assert_eq!(
            field_at(blade, at(blade, "email"), true),
            Some(FieldRef {
                field: "email".into(),
                origin: Origin::Livewire {
                    path: "form.email".into()
                }
            })
        );
    }

    #[test]
    fn a_field_s_kind_comes_from_its_name_and_starting_value() {
        assert_eq!(kind_of("phone", None), Kind::String);
        assert_eq!(kind_of("customer_id", None), Kind::ForeignId);
        assert_eq!(kind_of("is_vip", None), Kind::Boolean);
        assert_eq!(kind_of("newsletter", Some("false")), Kind::Boolean);
        assert_eq!(kind_of("shipped_at", None), Kind::DateTime);
        assert_eq!(kind_of("notes", None), Kind::Text);
        assert_eq!(kind_of("unit_price", None), Kind::Decimal);
        assert_eq!(kind_of("seats", Some("0")), Kind::Integer);
        assert_eq!(
            Kind::ForeignId.rule("customer_id"),
            ["nullable", "exists:customers,id"]
        );
    }

    #[test]
    fn entries_join_a_list_as_its_others_are_laid_out() {
        let rules = "return [\n            'name' => ['required', 'string'],\n            // more later\n        ];";
        assert_eq!(
            insert_entry(rules, 7, "phone", "'phone' => ['nullable']", ',').unwrap(),
            "return [\n            'name' => ['required', 'string'],\n            'phone' => ['nullable'],\n            // more later\n        ];"
        );
        assert_eq!(insert_entry(rules, 7, "name", "'name' => []", ','), None);
        assert_eq!(
            insert_entry(
                "$fillable = ['name', 'email'];",
                12,
                "phone",
                "'phone'",
                ','
            )
            .unwrap(),
            "$fillable = ['name', 'email', 'phone'];"
        );
        assert_eq!(
            insert_entry("    return [];", 11, "phone", "'phone' => 'x'", ',').unwrap(),
            "    return [\n        'phone' => 'x',\n    ];"
        );
        assert_eq!(
            insert_entry(
                "interface F {\n  name: string\n}",
                12,
                "phone",
                "phone: string",
                ';'
            )
            .unwrap(),
            "interface F {\n  name: string;\n  phone: string;\n}"
        );
    }

    #[test]
    fn rules_follow_the_style_beside_them() {
        let piped = "public function rules(): array\n{\n    return [\n        'name' => 'required|string',\n    ];\n}";
        let out = add_rule(piped, None, "phone", &Kind::String.rule("phone"))
            .unwrap()
            .unwrap();
        assert!(
            out.contains("'phone' => 'nullable|string|max:255',"),
            "{out}"
        );
        let inline = "public function store(Request $r)\n{\n    $data = $r->validate([\n        'name' => ['required'],\n    ]);\n}";
        let out = add_rule(inline, Some("store"), "phone", &Kind::String.rule("phone"))
            .unwrap()
            .unwrap();
        assert!(
            out.contains("'phone' => ['nullable', 'string', 'max:255'],\n    ]);"),
            "{out}"
        );
        assert!(add_rule("class A {}", None, "phone", &[]).is_err());
        let built = "public function rules(): array\n{\n    $rules = [\n        'name' => 'required',\n    ];\n    if ($admin) {\n        $rules['x'] = 'y';\n    }\n    return $rules;\n}";
        let out = add_rule(built, None, "phone", &Kind::String.rule("phone"))
            .unwrap()
            .unwrap();
        assert!(
            out.contains(
                "'name' => 'required',\n        'phone' => 'nullable|string|max:255',\n    ];"
            ),
            "{out}"
        );
    }

    #[test]
    fn a_fortify_action_validates_and_fills_the_field() {
        let src = "Validator::make($input, [\n    'name' => ['required'],\n])->validate();\n$user->forceFill([\n    'name' => $input['name'],\n])->save();\n";
        let out = add_rule(src, None, "website", &Kind::String.rule("website"))
            .unwrap()
            .unwrap();
        let out = add_input(&out, "website").unwrap();
        assert!(out.contains("    'name' => ['required'],\n    'website' => ['nullable', 'string', 'max:255'],\n])"), "{out}");
        assert!(out.contains("    'name' => $input['name'],\n    'website' => $input['website'] ?? null,\n])->save();"), "{out}");
    }

    #[test]
    fn models_get_the_field_fillable_and_cast() {
        let m = "class Order extends Model\n{\n    protected $fillable = [\n        'name',\n    ];\n}\n";
        let Fillable::Added(out) = add_fillable(m, "phone") else {
            panic!()
        };
        assert!(
            out.contains("        'name',\n        'phone',\n    ];"),
            "{out}"
        );
        assert_eq!(
            add_fillable("protected $guarded = [];", "phone"),
            Fillable::Already
        );
        assert_eq!(add_fillable("class A {}", "phone"), Fillable::Undeclared);
        let out = add_cast(m, "is_vip", "boolean").unwrap();
        assert!(out.ends_with("    ];\n\n    protected function casts(): array\n    {\n        return [\n            'is_vip' => 'boolean',\n        ];\n    }\n}\n"), "{out}");
        let with = "    protected function casts(): array\n    {\n        return [\n            'paid' => 'boolean',\n        ];\n    }\n";
        assert!(add_cast(with, "is_vip", "boolean")
            .unwrap()
            .contains("'paid' => 'boolean',\n            'is_vip' => 'boolean',"));
    }

    #[test]
    fn the_model_a_piece_of_code_writes() {
        let is_model = |n: &str| ["Order", "User", "Note"].contains(&n);
        let typed = vec![("Order".to_string(), "order".to_string())];
        assert_eq!(
            written_model("$order->update($request->validated());", &typed, &is_model).as_deref(),
            Some("Order")
        );
        assert_eq!(
            written_model("Validator::make(); Order::create($data);", &[], &is_model).as_deref(),
            Some("Order")
        );
        assert_eq!(
            written_model("$request->user()->update($v);", &[], &is_model).as_deref(),
            Some("User")
        );
        assert_eq!(
            written_model("$order->notes()->create($v);", &typed, &is_model).as_deref(),
            Some("Note")
        );
        assert_eq!(written_model("return back();", &typed, &is_model), None);
    }

    #[test]
    fn a_livewire_property_goes_beside_the_others() {
        let src = "class Edit extends Component\n{\n    #[Validate('required')]\n    public string $name = '';\n\n    public function save() {}\n}";
        let out = add_property(src, "phone", Kind::String, &Kind::String.rule("phone")).unwrap();
        assert!(
            out.contains("    public string $name = '';\n\n    #[Validate('nullable|string|max:255')]\n    public string $phone = '';\n\n    public function save"),
            "{out}"
        );
        let plain = "class Edit extends Component\n{\n    public string $name = '';\n}";
        let out = add_property(plain, "seats", Kind::Integer, &[]).unwrap();
        assert!(
            out.contains("    public string $name = '';\n    public ?int $seats = null;\n}"),
            "{out}"
        );
    }

    /// A small Laravel app on disk.
    fn app(files: &[(&str, &str)]) -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "e-heal-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        for (path, src) in files {
            let p = root.join(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, src).unwrap();
        }
        root
    }

    const MODEL: &str = "<?php\n\nnamespace App\\Models;\n\nclass Customer extends Model\n{\n    protected $fillable = [\n        'name',\n    ];\n}\n";
    const REQUEST: &str = "<?php\n\nclass StoreCustomerRequest extends FormRequest\n{\n    public function rules(): array\n    {\n        return [\n            'name' => ['required', 'string'],\n        ];\n    }\n}\n";
    const CONTROLLER: &str = "<?php\n\nnamespace App\\Http\\Controllers;\n\nclass CustomerController extends Controller\n{\n    public function store(StoreCustomerRequest $request)\n    {\n        Customer::create($request->validated());\n        return back();\n    }\n}\n";
    const MIGRATION: &str = "<?php\nSchema::create('customers', function (Blueprint $table) {\n    $table->id();\n    $table->string('name');\n});\n";

    fn routes() -> Vec<RouteInfo> {
        vec![RouteInfo {
            name: "customers.store".into(),
            uri: "customers".into(),
            methods: "POST".into(),
            action: "App\\Http\\Controllers\\CustomerController@store".into(),
            middleware: "web".into(),
        }]
    }

    fn run(root: &Path, file: &str, text: &str, f: &FieldRef) -> Heal {
        run_with(root, file, text, f, HashMap::new())
    }

    fn run_with(
        root: &Path,
        file: &str,
        text: &str,
        f: &FieldRef,
        schema: HashMap<String, Vec<ColumnInfo>>,
    ) -> Heal {
        let routes = routes();
        let read = |p: &Path| std::fs::read_to_string(p).ok();
        let p = Project {
            root,
            routes: &routes,
            schema: &schema,
            read: &read,
            timestamp: "2026_10_26_120000",
        };
        plan(&p, &root.join(file), text, f).unwrap()
    }

    #[test]
    fn an_inertia_field_is_carried_to_the_request_the_model_and_a_migration() {
        let page = "<script setup lang=\"ts\">\ninterface CustomerForm { name: string }\nconst form = useForm<CustomerForm>({\n  name: '',\n})\nconst save = () => form.post(route('customers.store'))\n</script>\n<template><input v-model=\"form.phone\"></template>\n";
        let root = app(&[
            ("app/Models/Customer.php", MODEL),
            ("app/Http/Requests/StoreCustomerRequest.php", REQUEST),
            ("app/Http/Controllers/CustomerController.php", CONTROLLER),
            (
                "database/migrations/2024_01_01_000000_create_customers_table.php",
                MIGRATION,
            ),
            ("resources/js/Pages/Customers/Create.vue", page),
        ]);
        let f = field_at(page, page.find("form.phone").unwrap() + 6, false).unwrap();
        let heal = run(&root, "resources/js/Pages/Customers/Create.vue", page, &f);
        assert!(heal.notes.is_empty(), "{:?}", heal.notes);
        let after = |name: &str| {
            heal.changes
                .iter()
                .find(|(p, _, _)| p.ends_with(name))
                .map(|(_, _, n)| n.clone())
                .unwrap_or_else(|| panic!("no change to {name}"))
        };
        let page = after("Create.vue");
        assert!(
            page.contains("interface CustomerForm { name: string; phone: string }"),
            "{page}"
        );
        assert!(page.contains("  name: '',\n  phone: '',\n})"), "{page}");
        assert!(after("StoreCustomerRequest.php")
            .contains("'phone' => ['nullable', 'string', 'max:255'],"));
        assert!(after("Customer.php").contains("'name',\n        'phone',"));
        let (path, mig) = &heal.creates[0];
        assert!(path
            .ends_with("database/migrations/2026_10_26_120000_add_phone_to_customers_table.php"));
        assert!(mig.contains("$table->string('phone')->nullable();"));
        assert!(mig.contains("$table->dropColumn('phone');"));

        // Carried through, there is nothing left to do.
        for (p, _, new) in &heal.changes {
            std::fs::write(p, new).unwrap();
        }
        std::fs::write(&heal.creates[0].0, &heal.creates[0].1).unwrap();
        let page_now =
            std::fs::read_to_string(root.join("resources/js/Pages/Customers/Create.vue")).unwrap();
        let again = run(
            &root,
            "resources/js/Pages/Customers/Create.vue",
            &page_now,
            &f,
        );
        assert!(again.is_empty(), "{:?}", again.changes);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_migration_that_has_not_run_yet_is_not_made_twice() {
        let view = "<form method=\"POST\" action=\"{{ route('customers.store') }}\">\n  <input name=\"phone\">\n</form>\n";
        let pending = "<?php\nSchema::table('customers', function (Blueprint $table) {\n    $table->string('phone')->nullable();\n});\n";
        let root = app(&[
            ("app/Models/Customer.php", MODEL),
            ("app/Http/Requests/StoreCustomerRequest.php", REQUEST),
            ("app/Http/Controllers/CustomerController.php", CONTROLLER),
            (
                "database/migrations/2024_01_01_000000_create_customers_table.php",
                MIGRATION,
            ),
            (
                "database/migrations/2026_10_27_000000_add_phone_to_customers_table.php",
                pending,
            ),
            ("resources/views/customers/create.blade.php", view),
        ]);
        // The database has the table, without the column: the migration
        // hasn't run.
        let col = |name: &str| ColumnInfo {
            name: name.into(),
            data_type: "varchar(255)".into(),
            nullable: false,
            key: String::new(),
        };
        let schema: HashMap<String, Vec<ColumnInfo>> =
            [("customers".to_string(), vec![col("id"), col("name")])]
                .into_iter()
                .collect();
        let f = field_at(view, view.find("phone").unwrap(), true).unwrap();
        let heal = run_with(
            &root,
            "resources/views/customers/create.blade.php",
            view,
            &f,
            schema.clone(),
        );
        assert!(heal.creates.is_empty(), "{:?}", heal.creates);
        assert!(
            heal.notes
                .iter()
                .any(|n| n.contains("add_phone_to_customers_table.php adds the phone column")),
            "{:?}",
            heal.notes
        );

        // Without that migration, the live schema's word stands: one is made.
        std::fs::remove_file(
            root.join("database/migrations/2026_10_27_000000_add_phone_to_customers_table.php"),
        )
        .unwrap();
        let heal = run_with(
            &root,
            "resources/views/customers/create.blade.php",
            view,
            &f,
            schema,
        );
        assert_eq!(heal.creates.len(), 1);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_blade_form_and_a_livewire_form_object() {
        let view = "<form method=\"POST\" action=\"{{ route('customers.store') }}\">\n  @csrf\n  <input name=\"is_vip\" type=\"checkbox\">\n</form>\n";
        let root = app(&[
            ("app/Models/Customer.php", MODEL),
            ("app/Http/Requests/StoreCustomerRequest.php", REQUEST),
            ("app/Http/Controllers/CustomerController.php", CONTROLLER),
            ("database/migrations/2024_01_01_000000_create_customers_table.php", MIGRATION),
            ("resources/views/customers/create.blade.php", view),
            (
                "app/Livewire/Customers/Edit.php",
                "<?php\n\nnamespace App\\Livewire\\Customers;\n\nclass Edit extends Component\n{\n    public CustomerForm $form;\n\n    public function save()\n    {\n        $this->form->store();\n    }\n}\n",
            ),
            (
                "app/Livewire/Forms/CustomerForm.php",
                "<?php\n\nnamespace App\\Livewire\\Forms;\n\nclass CustomerForm extends Form\n{\n    #[Validate('required|string')]\n    public string $name = '';\n\n    public function store()\n    {\n        Customer::create($this->all());\n    }\n}\n",
            ),
            ("resources/views/livewire/customers/edit.blade.php", "<input wire:model=\"form.notes\">\n"),
        ]);
        let f = field_at(view, view.find("is_vip").unwrap(), true).unwrap();
        let heal = run(
            &root,
            "resources/views/customers/create.blade.php",
            view,
            &f,
        );
        assert!(heal.notes.is_empty(), "{:?}", heal.notes);
        let model = &heal
            .changes
            .iter()
            .find(|(p, _, _)| p.ends_with("Customer.php"))
            .unwrap()
            .2;
        assert!(model.contains("'is_vip',"), "{model}");
        assert!(model.contains("'is_vip' => 'boolean',"), "{model}");
        assert!(heal.creates[0]
            .1
            .contains("$table->boolean('is_vip')->default(false);"));

        let lw = "<input wire:model=\"form.notes\">\n";
        let f = field_at(lw, 20, true).unwrap();
        let heal = run(
            &root,
            "resources/views/livewire/customers/edit.blade.php",
            lw,
            &f,
        );
        let form = &heal
            .changes
            .iter()
            .find(|(p, _, _)| p.ends_with("CustomerForm.php"))
            .unwrap()
            .2;
        assert!(
            form.contains("    #[Validate('nullable|string')]\n    public string $notes = '';"),
            "{form}"
        );
        assert!(heal.creates[0]
            .1
            .contains("$table->text('notes')->nullable();"));
        std::fs::remove_dir_all(&root).ok();
    }
}
