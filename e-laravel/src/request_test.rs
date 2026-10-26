//! A feature test from a request the running app served.
//!
//! Grove captured the request — method, path, body (secrets redacted), status,
//! and with SQL capture on, the queries it ran. That is most of a feature test
//! already: replay the request against a fresh database and assert what it
//! did. This writes that test, in the project's style (Pest or PHPUnit):
//!
//! * the route's parameters become models made by their factories, owned by
//!   the signed-in user when the model belongs to one;
//! * `*_id` fields in the body that name a model get one made too;
//! * the status becomes `assertOk()`, `assertRedirect()`…; a form post that
//!   redirected also asserts it came back without validation errors; a request
//!   that failed becomes a test that expects it to succeed, red until fixed;
//! * the rows it wrote become `assertDatabaseHas`, `assertSoftDeleted`,
//!   `assertModelMissing` — for values the request itself sent, which the
//!   test sends again, never for ones the app made up (ids, timestamps);
//! * a Livewire update becomes `Livewire::test(…)->set(…)->call(…)`.
//!
//! Generating is pure ([`generate`], [`render`]); [`for_project`] reads what
//! it needs from the project.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::authz;
use crate::laravel::{route_for, RouteInfo};

/// A request as Grove (or Clockwork) captured it.
#[derive(Clone, Debug, Default)]
pub struct Captured {
    pub method: String,
    /// The path, with its query string.
    pub path: String,
    pub status: u16,
    /// Request headers, lower- or mixed-case names.
    pub headers: Vec<(String, String)>,
    pub body: String,
    /// The SQL it ran, values inlined.
    pub queries: Vec<String>,
}

impl Captured {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// What the test needs to know about one of the app's models.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Model {
    /// `App\Models\Order`.
    pub fqn: String,
    /// It has a factory to make one with.
    pub has_factory: bool,
    /// The column naming whose it is (`user_id`, `team_id`), if any.
    pub owner: Option<String>,
}

/// What the generator asks of the project.
pub struct Project<'a> {
    pub routes: &'a [RouteInfo],
    /// One of the app's models, by short name (`Order`).
    pub model: &'a dyn Fn(&str) -> Option<Model>,
    /// A class's source, by fully qualified name.
    pub source: &'a dyn Fn(&str) -> Option<String>,
}

/// A test, before it is put in a file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Generated {
    /// Relative to the project: `tests/Feature/OrderControllerTest.php`.
    pub file: PathBuf,
    /// `Tests\Feature\Admin`.
    pub namespace: String,
    /// `OrderControllerTest`.
    pub class: String,
    /// `PUT /orders/{order}`.
    pub name: String,
    /// Classes the test refers to.
    pub uses: BTreeSet<String>,
    /// The test's body, one line each, indented relative to the body.
    pub body: Vec<String>,
    /// What the test could not do by itself, for the person who runs it.
    pub notes: Vec<String>,
}

// ---- Values ---------------------------------------------------------------

/// A request value, as PHP will be given it.
#[derive(Clone, Debug, PartialEq)]
pub enum Val {
    Str(String),
    Num(String),
    Bool(bool),
    Null,
    List(Vec<Val>),
    /// In the order the request had them.
    Map(Vec<(String, Val)>),
    /// PHP, used as is: `$customer->id`.
    Php(String),
}

impl<'de> serde::Deserialize<'de> for Val {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Val, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = Val;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("JSON")
            }
            fn visit_bool<E: serde::de::Error>(self, b: bool) -> Result<Val, E> {
                Ok(Val::Bool(b))
            }
            fn visit_i64<E: serde::de::Error>(self, n: i64) -> Result<Val, E> {
                Ok(Val::Num(n.to_string()))
            }
            fn visit_u64<E: serde::de::Error>(self, n: u64) -> Result<Val, E> {
                Ok(Val::Num(n.to_string()))
            }
            fn visit_f64<E: serde::de::Error>(self, n: f64) -> Result<Val, E> {
                Ok(Val::Num(n.to_string()))
            }
            fn visit_str<E: serde::de::Error>(self, s: &str) -> Result<Val, E> {
                Ok(Val::Str(s.to_string()))
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Val, E> {
                Ok(Val::Null)
            }
            fn visit_none<E: serde::de::Error>(self) -> Result<Val, E> {
                Ok(Val::Null)
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut a: A) -> Result<Val, A::Error> {
                let mut out = Vec::new();
                while let Some(v) = a.next_element()? {
                    out.push(v);
                }
                Ok(Val::List(out))
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(self, mut a: A) -> Result<Val, A::Error> {
                let mut out = Vec::new();
                while let Some((k, v)) = a.next_entry::<String, Val>()? {
                    out.push((k, v));
                }
                Ok(Val::Map(out))
            }
        }
        d.deserialize_any(V)
    }
}

impl Val {
    fn get(&self, key: &str) -> Option<&Val> {
        match self {
            Val::Map(m) => m.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    fn as_str(&self) -> Option<&str> {
        match self {
            Val::Str(s) => Some(s),
            _ => None,
        }
    }

    /// The value as the database would store it, for a scalar.
    fn scalar(&self) -> Option<String> {
        match self {
            Val::Str(s) | Val::Num(s) => Some(s.clone()),
            Val::Bool(b) => Some(if *b { "1" } else { "0" }.into()),
            _ => None,
        }
    }

    /// PHP for the value. `indent` is the indentation of the line it starts
    /// on; an array that would not fit on it, or any non-empty one when
    /// `wrap`, goes one entry per line.
    pub fn php(&self, indent: usize, wrap: bool) -> String {
        let entries: Vec<(Option<String>, &Val)> = match self {
            Val::Str(s) => return php_str(s),
            Val::Num(n) => return n.clone(),
            Val::Bool(b) => return if *b { "true" } else { "false" }.into(),
            Val::Null => return "null".into(),
            Val::Php(code) => return code.clone(),
            Val::List(l) => l.iter().map(|v| (None, v)).collect(),
            Val::Map(m) => {
                let listlike = m.iter().enumerate().all(|(i, (k, _))| *k == i.to_string());
                m.iter()
                    .map(|(k, v)| {
                        let key = if listlike {
                            None
                        } else if k.parse::<i64>().is_ok() {
                            Some(k.clone())
                        } else {
                            Some(php_str(k))
                        };
                        (key, v)
                    })
                    .collect()
            }
        };
        if entries.is_empty() {
            return "[]".into();
        }
        let entry = |(k, v): &(Option<String>, &Val), indent: usize| match k {
            Some(k) => format!("{k} => {}", v.php(indent, false)),
            None => v.php(indent, false),
        };
        if !wrap {
            let inline = format!(
                "[{}]",
                entries
                    .iter()
                    .map(|e| entry(e, indent))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            if indent + inline.len() <= 100 && !inline.contains('\n') {
                return inline;
            }
        }
        let pad = " ".repeat(indent + 4);
        let mut out = String::from("[\n");
        for e in &entries {
            out.push_str(&format!("{pad}{},\n", entry(e, indent + 4)));
        }
        out.push_str(&" ".repeat(indent));
        out.push(']');
        out
    }
}

/// A PHP single-quoted string.
fn php_str(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                let hex = std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(v) => {
                        out.push(v);
                        i += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Put `v` at `path` (`items`, `0`, `name`) in a map; an empty segment (`[]`)
/// appends.
fn insert(map: &mut Vec<(String, Val)>, path: &[String], v: Val) {
    let Some((first, rest)) = path.split_first() else {
        return;
    };
    let key = if first.is_empty() {
        map.len().to_string()
    } else {
        first.clone()
    };
    if rest.is_empty() {
        match map.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = v,
            None => map.push((key, v)),
        }
        return;
    }
    if !map.iter().any(|(k, _)| *k == key) {
        map.push((key.clone(), Val::Map(Vec::new())));
    }
    if let Some((_, Val::Map(inner))) = map.iter_mut().find(|(k, _)| *k == key) {
        insert(inner, rest, v);
    }
}

/// `a[b][]` → `a`, `b`, ``.
fn key_path(key: &str) -> Vec<String> {
    let Some(open) = key.find('[') else {
        return vec![key.to_string()];
    };
    let mut out = vec![key[..open].to_string()];
    for part in key[open..].split('[').skip(1) {
        out.push(part.trim_end_matches(']').to_string());
    }
    out
}

/// A form-encoded body (`a=1&items[0][name]=x`) as a map.
pub fn parse_form(body: &str) -> Val {
    let mut map = Vec::new();
    for pair in body.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        insert(
            &mut map,
            &key_path(&percent_decode(k)),
            Val::Str(percent_decode(v)),
        );
    }
    Val::Map(map)
}

/// A multipart body's fields; a file becomes a fake upload of its name.
fn parse_multipart(body: &str, content_type: &str) -> Val {
    let Some(boundary) = content_type
        .split(';')
        .find_map(|p| p.trim().strip_prefix("boundary="))
        .map(|b| b.trim_matches('"'))
    else {
        return Val::Map(Vec::new());
    };
    let mut map = Vec::new();
    for part in body.split(&format!("--{boundary}")) {
        let Some((head, value)) = part
            .split_once("\r\n\r\n")
            .or_else(|| part.split_once("\n\n"))
        else {
            continue;
        };
        let attr = |name: &str| {
            let at = head.find(&format!("{name}=\""))? + name.len() + 2;
            head[at..].split('"').next().map(str::to_string)
        };
        let Some(name) = attr("name") else {
            continue;
        };
        let v = match attr("filename") {
            Some(file) if !file.is_empty() => Val::Php(format!(
                "UploadedFile::fake()->create({}, 100)",
                php_str(&file)
            )),
            Some(_) => continue,
            None => Val::Str(value.trim_end_matches(['\r', '\n']).to_string()),
        };
        insert(&mut map, &key_path(&name), v);
    }
    Val::Map(map)
}

// ---- SQL --------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    /// A quoted identifier.
    Name(String),
    /// A bare word: a keyword or an identifier.
    Word(String),
    /// A literal value; `None` for NULL.
    Lit(Option<String>),
    P(char),
}

fn tokens(sql: &str) -> Vec<Tok> {
    let c: Vec<char> = sql.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        if ch.is_whitespace() {
            i += 1;
        } else if ch == '`' || ch == '"' {
            let mut s = String::new();
            i += 1;
            while i < c.len() && c[i] != ch {
                s.push(c[i]);
                i += 1;
            }
            i += 1;
            out.push(Tok::Name(s));
        } else if ch == '\'' {
            let mut s = String::new();
            i += 1;
            while i < c.len() {
                if c[i] == '\\' && i + 1 < c.len() {
                    s.push(c[i + 1]);
                    i += 2;
                } else if c[i] == '\'' && c.get(i + 1) == Some(&'\'') {
                    s.push('\'');
                    i += 2;
                } else if c[i] == '\'' {
                    break;
                } else {
                    s.push(c[i]);
                    i += 1;
                }
            }
            i += 1;
            out.push(Tok::Lit(Some(s)));
        } else if ch.is_ascii_digit()
            || (ch == '-'
                && c.get(i + 1).is_some_and(char::is_ascii_digit)
                && matches!(out.last(), Some(Tok::P('=' | ',' | '('))))
        {
            let start = i;
            i += 1;
            while i < c.len() && (c[i].is_ascii_digit() || c[i] == '.') {
                i += 1;
            }
            out.push(Tok::Lit(Some(c[start..i].iter().collect())));
        } else if ch.is_alphanumeric() || ch == '_' {
            let start = i;
            while i < c.len() && (c[i].is_alphanumeric() || c[i] == '_') {
                i += 1;
            }
            let w: String = c[start..i].iter().collect();
            out.push(if w.eq_ignore_ascii_case("null") {
                Tok::Lit(None)
            } else {
                Tok::Word(w)
            });
        } else {
            out.push(Tok::P(ch));
            i += 1;
        }
    }
    out
}

/// A row the request wrote.
#[derive(Clone, Debug, PartialEq)]
enum Write {
    Insert {
        table: String,
        cols: Vec<(String, Option<String>)>,
    },
    Update {
        table: String,
        sets: Vec<(String, Option<String>)>,
        id: Option<String>,
    },
    Delete {
        table: String,
        id: Option<String>,
    },
}

/// A (possibly qualified) name at `i`: `orders`, `` `db`.`orders` `` → the
/// last part, and the index after it.
fn name_at(t: &[Tok], mut i: usize) -> Option<(String, usize)> {
    let mut last = None;
    loop {
        match t.get(i) {
            Some(Tok::Name(n) | Tok::Word(n)) => last = Some(n.clone()),
            _ => return last.map(|n| (n, i)),
        }
        i += 1;
        if t.get(i) != Some(&Tok::P('.')) {
            return last.map(|n| (n, i));
        }
        i += 1;
    }
}

/// The comma-separated values from `i` to the `)` that closes the list, and
/// the index after it. A value that is not one literal is `None`.
fn values_at(t: &[Tok], mut i: usize) -> (Vec<Option<String>>, usize) {
    let mut out = Vec::new();
    let mut cur: Vec<&Tok> = Vec::new();
    let mut depth = 0;
    while let Some(tok) = t.get(i) {
        match tok {
            Tok::P('(') => depth += 1,
            Tok::P(')') if depth == 0 => break,
            Tok::P(')') => depth -= 1,
            Tok::P(',') if depth == 0 => {
                out.push(single_lit(&cur));
                cur.clear();
                i += 1;
                continue;
            }
            _ => {}
        }
        cur.push(tok);
        i += 1;
    }
    out.push(single_lit(&cur));
    (out, i + 1)
}

fn single_lit(toks: &[&Tok]) -> Option<String> {
    match toks {
        [Tok::Lit(v)] => v.clone(),
        _ => None,
    }
}

/// The `id` a WHERE clause from `i` pins: `id = 42`, `orders.id = 42`,
/// `id in (42)`.
fn where_id(t: &[Tok], from: usize) -> Option<String> {
    (from..t.len()).find_map(|j| {
        let is_id = matches!(&t[j], Tok::Name(n) | Tok::Word(n) if n == "id")
            && !matches!(t.get(j + 1), Some(Tok::P('.')));
        if !is_id {
            return None;
        }
        match (t.get(j + 1), t.get(j + 2), t.get(j + 3), t.get(j + 4)) {
            (Some(Tok::P('=')), Some(Tok::Lit(Some(v))), _, _) => Some(v.clone()),
            (Some(Tok::Word(w)), Some(Tok::P('(')), Some(Tok::Lit(Some(v))), Some(Tok::P(')')))
                if w.eq_ignore_ascii_case("in") =>
            {
                Some(v.clone())
            }
            _ => None,
        }
    })
}

fn parse_write(sql: &str) -> Option<Write> {
    let t = tokens(sql);
    let kw =
        |i: usize, w: &str| matches!(t.get(i), Some(Tok::Word(x)) if x.eq_ignore_ascii_case(w));
    if kw(0, "insert") {
        let mut i = 1;
        if kw(i, "ignore") {
            i += 1;
        }
        if !kw(i, "into") {
            return None;
        }
        let (table, mut i) = name_at(&t, i + 1)?;
        if t.get(i) != Some(&Tok::P('(')) {
            return None;
        }
        i += 1;
        let mut names = Vec::new();
        while let Some((n, next)) = name_at(&t, i) {
            names.push(n);
            i = next;
            match t.get(i) {
                Some(Tok::P(',')) => i += 1,
                _ => break,
            }
        }
        if t.get(i) != Some(&Tok::P(')'))
            || !kw(i + 1, "values")
            || t.get(i + 2) != Some(&Tok::P('('))
        {
            return None;
        }
        let (vals, _) = values_at(&t, i + 3);
        return Some(Write::Insert {
            table,
            cols: names.into_iter().zip(vals).collect(),
        });
    }
    if kw(0, "update") {
        let (table, i) = name_at(&t, 1)?;
        if !kw(i, "set") {
            return None;
        }
        let mut i = i + 1;
        let mut sets = Vec::new();
        while let Some((col, next)) = name_at(&t, i) {
            if t.get(next) != Some(&Tok::P('=')) {
                break;
            }
            let mut j = next + 1;
            let mut val: Vec<&Tok> = Vec::new();
            let mut depth = 0;
            while let Some(tok) = t.get(j) {
                match tok {
                    Tok::P('(') => depth += 1,
                    Tok::P(')') => depth -= 1,
                    Tok::P(',') if depth == 0 => break,
                    Tok::Word(w) if depth == 0 && w.eq_ignore_ascii_case("where") => break,
                    _ => {}
                }
                val.push(tok);
                j += 1;
            }
            sets.push((col, single_lit(&val)));
            i = j;
            if t.get(i) == Some(&Tok::P(',')) {
                i += 1;
            } else {
                break;
            }
        }
        return Some(Write::Update {
            table,
            sets,
            id: kw(i, "where").then(|| where_id(&t, i + 1)).flatten(),
        });
    }
    if kw(0, "delete") && kw(1, "from") {
        let (table, i) = name_at(&t, 2)?;
        return Some(Write::Delete {
            table,
            id: kw(i, "where").then(|| where_id(&t, i + 1)).flatten(),
        });
    }
    None
}

/// Tables the framework writes on its own account.
fn framework_table(table: &str) -> bool {
    matches!(
        table,
        "sessions"
            | "cache"
            | "cache_locks"
            | "jobs"
            | "job_batches"
            | "failed_jobs"
            | "password_reset_tokens"
            | "password_resets"
            | "personal_access_tokens"
            | "migrations"
            | "notifications"
            | "activity_log"
    ) || table.starts_with("telescope_")
        || table.starts_with("pulse_")
}

/// Columns whose value the app makes up, or that can't be compared.
fn made_up_column(col: &str) -> bool {
    matches!(
        col,
        "id" | "uuid"
            | "ulid"
            | "created_at"
            | "updated_at"
            | "deleted_at"
            | "password"
            | "remember_token"
            | "email_verified_at"
    )
}

// ---- Building a test --------------------------------------------------------

/// The pieces of a test as they are found.
#[derive(Default)]
struct Builder {
    setup: Vec<String>,
    uses: BTreeSet<String>,
    notes: Vec<String>,
    /// Models made, by the id the request knew them by: `(model, literal id,
    /// variable)`.
    made: Vec<(String, String, String)>,
    /// Whether `$user` exists.
    user: bool,
}

impl Builder {
    fn use_class(&mut self, fqn: &str) {
        self.uses.insert(fqn.trim_start_matches('\\').to_string());
    }

    /// `$user`, made if it isn't yet; `attrs` are given when it is made.
    fn make_user(&mut self, p: &Project, attrs: &str) {
        if self.user {
            return;
        }
        let fqn = (p.model)("User")
            .map(|m| m.fqn)
            .unwrap_or_else(|| "App\\Models\\User".into());
        self.use_class(&fqn);
        self.setup
            .push(format!("$user = User::factory()->create({attrs});"));
        self.user = true;
    }

    /// A model the request used, made for the test: its variable. `id` is
    /// the id the request knew it by.
    fn make(&mut self, p: &Project, model: &str, var: &str, id: &str) -> String {
        if let Some((_, _, v)) = self.made.iter().find(|(m, i, _)| m == model && i == id) {
            return v.clone();
        }
        // An account handed to a signed-in user is, most often, their own.
        if model == "User" && self.user {
            self.made.push((model.into(), id.into(), "$user".into()));
            return "$user".into();
        }
        let info = (p.model)(model).unwrap_or_else(|| Model {
            fqn: format!("App\\Models\\{model}"),
            ..Model::default()
        });
        self.use_class(&info.fqn);
        let mut var = format!("${var}");
        if self.made.iter().any(|(_, _, v)| *v == var) || var == "$user" {
            var = format!("{var}{}", self.made.len() + 1);
        }
        let attrs = match info.owner.as_deref() {
            Some(col @ ("user_id" | "owner_id" | "author_id")) if self.user => {
                format!("[{} => $user->id]", php_str(col))
            }
            Some(col) if col != "user_id" && col != "owner_id" => {
                self.notes.push(format!(
                    "{model} belongs to a {}: the test's user may need to be part of it",
                    col.trim_end_matches("_id")
                ));
                String::new()
            }
            _ => String::new(),
        };
        if !info.has_factory {
            self.notes.push(format!(
                "{model} has no factory yet: php artisan make:factory {model}Factory --model={model}"
            ));
        }
        self.setup
            .push(format!("{var} = {model}::factory()->create({attrs});"));
        self.made.push((model.into(), id.into(), var.clone()));
        var
    }

    /// The variable for the model the request knew by `id`, if one was made.
    fn var_for_id(&self, id: &str) -> Option<&str> {
        self.made
            .iter()
            .find(|(_, i, _)| i == id)
            .map(|(_, _, v)| v.as_str())
    }
}

fn pascal(s: &str) -> String {
    s.split(['-', '_', ' ', '.'])
        .filter(|p| !p.is_empty())
        .map(|p| {
            let mut c = p.chars();
            c.next()
                .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                .unwrap_or_default()
        })
        .collect()
}

fn camel(s: &str) -> String {
    let p = pascal(s);
    let mut c = p.chars();
    c.next()
        .map(|f| f.to_lowercase().collect::<String>() + c.as_str())
        .unwrap_or_default()
}

/// The guard an `auth:` middleware names, unless it is the default.
fn guard_of(middleware: &str) -> Option<String> {
    middleware.split(',').find_map(|m| {
        let m = m.trim();
        let lower = m.to_ascii_lowercase();
        let class = lower.rsplit('\\').next().unwrap_or(&lower);
        let guard = class
            .strip_prefix("auth:")
            .or_else(|| class.strip_prefix("authenticate:"))?;
        let guard = guard.split(',').next().unwrap_or(guard).trim();
        (!guard.is_empty() && guard != "web").then(|| guard.to_string())
    })
}

/// Each route parameter and the value the request gave it.
fn param_values(uri: &str, path: &str) -> Vec<(String, String)> {
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    uri.split('/')
        .filter(|s| !s.is_empty())
        .enumerate()
        .filter_map(|(i, seg)| {
            let inner = seg.strip_prefix('{')?.strip_suffix('}')?;
            let name = inner.trim_end_matches('?');
            let name = name.split(':').next().unwrap_or(name);
            let value = percent_decode(segs.get(i)?);
            Some((name.to_string(), value))
        })
        .collect()
}

/// The custom key a route binds a parameter by: `{post:slug}` → `slug`.
fn binding_key(uri: &str, param: &str) -> Option<String> {
    let at = uri.find(&format!("{{{param}:"))? + param.len() + 2;
    uri[at..].split('}').next().map(str::to_string)
}

/// The test's file, namespace and class for a route action.
fn place(action: &str) -> (Vec<String>, String) {
    let class = action.split('@').next().unwrap_or(action);
    let class = class.trim_start_matches('\\');
    let rel = class
        .strip_prefix("App\\Http\\Controllers\\")
        .or_else(|| class.strip_prefix("App\\"));
    match rel {
        Some(rel) => {
            let mut parts: Vec<String> = rel.split('\\').map(str::to_string).collect();
            let last = parts.pop().unwrap_or_default();
            (parts, format!("{last}Test"))
        }
        None => (Vec::new(), "RoutesTest".into()),
    }
}

fn with_place(g: &mut Generated, dirs: Vec<String>, class: String) {
    let mut file = PathBuf::from("tests/Feature");
    let mut ns = String::from("Tests\\Feature");
    for d in &dirs {
        file.push(d);
        ns.push('\\');
        ns.push_str(d);
    }
    file.push(format!("{class}.php"));
    g.file = file;
    g.namespace = ns;
    g.class = class;
}

/// `$this->assertStatus`-style assertions for what the request answered.
/// `form` is a write from a page, which on success redirects with no errors.
fn status_assertions(status: u16, form: bool, notes: &mut Vec<String>) -> Vec<String> {
    let redirect = (300..400).contains(&status);
    if status >= 500 {
        notes.push(format!(
            "It failed with {status} when it was captured: the test fails until that is fixed"
        ));
        return if form {
            vec![
                "->assertSessionHasNoErrors()".into(),
                "->assertRedirect()".into(),
            ]
        } else {
            vec!["->assertSuccessful()".into()]
        };
    }
    let mut out = Vec::new();
    if form && redirect {
        out.push("->assertSessionHasNoErrors()".into());
    }
    out.push(match status {
        200 => "->assertOk()".into(),
        201 => "->assertCreated()".into(),
        202 => "->assertAccepted()".into(),
        204 => "->assertNoContent()".into(),
        401 => "->assertUnauthorized()".into(),
        403 => "->assertForbidden()".into(),
        404 => "->assertNotFound()".into(),
        422 => "->assertUnprocessable()".into(),
        _ if redirect => "->assertRedirect()".into(),
        s => format!("->assertStatus({s})"),
    });
    out
}

/// What the request sent, flattened to `(key, value as stored, PHP)`.
fn sent_values(fields: &Val) -> Vec<(String, String, String)> {
    let Val::Map(m) = fields else {
        return Vec::new();
    };
    m.iter()
        .filter_map(|(k, v)| {
            let stored = match v {
                Val::Php(code) if code.ends_with("->id") => return None,
                _ => v.scalar()?,
            };
            Some((k.clone(), stored, v.php(0, false)))
        })
        .collect()
}

/// Database assertions for the rows the request wrote, in terms of what it
/// sent and the models the test made.
fn db_assertions(queries: &[String], fields: &Val, b: &Builder) -> Vec<String> {
    let sent = sent_values(fields);
    let pairs = |cols: &[(String, Option<String>)]| -> Vec<String> {
        cols.iter()
            .filter(|(c, _)| !made_up_column(c))
            .filter_map(|(col, v)| {
                let v = v.as_ref()?;
                if v.is_empty() {
                    return None;
                }
                // The field of that name, or a value long enough to be the
                // one the request sent rather than a coincidence.
                let php = sent
                    .iter()
                    .find(|(k, s, _)| k == col && s == v)
                    .or_else(|| sent.iter().find(|(_, s, _)| v.len() >= 4 && s == v))
                    .map(|(_, _, php)| php.clone())
                    .or_else(|| {
                        col.ends_with("_id")
                            .then(|| b.var_for_id(v).map(|var| format!("{var}->id")))
                            .flatten()
                    })?;
                Some(format!("{} => {php}", php_str(col)))
            })
            .collect()
    };
    let mut out: Vec<String> = Vec::new();
    for w in queries.iter().filter_map(|q| parse_write(q)) {
        let line = match w {
            Write::Insert { table, cols } if !framework_table(&table) => {
                let p = pairs(&cols);
                (!p.is_empty()).then(|| database_has(&table, &p))
            }
            Write::Update { table, sets, id } if !framework_table(&table) => {
                let var = id.as_deref().and_then(|i| b.var_for_id(i));
                let deleted = sets.iter().any(|(c, v)| c == "deleted_at" && v.is_some());
                match var {
                    Some(var) if deleted => Some(format!("$this->assertSoftDeleted({var});")),
                    _ => {
                        let mut p = pairs(&sets);
                        if p.is_empty() {
                            None
                        } else {
                            if let Some(var) = var {
                                p.insert(0, format!("'id' => {var}->id"));
                            }
                            Some(database_has(&table, &p))
                        }
                    }
                }
            }
            Write::Delete { table, id } if !framework_table(&table) => id
                .as_deref()
                .and_then(|i| b.var_for_id(i))
                .map(|var| format!("$this->assertModelMissing({var});")),
            _ => None,
        };
        if let Some(line) = line {
            if !out.contains(&line) {
                out.push(line);
            }
        }
    }
    out
}

/// `$this->assertDatabaseHas('orders', [...])`, one pair per line when it
/// would be long.
fn database_has(table: &str, pairs: &[String]) -> String {
    let one = format!(
        "$this->assertDatabaseHas({}, [{}]);",
        php_str(table),
        pairs.join(", ")
    );
    if one.len() <= 100 {
        return one;
    }
    let mut out = format!("$this->assertDatabaseHas({}, [\n", php_str(table));
    for p in pairs {
        out.push_str(&format!("    {p},\n"));
    }
    out.push_str("]);");
    out
}

/// Put a chain (`$this->…`, `->post(…)`, `->assertOk()`) one call per line.
fn chain(head: &str, calls: &[String]) -> Vec<String> {
    let mut out = vec![head.to_string()];
    for c in calls {
        for (i, l) in c.lines().enumerate() {
            out.push(if i == 0 {
                format!("    {l}")
            } else {
                l.to_string()
            });
        }
    }
    if let Some(last) = out.last_mut() {
        last.push(';');
    }
    out
}

fn is_livewire(c: &Captured) -> bool {
    let path = c.path.split('?').next().unwrap_or("");
    c.header("x-livewire").is_some() || (path.starts_with("/livewire") && path.ends_with("/update"))
}

/// The test for a captured request.
pub fn generate(c: &Captured, p: &Project) -> Result<Generated, String> {
    if is_livewire(c) {
        livewire(c, p)
    } else {
        http(c, p)
    }
}

fn http(c: &Captured, p: &Project) -> Result<Generated, String> {
    let content_type = c.header("content-type").unwrap_or("").to_ascii_lowercase();
    let mut fields = if content_type.contains("json") {
        serde_json::from_str::<Val>(&c.body).unwrap_or(Val::Map(Vec::new()))
    } else if content_type.contains("multipart/form-data") {
        parse_multipart(&c.body, c.header("content-type").unwrap_or(""))
    } else {
        parse_form(&c.body)
    };
    let mut method = c.method.to_ascii_uppercase();
    if let Val::Map(m) = &mut fields {
        if let Some(spoof) = m
            .iter()
            .find(|(k, _)| k == "_method")
            .and_then(|(_, v)| v.as_str())
        {
            if method == "POST" {
                method = spoof.to_ascii_uppercase();
            }
        }
        m.retain(|(k, _)| k != "_method" && k != "_token");
    }
    let (path, query) = c.path.split_once('?').unwrap_or((&c.path, ""));
    let route = route_for(p.routes, &method, path)
        .ok_or_else(|| format!("No route of the app's answers {method} {path}"))?;

    let mut b = Builder::default();
    let mut g = Generated::default();
    let (dirs, class) = place(&route.action);
    with_place(&mut g, dirs, class);
    g.name = format!("{method} /{}", route.uri.trim_start_matches('/'));

    let auth = route.has_auth();
    if auth {
        b.make_user(p, "");
    }

    // Secrets the capture redacted: a password is the factory's, the rest
    // need saying.
    if let Val::Map(m) = &mut fields {
        for (k, v) in m.iter_mut() {
            if v.as_str() == Some("[redacted]") {
                if k.contains("password") {
                    *v = Val::Str("password".into());
                } else {
                    b.notes.push(format!(
                        "{k} was redacted when captured: the test sends 'secret'"
                    ));
                    *v = Val::Str("secret".into());
                }
            }
        }
    }

    // Signing in: the account it signs in to.
    let login = !auth
        && route.uri.trim_end_matches('/').ends_with("login")
        && fields.get("password").is_some();
    if login {
        let email = fields
            .get("email")
            .and_then(Val::as_str)
            .map(|e| format!("['email' => {}]", php_str(e)))
            .unwrap_or_default();
        b.make_user(p, &email);
    }

    // Route parameters: the models they name, made for the test.
    let names = authz::route_params(&route.uri);
    let bound = route
        .action
        .split_once('@')
        .and_then(|(class, m)| Some(((p.source)(class.trim_start_matches('\\'))?, m)))
        .map(|(src, m)| authz::bound_models(&src, m, &names, &|name| (p.model)(name).is_some()))
        .unwrap_or_default();
    let mut args: Vec<(String, String, Option<String>)> = Vec::new();
    for (name, value) in param_values(&route.uri, path) {
        match bound.iter().find(|(param, _)| *param == name) {
            Some((_, model)) => {
                let var = b.make(p, model, &camel(&name), &value);
                args.push((name, var, Some(model.clone())));
            }
            None => args.push((name, php_str(&value), None)),
        }
    }

    // `customer_id` in the body: a customer, made for the test.
    if let Val::Map(m) = &mut fields {
        for (k, v) in m.iter_mut() {
            let Some(base) = k.strip_suffix("_id") else {
                continue;
            };
            let Some(id) = v.scalar().filter(|s| s.parse::<u64>().is_ok()) else {
                continue;
            };
            if base == "user" && b.user {
                *v = Val::Php("$user->id".into());
                continue;
            }
            let model = pascal(base);
            if (p.model)(&model).is_some() {
                let var = b.make(p, &model, &camel(base), &id);
                *v = Val::Php(format!("{var}->id"));
            }
        }
    }
    if let Val::Map(m) = &fields {
        if m.iter()
            .any(|(_, v)| matches!(v, Val::Php(c) if c.starts_with("UploadedFile")))
        {
            b.use_class("Illuminate\\Http\\UploadedFile");
        }
    }

    // The URL: by route name when it has one.
    let query: Vec<(String, String)> = query
        .split('&')
        .filter(|q| !q.is_empty())
        .map(|q| {
            let (k, v) = q.split_once('=').unwrap_or((q, ""));
            (percent_decode(k), percent_decode(v))
        })
        .collect();
    let url = if !route.name.is_empty() {
        let mut items: Vec<String> = args
            .iter()
            .map(|(n, e, _)| format!("{} => {e}", php_str(n)))
            .collect();
        items.extend(
            query
                .iter()
                .map(|(k, v)| format!("{} => {}", php_str(k), php_str(v))),
        );
        match (args.len(), query.len()) {
            (0, 0) => format!("route({})", php_str(&route.name)),
            (1, 0) => format!("route({}, {})", php_str(&route.name), args[0].1),
            _ => format!("route({}, [{}])", php_str(&route.name), items.join(", ")),
        }
    } else {
        let esc = |s: &str| {
            s.replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('$', "\\$")
        };
        let mut out = String::new();
        for seg in route.uri.split('/').filter(|s| !s.is_empty()) {
            out.push('/');
            let param = seg
                .strip_prefix('{')
                .and_then(|s| s.strip_suffix('}'))
                .map(|s| s.trim_end_matches('?'))
                .map(|s| s.split(':').next().unwrap_or(s));
            match param.and_then(|n| args.iter().find(|(a, _, _)| a == n)) {
                Some((n, var, Some(_))) => {
                    let key = binding_key(&route.uri, n).unwrap_or_else(|| "id".into());
                    out.push_str(&format!("{{{var}->{key}}}"));
                }
                Some((_, lit, None)) => out.push_str(&esc(lit.trim_matches('\''))),
                None => out.push_str(&esc(seg)),
            }
        }
        if out.is_empty() {
            out.push('/');
        }
        if !query.is_empty() {
            out.push('?');
            out.push_str(&esc(query
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("&")
                .as_str()));
        }
        if out.contains("{$") {
            format!("\"{out}\"")
        } else {
            php_str(&out.replace("\\$", "$").replace("\\\"", "\""))
        }
    };

    let json = content_type.contains("json")
        || c.header("accept")
            .is_some_and(|a| a.contains("application/json") && !a.contains("text/html"));
    let verb = match method.as_str() {
        "GET" | "HEAD" => "get",
        "POST" => "post",
        "PUT" => "put",
        "PATCH" => "patch",
        "DELETE" => "delete",
        "OPTIONS" => "options",
        _ => "call",
    };
    let verb = if json && verb != "call" {
        format!("{verb}Json")
    } else {
        verb.to_string()
    };
    let has_data = matches!(&fields, Val::Map(m) if !m.is_empty());
    let request = if verb == "call" {
        format!("->call({}, {url})", php_str(&method))
    } else if has_data && verb != "get" && verb != "getJson" {
        // Indented as the line it ends up on: chained under `actingAs`, or
        // the first.
        format!(
            "->{verb}({url}, {})",
            fields.php(if auth { 4 } else { 0 }, true)
        )
    } else {
        format!("->{verb}({url})")
    };

    let form = !json && verb != "get";
    let mut calls = vec![request];
    calls.extend(status_assertions(c.status, form, &mut b.notes));
    let head = match (auth, guard_of(&route.middleware)) {
        (true, Some(guard)) => format!("$this->actingAs($user, {})", php_str(&guard)),
        (true, None) => "$this->actingAs($user)".to_string(),
        (false, _) => "$this".to_string(),
    };
    let mut call = chain(&head, &calls);
    if !auth {
        // `$this` alone on a line reads oddly: start with the request.
        let first = call.remove(1);
        call[0] = format!("$this{}", first.trim_start());
    }

    let mut body = b.setup.clone();
    if !body.is_empty() {
        body.push(String::new());
    }
    body.extend(call);
    let mut after = Vec::new();
    if login && c.status < 400 {
        after.push("$this->assertAuthenticatedAs($user);".to_string());
    }
    if method != "GET" && c.status < 500 {
        after.extend(db_assertions(&c.queries, &fields, &b));
    }
    if !after.is_empty() {
        body.push(String::new());
        body.extend(after);
    }
    g.body = body;
    g.uses = b.uses;
    g.notes = b.notes;
    Ok(g)
}

fn livewire(c: &Captured, p: &Project) -> Result<Generated, String> {
    let payload: Val = serde_json::from_str(&c.body)
        .map_err(|_| "The Livewire request's body was not captured whole".to_string())?;
    let comps: Vec<&Val> = match payload.get("components") {
        Some(Val::List(l)) => l.iter().collect(),
        _ => Vec::new(),
    };
    let busy = |v: &Val| {
        matches!(v.get("updates"), Some(Val::Map(m)) if !m.is_empty())
            || matches!(v.get("calls"), Some(Val::List(l)) if l.iter().any(|c| {
                !matches!(c.get("method").and_then(Val::as_str), Some("$commit" | "$refresh" | "$sync"))
            }))
    };
    let comp = comps
        .iter()
        .find(|v| busy(v))
        .or(comps.first())
        .ok_or("The Livewire request named no component")?;
    let snapshot: Val = comp
        .get("snapshot")
        .and_then(Val::as_str)
        .and_then(|s| serde_json::from_str(s).ok())
        .ok_or("The Livewire request's snapshot was not captured whole")?;
    let memo = snapshot.get("memo");
    let name = memo
        .and_then(|m| m.get("name"))
        .and_then(Val::as_str)
        .ok_or("The Livewire snapshot names no component")?
        .to_string();

    let mut b = Builder::default();
    let mut g = Generated::default();
    let mut dirs = vec!["Livewire".to_string()];
    let parts: Vec<&str> = name
        .rsplit("::")
        .next()
        .unwrap_or(&name)
        .split('.')
        .collect();
    let (last, rest) = parts.split_last().unwrap_or((&"", &[]));
    dirs.extend(rest.iter().map(|d| pascal(&d.replace('⚡', ""))));
    with_place(
        &mut g,
        dirs,
        format!("{}Test", pascal(&last.replace('⚡', ""))),
    );
    b.use_class("Livewire\\Livewire");

    // Signed in when the page it is on needs it.
    let page = memo
        .and_then(|m| m.get("path"))
        .and_then(Val::as_str)
        .unwrap_or("");
    let auth = route_for(
        p.routes,
        "GET",
        &format!("/{}", page.trim_start_matches('/')),
    )
    .is_some_and(|r| r.has_auth());
    if auth {
        b.make_user(p, "");
    }

    // Models it holds, dehydrated as `[null, {"class": …, "key": 5, "s": "mdl"}]`.
    let mut params = Vec::new();
    if let Some(Val::Map(data)) = snapshot.get("data") {
        for (prop, v) in data {
            let Val::List(pair) = v else {
                continue;
            };
            let Some(meta) = pair.get(1) else {
                continue;
            };
            if meta.get("s").and_then(Val::as_str) != Some("mdl") {
                continue;
            }
            let Some(class) = meta.get("class").and_then(Val::as_str) else {
                continue;
            };
            let Some(key) = meta.get("key").and_then(Val::scalar) else {
                continue;
            };
            let model = pascal(class.rsplit('\\').next().unwrap_or(class));
            let var = b.make(p, &model, prop, &key);
            params.push(format!("{} => {var}", php_str(prop)));
        }
    }

    let mut calls = vec![if params.is_empty() {
        format!("->test({})", php_str(&name))
    } else {
        format!("->test({}, [{}])", php_str(&name), params.join(", "))
    }];
    let mut sent: Vec<(String, Val)> = Vec::new();
    if let Some(Val::Map(updates)) = comp.get("updates") {
        for (k, v) in updates {
            calls.push(format!("->set({}, {})", php_str(k), v.php(4, false)));
            sent.push((k.clone(), v.clone()));
        }
    }
    let mut called = Vec::new();
    if let Some(Val::List(list)) = comp.get("calls") {
        for call in list {
            let method = call.get("method").and_then(Val::as_str).unwrap_or("");
            let args: Vec<&Val> = match call.get("params") {
                Some(Val::List(l)) => l.iter().collect(),
                _ => Vec::new(),
            };
            let php: Vec<String> = args.iter().map(|a| a.php(4, false)).collect();
            match method {
                "$commit" | "$refresh" | "$sync" | "" => {}
                "$set" if php.len() >= 2 => calls.push(format!("->set({}, {})", php[0], php[1])),
                "$toggle" if !php.is_empty() => calls.push(format!("->toggle({})", php[0])),
                "__dispatch" if !php.is_empty() => {
                    let mut a = vec![php[0].clone()];
                    if let Some(Val::Map(m)) = args.get(1) {
                        a.extend(m.iter().map(|(k, v)| format!("{k}: {}", v.php(4, false))));
                    }
                    calls.push(format!("->dispatch({})", a.join(", ")));
                }
                m if m.starts_with("_startUpload")
                    || m.starts_with("_finishUpload")
                    || m.starts_with("_upload") =>
                {
                    b.notes.push("It uploaded a file, which the test doesn't repeat: set the property to UploadedFile::fake()->create(…)".into());
                }
                m => {
                    let mut a = vec![php_str(m)];
                    a.extend(php.iter().cloned());
                    calls.push(format!("->call({})", a.join(", ")));
                    called.push(m.to_string());
                    for (i, v) in args.iter().enumerate() {
                        sent.push((format!("{m}#{i}"), (*v).clone()));
                    }
                }
            }
        }
    }
    if c.status >= 500 {
        b.notes.push(format!(
            "It failed with {} when it was captured: the test fails until that is fixed",
            c.status
        ));
    }
    calls.push("->assertHasNoErrors()".into());
    g.name = if called.is_empty() {
        format!("{name} updates")
    } else {
        format!("{name}: {}", called.join(", "))
    };
    let head = if auth {
        "Livewire::actingAs($user)"
    } else {
        "Livewire"
    };
    let mut call = chain(head, &calls);
    if !auth {
        let first = call.remove(1);
        call[0] = format!("Livewire::{}", first.trim_start().trim_start_matches("->"));
    }
    let mut body = b.setup.clone();
    if !body.is_empty() {
        body.push(String::new());
    }
    body.extend(call);
    if c.status < 500 {
        let after = db_assertions(&c.queries, &Val::Map(sent), &b);
        if !after.is_empty() {
            body.push(String::new());
            body.extend(after);
        }
    }
    g.body = body;
    g.uses = b.uses;
    g.notes = b.notes;
    Ok(g)
}

// ---- Into a file ------------------------------------------------------------

/// `test_put_orders_order` for `PUT /orders/{order}`.
fn method_name(name: &str) -> String {
    let mut out = String::from("test");
    let mut gap = true;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            if gap {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
            gap = false;
        } else {
            gap = true;
        }
    }
    out
}

/// The test file with `g` added: a new file when there is none, otherwise the
/// one there with the test at the end and any imports it lacks. `refresh`
/// adds `RefreshDatabase` to a new file. Returns the file and the 1-based
/// line the test starts on.
pub fn render(existing: Option<&str>, g: &Generated, pest: bool, refresh: bool) -> (String, usize) {
    let mut uses = g.uses.clone();
    let fresh = existing.is_none();
    if fresh && refresh {
        uses.insert("Illuminate\\Foundation\\Testing\\RefreshDatabase".into());
    }
    if !pest {
        uses.insert("Tests\\TestCase".into());
    }
    let src = existing.map(str::to_string).unwrap_or_default();

    // A name the file doesn't have yet.
    let taken = |n: &str| {
        if pest {
            src.contains(&format!("'{}'", n.replace('\'', "\\'")))
        } else {
            src.contains(&format!("function {}(", method_name(n)))
        }
    };
    let mut name = g.name.clone();
    let mut k = 2;
    while taken(&name) {
        name = format!("{} {k}", g.name);
        k += 1;
    }

    let indent = if pest { 4 } else { 8 };
    let pad = " ".repeat(indent);
    let body: Vec<String> = g
        .body
        .iter()
        // `"".lines()` is nothing: keep a blank line blank, not gone.
        .flat_map(|l| l.split('\n').map(str::to_string).collect::<Vec<_>>())
        .map(|l| if l.is_empty() { l } else { format!("{pad}{l}") })
        .collect();
    let block = if pest {
        format!(
            "test({}, function () {{\n{}\n}});\n",
            php_str(&name),
            body.join("\n")
        )
    } else {
        format!(
            "    public function {}(): void\n    {{\n{}\n    }}\n",
            method_name(&name),
            body.join("\n")
        )
    };

    if fresh {
        let mut out = String::from("<?php\n\n");
        if !pest {
            out.push_str(&format!("namespace {};\n\n", g.namespace));
        }
        for u in &uses {
            out.push_str(&format!("use {u};\n"));
        }
        if !uses.is_empty() {
            out.push('\n');
        }
        if pest {
            if refresh {
                out.push_str("uses(RefreshDatabase::class);\n\n");
            }
            let line = out.lines().count() + 1;
            out.push_str(&block);
            return (out, line);
        }
        out.push_str(&format!("class {} extends TestCase\n{{\n", g.class));
        if refresh {
            out.push_str("    use RefreshDatabase;\n\n");
        }
        let line = out.lines().count() + 1;
        out.push_str(&block);
        out.push_str("}\n");
        return (out, line);
    }

    // Imports it lacks, after the last one there (or after `<?php`).
    let mut lines: Vec<String> = src.lines().map(str::to_string).collect();
    let missing: Vec<String> = uses
        .iter()
        .filter(|u| {
            let short = u.rsplit('\\').next().unwrap_or(u);
            !lines.iter().any(|l| {
                l.starts_with("use ")
                    && (l.trim_end_matches(';').ends_with(&format!("\\{short}"))
                        || l.contains(&format!(" as {short};")))
            })
        })
        .map(|u| format!("use {u};"))
        .collect();
    if !missing.is_empty() {
        let first_code = lines
            .iter()
            .position(|l| {
                let t = l.trim();
                !(t.is_empty()
                    || t.starts_with("<?php")
                    || t.starts_with("namespace ")
                    || t.starts_with("use ")
                    || t.starts_with("declare("))
            })
            .unwrap_or(lines.len());
        match lines[..first_code]
            .iter()
            .rposition(|l| l.starts_with("use "))
        {
            Some(last) => {
                for (i, m) in missing.into_iter().enumerate() {
                    lines.insert(last + 1 + i, m);
                }
            }
            None => {
                let after = lines[..first_code]
                    .iter()
                    .rposition(|l| l.starts_with("<?php") || l.starts_with("namespace "))
                    .map_or(0, |i| i + 1);
                let mut add = vec![String::new()];
                add.extend(missing);
                for (i, m) in add.into_iter().enumerate() {
                    lines.insert(after + i, m);
                }
            }
        }
    }
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    if pest {
        lines.push(String::new());
        let line = lines.len() + 1;
        let mut out = lines.join("\n");
        out.push('\n');
        out.push_str(&block);
        return (out, line);
    }
    // Before the class's closing brace.
    let close = lines
        .iter()
        .rposition(|l| l.trim() == "}")
        .unwrap_or(lines.len());
    let mut tail = lines.split_off(close);
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    lines.push(String::new());
    let line = lines.len() + 1;
    lines.extend(block.trim_end().lines().map(str::to_string));
    lines.append(&mut tail);
    (lines.join("\n") + "\n", line)
}

// ---- The project ------------------------------------------------------------

/// What [`for_project`] made.
#[derive(Debug)]
pub struct Written {
    pub path: PathBuf,
    /// The whole file, with the test in it.
    pub content: String,
    /// The 1-based line the test starts on.
    pub line: usize,
    pub name: String,
    pub notes: Vec<String>,
}

/// Lines of a file that aren't comments.
fn code_lines(src: &str) -> impl Iterator<Item = &str> {
    src.lines().filter(|l| {
        let t = l.trim_start();
        !(t.starts_with("//") || t.starts_with('*') || t.starts_with("/*") || t.starts_with('#'))
    })
}

/// Whether the project's tests already reset the database themselves.
fn resets_database(root: &Path) -> bool {
    ["tests/Pest.php", "tests/TestCase.php"].iter().any(|f| {
        std::fs::read_to_string(root.join(f)).is_ok_and(|src| {
            code_lines(&src).any(|l| {
                [
                    "RefreshDatabase",
                    "DatabaseTransactions",
                    "DatabaseMigrations",
                    "DatabaseTruncation",
                ]
                .iter()
                .any(|t| l.contains(t))
            })
        })
    })
}

/// Whether the tests run against a database of their own (`phpunit.xml` or
/// `.env.testing` name one) rather than the app's.
pub fn separate_test_database(root: &Path) -> bool {
    let test_db = |v: &str| {
        let v = v.trim().trim_matches(['"', '\'']);
        v == ":memory:" || v.to_ascii_lowercase().contains("test")
    };
    let from_xml = ["phpunit.xml", "phpunit.xml.dist"].iter().any(|f| {
        let Ok(src) = std::fs::read_to_string(root.join(f)) else {
            return false;
        };
        // Without its comments: a commented-out `DB_DATABASE` sets nothing.
        let mut xml = String::new();
        let mut rest = src.as_str();
        while let Some(open) = rest.find("<!--") {
            xml.push_str(&rest[..open]);
            match rest[open..].find("-->") {
                Some(c) => rest = &rest[open + c + 3..],
                None => rest = "",
            }
        }
        xml.push_str(rest);
        xml.match_indices("name=\"DB_DATABASE\"").any(|(i, _)| {
            let tag_end = xml[i..].find('>').map_or(xml.len(), |e| i + e);
            xml[i..tag_end]
                .split_once("value=\"")
                .and_then(|(_, v)| v.split('"').next())
                .is_some_and(test_db)
        })
    });
    from_xml
        || std::fs::read_to_string(root.join(".env.testing")).is_ok_and(|env| {
            env.lines()
                .filter_map(|l| l.trim().strip_prefix("DB_DATABASE="))
                .any(test_db)
        })
}

/// One of the app's models, from `app/Models` (or `app/`).
fn read_model(root: &Path, name: &str) -> Option<Model> {
    let read = |name: &str| {
        [
            root.join("app/Models").join(format!("{name}.php")),
            root.join("app").join(format!("{name}.php")),
        ]
        .into_iter()
        .find_map(|p| {
            let src = std::fs::read_to_string(&p).ok()?;
            src.contains(&format!("class {name} ")).then_some(src)
        })
    };
    let src = read(name)?;
    // A model that renames another (`class Subscription extends
    // UserSubscription {}`) is owned as that one is, and has its traits.
    let parent = src
        .split(&format!("class {name} extends "))
        .nth(1)
        .map(|r| {
            r.chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect::<String>()
        })
        .filter(|p| !["Model", "Authenticatable", "Pivot", name].contains(&p.as_str()))
        .and_then(|p| read(&p));
    let facts = authz::model_facts(&src, &|_| None);
    let owner = facts
        .owner
        .or_else(|| {
            parent
                .as_deref()
                .and_then(|p| authz::model_facts(p, &|_| None).owner)
        })
        .map(|o| {
            if o.ends_with("_id") {
                o
            } else {
                format!("{o}_id")
            }
        });
    let has_trait = |s: &str| s.contains("HasFactory") || s.contains("UseFactory");
    Some(Model {
        fqn: authz::class_of(&src).unwrap_or_else(|| format!("App\\Models\\{name}")),
        // The factory is found by the class's own name.
        has_factory: (has_trait(&src) || parent.as_deref().is_some_and(has_trait))
            && root
                .join("database/factories")
                .join(format!("{name}Factory.php"))
                .exists(),
        owner,
    })
}

/// The test for a captured request, put in the project's test file for it.
/// Blocking: reads the controller, models and tests. Writes nothing.
pub fn for_project(root: &Path, routes: &[RouteInfo], c: &Captured) -> Result<Written, String> {
    let model = |name: &str| read_model(root, name);
    let source = |fqn: &str| {
        let rel = fqn.strip_prefix("App\\")?;
        std::fs::read_to_string(
            root.join("app")
                .join(format!("{}.php", rel.replace('\\', "/"))),
        )
        .ok()
    };
    let project = Project {
        routes,
        model: &model,
        source: &source,
    };
    let mut g = generate(c, &project)?;
    let pest = root.join("tests/Pest.php").exists()
        || std::fs::read_to_string(root.join("composer.json"))
            .is_ok_and(|s| s.contains("pestphp/pest"));
    let resets = resets_database(root);
    let path = root.join(&g.file);
    let existing = std::fs::read_to_string(&path).ok();
    let uses_db = g
        .body
        .iter()
        .any(|l| l.contains("::factory()") || l.contains("$this->assert"));
    let refresh = !resets && uses_db;
    if (refresh || resets) && !separate_test_database(root) {
        g.notes.insert(
            0,
            "The tests here use the app's own database (phpunit.xml names no test database): \
             refreshing it would empty it. Give the tests one of their own before running this"
                .into(),
        );
    }
    let (content, line) = render(existing.as_deref(), &g, pest, refresh);
    Ok(Written {
        path,
        content,
        line,
        name: g.name,
        notes: g.notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(methods: &str, uri: &str, name: &str, action: &str, middleware: &str) -> RouteInfo {
        RouteInfo {
            name: name.into(),
            uri: uri.into(),
            methods: methods.into(),
            action: action.into(),
            middleware: middleware.into(),
        }
    }

    fn routes() -> Vec<RouteInfo> {
        vec![
            route(
                "PUT",
                "orders/{order}",
                "orders.update",
                "App\\Http\\Controllers\\OrderController@update",
                "web,auth",
            ),
            route(
                "POST",
                "orders",
                "",
                "App\\Http\\Controllers\\OrderController@store",
                "web,auth",
            ),
            route(
                "DELETE",
                "api/orders/{order}",
                "",
                "App\\Http\\Controllers\\Api\\OrderController@destroy",
                "api,auth:sanctum",
            ),
            route(
                "POST",
                "login",
                "login",
                "App\\Http\\Controllers\\Auth\\LoginController@store",
                "web,guest",
            ),
            route(
                "GET",
                "app/stations/{station}",
                "",
                "App\\Livewire\\StationPage",
                "web,auth",
            ),
            route("GET", "reports", "reports", "Closure", "web"),
        ]
    }

    const CONTROLLER: &str = "<?php
namespace App\\Http\\Controllers;
class OrderController {
    public function update(UpdateOrder $request, Order $order) { $order->update($request->validated()); return back(); }
    public function store(Request $request) { Order::create($request->all()); return back(); }
    public function destroy(int $order) { Order::findOrFail($order)->delete(); return response()->noContent(); }
}";

    fn model(name: &str) -> Option<Model> {
        match name {
            "Order" => Some(Model {
                fqn: "App\\Models\\Order".into(),
                has_factory: true,
                owner: Some("user_id".into()),
            }),
            "Customer" => Some(Model {
                fqn: "App\\Models\\Customer".into(),
                has_factory: false,
                owner: Some("team_id".into()),
            }),
            "User" => Some(Model {
                fqn: "App\\Models\\User".into(),
                has_factory: true,
                owner: None,
            }),
            "Station" => Some(Model {
                fqn: "App\\Models\\Station".into(),
                has_factory: true,
                owner: None,
            }),
            _ => None,
        }
    }

    fn gen(c: &Captured) -> Generated {
        let routes = routes();
        let source = |_: &str| Some(CONTROLLER.to_string());
        let p = Project {
            routes: &routes,
            model: &model,
            source: &source,
        };
        generate(c, &p).unwrap()
    }

    fn captured(method: &str, path: &str, status: u16, body: &str, queries: &[&str]) -> Captured {
        Captured {
            method: method.into(),
            path: path.into(),
            status,
            headers: vec![(
                "content-type".into(),
                "application/x-www-form-urlencoded".into(),
            )],
            body: body.into(),
            queries: queries.iter().map(|q| q.to_string()).collect(),
        }
    }

    #[test]
    fn a_form_update_becomes_a_test_with_its_model_and_what_it_wrote() {
        let g = gen(&captured(
            "POST",
            "/orders/42",
            302,
            "_token=%5Bredacted%5D&_method=PUT&status=paid&note=Leave+at+the+door",
            &[
                "select * from `orders` where `id` = 42 limit 1",
                "update `orders` set `status` = 'paid', `note` = 'Leave at the door', `updated_at` = '2026-10-26 12:00:00' where `id` = 42",
            ],
        ));
        assert_eq!(g.name, "PUT /orders/{order}");
        assert_eq!(
            g.file,
            PathBuf::from("tests/Feature/OrderControllerTest.php")
        );
        assert_eq!(
            g.body.join("\n"),
            "$user = User::factory()->create();
$order = Order::factory()->create(['user_id' => $user->id]);

$this->actingAs($user)
    ->put(route('orders.update', $order), [
        'status' => 'paid',
        'note' => 'Leave at the door',
    ])
    ->assertSessionHasNoErrors()
    ->assertRedirect();

$this->assertDatabaseHas('orders', [
    'id' => $order->id,
    'status' => 'paid',
    'note' => 'Leave at the door',
]);"
        );
        assert!(g.uses.contains("App\\Models\\Order"));
        assert!(g.notes.is_empty(), "{:?}", g.notes);
    }

    #[test]
    fn ids_in_the_body_become_models_and_inserts_assert_what_was_sent() {
        let g = gen(&captured(
            "POST",
            "/orders",
            302,
            "customer_id=7&reference=PO-1234&qty=1&items%5B0%5D%5Bsku%5D=A1&items%5B1%5D%5Bsku%5D=B2",
            &["insert into `orders` (`customer_id`, `reference`, `qty`, `status`, `updated_at`, `created_at`) values (7, 'PO-1234', 1, 'new', '2026-10-26', '2026-10-26')"],
        ));
        let body = g.body.join("\n");
        assert!(
            body.contains("$customer = Customer::factory()->create();"),
            "{body}"
        );
        assert!(body.contains("'customer_id' => $customer->id,"), "{body}");
        assert!(
            body.contains("'items' => [['sku' => 'A1'], ['sku' => 'B2']],"),
            "{body}"
        );
        // An unnamed route is called by its path.
        assert!(body.contains("->post('/orders', ["), "{body}");
        // `status` the app chose, not the request: not asserted.
        assert!(
            body.contains(
                "$this->assertDatabaseHas('orders', [\n    'customer_id' => $customer->id,\n    'reference' => 'PO-1234',\n    'qty' => '1',\n]);"
            ),
            "{body}"
        );
        assert!(g
            .notes
            .iter()
            .any(|n| n.contains("Customer has no factory")));
        assert!(g.notes.iter().any(|n| n.contains("belongs to a team")));
    }

    #[test]
    fn an_api_delete_on_a_guard_asserts_the_row_is_gone() {
        let mut c = captured(
            "DELETE",
            "/api/orders/9",
            204,
            "",
            &["delete from `orders` where `id` = 9"],
        );
        c.headers = vec![("accept".into(), "application/json".into())];
        let g = gen(&c);
        assert_eq!(
            g.file,
            PathBuf::from("tests/Feature/Api/OrderControllerTest.php")
        );
        assert_eq!(g.namespace, "Tests\\Feature\\Api");
        let body = g.body.join("\n");
        assert!(body.contains("$this->actingAs($user, 'sanctum')"), "{body}");
        assert!(
            body.contains("->deleteJson(\"/api/orders/{$order->id}\")"),
            "{body}"
        );
        assert!(body.contains("->assertNoContent();"), "{body}");
        assert!(
            body.contains("$this->assertModelMissing($order);"),
            "{body}"
        );
    }

    #[test]
    fn a_login_makes_the_account_and_a_crash_becomes_a_red_test() {
        let g = gen(&captured(
            "POST",
            "/login",
            302,
            "_token=[redacted]&email=admin%40example.com&password=[redacted]",
            &[],
        ));
        let body = g.body.join("\n");
        assert!(
            body.starts_with("$user = User::factory()->create(['email' => 'admin@example.com']);"),
            "{body}"
        );
        assert!(body.contains("$this->post(route('login'), [\n    'email' => 'admin@example.com',\n    'password' => 'password',\n])\n    ->assertSessionHasNoErrors()"), "{body}");
        assert!(
            body.contains("$this->assertAuthenticatedAs($user);"),
            "{body}"
        );
        assert!(!body.contains("actingAs"), "{body}");

        let g = gen(&captured("GET", "/reports?year=2026", 500, "", &[]));
        assert_eq!(g.file, PathBuf::from("tests/Feature/RoutesTest.php"));
        assert_eq!(
            g.body.join("\n"),
            "$this->get(route('reports', ['year' => '2026']))\n    ->assertSuccessful();"
        );
        assert!(g.notes[0].contains("failed with 500"));
    }

    #[test]
    fn a_livewire_update_becomes_a_livewire_test() {
        let snapshot = serde_json::json!({
            "data": {"station": [null, {"class": "App\\Models\\Station", "key": 5, "s": "mdl"}], "showModal": false},
            "memo": {"name": "admin.station-notes", "path": "app/stations/5", "method": "GET"},
            "checksum": "x"
        })
        .to_string();
        // Written out, so the updates keep the order they were made in.
        let body = format!(
            r#"{{"_token":"[redacted]","components":[{{"snapshot":{},"updates":{{"note":"Cable fault","form.priority":2}},"calls":[{{"method":"save","params":[],"metadata":{{}}}},{{"method":"$refresh","params":[]}}]}}]}}"#,
            serde_json::to_string(&snapshot).unwrap()
        );
        let c = Captured {
            method: "POST".into(),
            path: "/livewire-2813e777/update".into(),
            status: 200,
            headers: vec![("x-livewire".into(), "1".into())],
            body,
            queries: vec!["insert into `notes` (`station_id`, `body`, `created_at`) values (5, 'Cable fault', '2026')".into()],
        };
        let g = gen(&c);
        assert_eq!(
            g.file,
            PathBuf::from("tests/Feature/Livewire/Admin/StationNotesTest.php")
        );
        assert_eq!(g.name, "admin.station-notes: save");
        assert_eq!(
            g.body.join("\n"),
            "$user = User::factory()->create();
$station = Station::factory()->create();

Livewire::actingAs($user)
    ->test('admin.station-notes', ['station' => $station])
    ->set('note', 'Cable fault')
    ->set('form.priority', 2)
    ->call('save')
    ->assertHasNoErrors();

$this->assertDatabaseHas('notes', ['station_id' => $station->id, 'body' => 'Cable fault']);"
        );
        assert!(g.uses.contains("Livewire\\Livewire"));
    }

    #[test]
    fn sql_writes_are_read_with_their_values() {
        assert_eq!(
            parse_write("update `orders` set `note` = 'it''s here', `total` = 12.5, `paid_at` = now() where `orders`.`id` = 42 and `deleted_at` is null"),
            Some(Write::Update {
                table: "orders".into(),
                sets: vec![
                    ("note".into(), Some("it's here".into())),
                    ("total".into(), Some("12.5".into())),
                    ("paid_at".into(), None),
                ],
                id: Some("42".into()),
            })
        );
        assert_eq!(
            parse_write("delete from \"orders\" where \"id\" in (3)"),
            Some(Write::Delete {
                table: "orders".into(),
                id: Some("3".into())
            })
        );
        assert_eq!(parse_write("select * from orders where id = 1"), None);
    }

    #[test]
    fn a_test_goes_in_a_new_file_or_at_the_end_of_the_one_there() {
        let g = Generated {
            file: "tests/Feature/OrderControllerTest.php".into(),
            namespace: "Tests\\Feature".into(),
            class: "OrderControllerTest".into(),
            name: "GET /orders".into(),
            uses: ["App\\Models\\User".to_string()].into_iter().collect(),
            body: vec!["$this->get('/orders')".into(), "    ->assertOk();".into()],
            notes: Vec::new(),
        };
        let (pest, line) = render(None, &g, true, true);
        assert_eq!(
            pest,
            "<?php

use App\\Models\\User;
use Illuminate\\Foundation\\Testing\\RefreshDatabase;

uses(RefreshDatabase::class);

test('GET /orders', function () {
    $this->get('/orders')
        ->assertOk();
});
"
        );
        assert_eq!(line, 8);
        // Again: the same name gets a number, the import isn't repeated.
        let (again, line) = render(Some(&pest), &g, true, true);
        assert!(again.ends_with("});\n\ntest('GET /orders 2', function () {\n    $this->get('/orders')\n        ->assertOk();\n});\n"), "{again}");
        assert_eq!(again.matches("use App\\Models\\User;").count(), 1);
        assert_eq!(
            again.lines().nth(line - 1),
            Some("test('GET /orders 2', function () {")
        );

        let (unit, line) = render(None, &g, false, false);
        assert!(unit.contains("namespace Tests\\Feature;\n\nuse App\\Models\\User;\nuse Tests\\TestCase;\n\nclass OrderControllerTest extends TestCase\n{\n    public function test_get_orders(): void\n    {\n        $this->get('/orders')\n            ->assertOk();\n    }\n}\n"), "{unit}");
        assert_eq!(
            unit.lines().nth(line - 1),
            Some("    public function test_get_orders(): void")
        );
        let existing = "<?php\n\nnamespace Tests\\Feature;\n\nuse Tests\\TestCase;\n\nclass OrderControllerTest extends TestCase\n{\n    public function test_index(): void\n    {\n    }\n}\n";
        let (added, line) = render(Some(existing), &g, false, true);
        assert!(
            added.contains("use Tests\\TestCase;\nuse App\\Models\\User;\n"),
            "{added}"
        );
        assert!(added.ends_with("    }\n\n    public function test_get_orders(): void\n    {\n        $this->get('/orders')\n            ->assertOk();\n    }\n}\n"), "{added}");
        assert!(!added.contains("RefreshDatabase"));
        assert_eq!(
            added.lines().nth(line - 1),
            Some("    public function test_get_orders(): void")
        );
    }

    #[test]
    fn a_test_database_is_one_phpunit_names() {
        let dir = std::env::temp_dir().join(format!("e-reqtest-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("phpunit.xml"),
            "<php>\n<!-- <env name=\"DB_DATABASE\" value=\":memory:\"/> -->\n<env name=\"APP_ENV\" value=\"testing\"/>\n</php>",
        )
        .unwrap();
        assert!(!separate_test_database(&dir));
        std::fs::write(
            dir.join("phpunit.xml"),
            "<php>\n<env name=\"DB_CONNECTION\" value=\"sqlite\"/>\n<env name=\"DB_DATABASE\" value=\":memory:\"/>\n</php>",
        )
        .unwrap();
        assert!(separate_test_database(&dir));
        std::fs::remove_dir_all(&dir).ok();
    }
}
