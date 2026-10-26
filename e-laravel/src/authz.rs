//! Controller methods that hand a signed-in user a model by its id without
//! checking it is theirs — the insecure direct object reference (IDOR):
//! `GET /orders/{order}` behind `auth`, a method that takes `Order $order` (or
//! does `Order::findOrFail($id)`), and nothing that asks whether this user may
//! see that order. Any user can then read anyone's order by changing the id.
//!
//! It's a heuristic and errs towards silence. Only models that belong to
//! someone count — a `user()`/`team()`/`workspace()` relation, a `user_id` or
//! `team_id` — and not those a global scope already confines (a tenant or
//! team trait). A method is only reported when nothing in it, its route or
//! its controller looks like authorization —
//! `authorize()`, `Gate`, `can()`, a FormRequest whose `authorize()` does more
//! than `return true`, `can:`/admin/role middleware, `authorizeResource()` — or
//! like scoping to the user: `$request->user()->orders()`, `auth()->id()`,
//! `user_id`, `whereBelongsTo`, or a helper like `$this->ensureOwns($order)`.
//! Some models are meant to be read by every signed-in user; the message
//! says so.

use std::collections::HashMap;

use crate::laravel::RouteInfo;

/// A method that looks a model up by a route parameter with no check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub method: String,
    /// 0-based line of the method's `function`.
    pub line: usize,
    /// 0-based line of its closing brace.
    pub end_line: usize,
    /// The models it hands out, by the parameter they come from:
    /// `(param, model)`.
    pub lookups: Vec<(String, String)>,
    /// What the first model belongs to: `user_id`, `team`.
    pub owner: String,
    /// `GET orders/{order}`.
    pub route: String,
}

impl Finding {
    pub fn message(&self) -> String {
        let models = self
            .lookups
            .iter()
            .map(|(p, m)| format!("{m} {{{p}}}"))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "{} hands any signed-in user the {models} they ask for, and it belongs to \
             someone ({}): nothing here checks it is theirs — no authorize() or policy, no \
             can: middleware, no scoping to the user. If every user may see any of them, \
             ignore this.",
            self.route, self.owner
        )
    }
}

/// Route parameter names in a URI: `orders/{order}/items/{item?}` →
/// `order`, `item` (`{post:slug}` → `post`).
pub fn route_params(uri: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = uri;
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open..].find('}') else {
            break;
        };
        let inner = &rest[open + 1..open + close];
        let name = inner.trim_end_matches('?');
        let name = name.split(':').next().unwrap_or(name);
        if !name.is_empty() {
            out.push(name.to_string());
        }
        rest = &rest[open + close + 1..];
    }
    out
}

/// Middleware that decides who may do what: `can:`, Laravel's `Authorize`,
/// roles, permissions, admin gates.
fn authorizing_middleware(middleware: &str) -> bool {
    let m = middleware.to_ascii_lowercase();
    [
        "can:",
        "authorize",
        "role",
        "permission",
        "admin",
        "ability",
        "policy",
        "owner",
        // A signed URL can't be made up for someone else's record either.
        "signed",
        "validatesignature",
    ]
    .iter()
    .any(|w| m.contains(w))
}

/// Code that checks permission, or ties the lookup to the user.
const CHECKS: &[&str] = &[
    "authorize(",
    "authorizeForUser(",
    "Gate::",
    "->can(",
    "->cannot(",
    "->cant(",
    "->allows(",
    "->denies(",
    "policy(",
    "abort_unless(",
    "abort_if(",
    "AuthorizationException",
    "Response::deny",
    "->user()->",
    "auth()->id()",
    "auth()->user()",
    "Auth::id()",
    "Auth::user()",
    "auth()->guard(",
    "whereBelongsTo(",
    "user_id",
    "owner",
    "currentTeam",
    "team_id",
    "tenant",
];

/// What decides whether looking a model up unchecked matters.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModelFacts {
    /// What it belongs to (`user_id`, `team`), if anything.
    pub owner: Option<String>,
    /// A global scope confines it already (a tenant or team trait).
    pub scoped: bool,
}

/// Columns and relations that make a model someone's.
const OWNER_COLUMNS: &[&str] = &[
    "user_id",
    "owner_id",
    "team_id",
    "workspace_id",
    "tenant_id",
    "account_id",
    "organization_id",
    "organisation_id",
    "company_id",
    "customer_id",
];
const OWNER_RELATIONS: &[&str] = &[
    "User",
    "Team",
    "Workspace",
    "Tenant",
    "Account",
    "Organization",
    "Organisation",
    "Company",
    "Customer",
];

/// What a model's source says about who owns it and whether a global scope
/// confines it. `trait_src` reads one of its traits' source, if it is the
/// app's.
pub fn model_facts(src: &str, trait_src: &dyn Fn(&str) -> Option<String>) -> ModelFacts {
    // A key named on a `hasMany(…, 'user_id')` line is the other table's.
    let own_lines: Vec<&str> = src
        .lines()
        .filter(|l| {
            ![
                "hasMany",
                "hasOne",
                "belongsToMany",
                "morphMany",
                "morphOne",
            ]
            .iter()
            .any(|r| l.contains(r))
        })
        .collect();
    let owner = OWNER_COLUMNS
        .iter()
        .find(|c| {
            own_lines
                .iter()
                .any(|l| l.contains(&format!("'{c}'")) || l.contains(&format!("\"{c}\"")))
        })
        .map(|c| c.to_string())
        .or_else(|| {
            // `belongsTo(User::class)`, or with an owner's key — not
            // `belongsTo(User::class, 'created_by')`, which says who made the
            // row, not whose it is.
            OWNER_RELATIONS
                .iter()
                .find(|r| {
                    src.match_indices(&format!("belongsTo({r}::class"))
                        .any(|(i, m)| {
                            let rest = src[i + m.len()..].trim_start();
                            rest.starts_with(')')
                                || rest
                                    .strip_prefix(',')
                                    .map(|a| a.trim_start().trim_start_matches(['\'', '"']))
                                    .is_some_and(|a| OWNER_COLUMNS.iter().any(|c| a.starts_with(c)))
                        })
                })
                .map(|r| r.to_ascii_lowercase())
        });
    let traits: Vec<String> = src
        .lines()
        .filter_map(|l| l.trim().strip_prefix("use "))
        .filter(|l| !l.contains('\\'))
        .flat_map(|l| {
            l.trim_end_matches(';')
                .split(',')
                .map(|t| t.trim().to_string())
                .collect::<Vec<_>>()
        })
        .collect();
    let scoped = src.contains("addGlobalScope")
        || src.contains("#[ScopedBy")
        || traits.iter().any(|t| {
            [
                "Tenant",
                "Team",
                "Workspace",
                "Scope",
                "Organization",
                "Account",
            ]
            .iter()
            .any(|w| t.contains(w))
                || trait_src(t).is_some_and(|s| s.contains("addGlobalScope"))
        });
    ModelFacts { owner, scoped }
}

/// A call to one of the controller's own helpers with the model, that by
/// its name checks something: `$this->ensureOwns($order)`.
fn helper_check(body: &str, params: &[String]) -> bool {
    const WORDS: &[&str] = &[
        "ensure", "check", "verify", "assert", "authoriz", "guard", "belong", "own", "permi",
        "access", "allow", "deny", "abort", "can",
    ];
    body.match_indices("$this->").any(|(i, _)| {
        let rest = &body[i + 7..];
        let name: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        let lower = name.to_ascii_lowercase();
        let Some(args) = rest[name.len()..].strip_prefix('(') else {
            return false;
        };
        let args = &args[..args.find(')').unwrap_or(args.len())];
        WORDS.iter().any(|w| lower.contains(w))
            && params.iter().any(|p| args.contains(&format!("${p}")))
    })
}

/// One method of a class: name, line, parameter list, body.
struct Method<'a> {
    name: &'a str,
    line: usize,
    end_line: usize,
    params: &'a str,
    body: &'a str,
}

/// Skip a string or comment starting at `i`; the index after it, or `None`
/// when `i` doesn't start one.
fn skip_literal(b: &[u8], i: usize) -> Option<usize> {
    match b[i] {
        q @ (b'\'' | b'"') => {
            let mut j = i + 1;
            while j < b.len() && b[j] != q {
                if b[j] == b'\\' {
                    j += 1;
                }
                j += 1;
            }
            Some(j + 1)
        }
        b'/' if b.get(i + 1) == Some(&b'/') => Some(
            b[i..]
                .iter()
                .position(|c| *c == b'\n')
                .map_or(b.len(), |p| i + p),
        ),
        b'#' if b.get(i + 1) != Some(&b'[') => Some(
            b[i..]
                .iter()
                .position(|c| *c == b'\n')
                .map_or(b.len(), |p| i + p),
        ),
        b'/' if b.get(i + 1) == Some(&b'*') => Some(
            b[i + 2..]
                .windows(2)
                .position(|w| w == b"*/")
                .map_or(b.len(), |p| i + 2 + p + 2),
        ),
        _ => None,
    }
}

/// The index of the bracket closing the one at `open`.
fn matching(src: &str, open: usize, (o, c): (u8, u8)) -> Option<usize> {
    let b = src.as_bytes();
    let mut depth = 0;
    let mut i = open;
    while i < b.len() {
        if let Some(next) = skip_literal(b, i) {
            i = next;
            continue;
        }
        if b[i] == o {
            depth += 1;
        } else if b[i] == c {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

fn methods(src: &str) -> Vec<Method<'_>> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(at) = src[from..].find("function ") {
        let start = from + at;
        from = start + 9;
        let name_start = from;
        let name_len = src[name_start..]
            .find(|c: char| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(0);
        if name_len == 0 {
            continue; // a closure
        }
        let name = &src[name_start..name_start + name_len];
        let Some(open) = src[name_start..].find('(').map(|i| name_start + i) else {
            continue;
        };
        let Some(close) = matching(src, open, (b'(', b')')) else {
            continue;
        };
        let Some(brace) = src[close..].find(['{', ';']).map(|i| close + i) else {
            continue;
        };
        if src.as_bytes()[brace] == b';' {
            continue; // abstract / interface
        }
        let Some(end) = matching(src, brace, (b'{', b'}')) else {
            continue;
        };
        out.push(Method {
            name,
            line: src[..start].matches('\n').count(),
            end_line: src[..end].matches('\n').count(),
            params: &src[open + 1..close],
            body: &src[brace..=end],
        });
        from = end;
    }
    out
}

/// `(type, name)` for each typed parameter: `Order $order` → `("Order", "order")`.
fn typed_params(params: &str) -> Vec<(String, String)> {
    params
        .split(',')
        .filter_map(|p| {
            let p = p.split('=').next()?.trim();
            let dollar = p.find('$')?;
            let ty = p[..dollar]
                .split_whitespace()
                .last()?
                .trim_start_matches('?')
                .rsplit('\\')
                .next()?
                .to_string();
            let name: String = p[dollar + 1..]
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            (!ty.is_empty() && !name.is_empty()).then_some((ty, name))
        })
        .collect()
}

/// Models a method body looks up by one of `params`:
/// `Order::findOrFail($id)` → `("id", "Order")`.
fn lookups_in_body(
    body: &str,
    params: &[String],
    is_model: &dyn Fn(&str) -> bool,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for stmt in body.split(';') {
        let Some(colons) = stmt.find("::") else {
            continue;
        };
        let model: String = stmt[..colons]
            .chars()
            .rev()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        if model.is_empty() || !is_model(&model) {
            continue;
        }
        let tail = &stmt[colons..];
        if ![
            "find(",
            "findOrFail(",
            "firstWhere(",
            "where(",
            "whereKey(",
            "query(",
        ]
        .iter()
        .any(|f| tail.contains(f))
        {
            continue;
        }
        for p in params {
            let var = format!("${p}");
            // The parameter itself, not a property or an element of it.
            let used = tail.match_indices(&var).any(|(i, _)| {
                let after = &tail[i + var.len()..];
                !after
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '[')
                    && !after.starts_with("->")
            });
            if used && !out.iter().any(|(q, m)| q == p && *m == model) {
                out.push((p.clone(), model.clone()));
            }
        }
    }
    out
}

/// The unchecked lookups in a controller. `src` is its source, `class` its
/// fully qualified name, `routes` the app's routes; `model` describes an
/// Eloquent model by its short name (`None`: not a model), and
/// `form_request_authorizes` says whether a FormRequest class's `authorize()`
/// does more than `return true`.
pub fn unchecked_lookups(
    src: &str,
    class: &str,
    routes: &[RouteInfo],
    model: &dyn Fn(&str) -> Option<ModelFacts>,
    form_request_authorizes: &dyn Fn(&str) -> bool,
) -> Vec<Finding> {
    // Only a model that belongs to someone, and that no global scope already
    // confines, can be handed to the wrong user.
    let owner_of = |name: &str| model(name).filter(|f| !f.scoped).and_then(|f| f.owner);
    let is_model = |name: &str| owner_of(name).is_some();
    // Authorization for the whole controller.
    if src.contains("authorizeResource(")
        || (src.contains("middleware") && src.contains("'can:"))
        || src.contains("#[Authorize")
    {
        return Vec::new();
    }
    let mut by_method: HashMap<&str, Vec<&RouteInfo>> = HashMap::new();
    for r in routes {
        let (c, m) = r.action.split_once('@').unwrap_or((&r.action, "__invoke"));
        if c.trim_start_matches('\\') == class {
            by_method.entry(m).or_default().push(r);
        }
    }
    let mut out = Vec::new();
    for m in methods(src) {
        let Some(rs) = by_method.get(m.name) else {
            continue;
        };
        // Only signed-in users can be handed someone else's record; a public
        // route is public on purpose (or already flagged as unprotected).
        let Some(route) = rs.iter().find(|r| {
            r.has_auth()
                && !authorizing_middleware(&r.middleware)
                && !route_params(&r.uri).is_empty()
        }) else {
            continue;
        };
        if rs.iter().any(|r| authorizing_middleware(&r.middleware)) {
            continue;
        }
        // A secret in the URL (an invitation token) is the check; a slug
        // names content that is published, not someone's record.
        let params: Vec<String> = route_params(&route.uri)
            .into_iter()
            .filter(|p| {
                let lower = p.to_ascii_lowercase();
                !["token", "hash", "signature", "secret", "slug"]
                    .iter()
                    .any(|w| lower.contains(w))
                    && !route.uri.contains(&format!("{{{p}:slug}}"))
            })
            .collect();
        if params.is_empty() {
            continue;
        }
        if CHECKS.iter().any(|c| m.body.contains(c)) || helper_check(m.body, &params) {
            continue;
        }
        let typed = typed_params(m.params);
        if typed.iter().any(|(ty, _)| {
            ty.ends_with("Request") && ty != "Request" && form_request_authorizes(ty)
        }) {
            continue;
        }
        let mut lookups: Vec<(String, String)> = typed
            .iter()
            .filter(|(ty, name)| params.contains(name) && is_model(ty))
            .map(|(ty, name)| (name.clone(), ty.clone()))
            .collect();
        for l in lookups_in_body(m.body, &params, &is_model) {
            if !lookups.contains(&l) {
                lookups.push(l);
            }
        }
        if lookups.is_empty() {
            continue;
        }
        let method = route.methods.split('|').next().unwrap_or("GET");
        let owner = owner_of(&lookups[0].1).unwrap_or_default();
        out.push(Finding {
            method: m.name.to_string(),
            line: m.line,
            end_line: m.end_line,
            lookups,
            owner,
            route: format!("{method} {}", route.uri),
        });
    }
    out
}

/// The models an action's route parameters become: `Order $order` in its
/// signature, or `Order::findOrFail($id)` in its body → `("order", "Order")`.
/// `src` is the controller's source; `is_model` says whether a short class
/// name is one of the app's models.
pub fn bound_models(
    src: &str,
    method: &str,
    params: &[String],
    is_model: &dyn Fn(&str) -> bool,
) -> Vec<(String, String)> {
    let Some(m) = methods(src).into_iter().find(|m| m.name == method) else {
        return Vec::new();
    };
    // `{order_item}` binds `$orderItem`.
    let norm = |s: &str| s.replace('_', "").to_ascii_lowercase();
    let mut out: Vec<(String, String)> = Vec::new();
    for (ty, name) in typed_params(m.params) {
        if let Some(p) = params.iter().find(|p| norm(p) == norm(&name)) {
            if is_model(&ty) {
                out.push((p.clone(), ty));
            }
        }
    }
    for l in lookups_in_body(m.body, params, is_model) {
        if !out.iter().any(|(p, _)| *p == l.0) {
            out.push(l);
        }
    }
    out
}

/// What the checks need to know about a project: its models (who owns them,
/// what confines them) and its FormRequests. Reads files as asked, once.
pub struct Project {
    root: std::path::PathBuf,
    app_files: Vec<std::path::PathBuf>,
    facts: std::cell::RefCell<HashMap<String, Option<ModelFacts>>>,
}

impl Project {
    /// Blocking: lists the files under `app/`.
    pub fn new(root: &std::path::Path) -> Project {
        let mut app_files = Vec::new();
        collect_php(&root.join("app"), &mut app_files);
        Project {
            root: root.to_path_buf(),
            app_files,
            facts: Default::default(),
        }
    }

    fn find(&self, stem: &str) -> Option<String> {
        self.app_files
            .iter()
            .find(|p| p.file_stem().is_some_and(|s| s == stem))
            .and_then(|p| std::fs::read_to_string(p).ok())
    }

    fn model(&self, name: &str) -> Option<ModelFacts> {
        if let Some(f) = self.facts.borrow().get(name) {
            return f.clone();
        }
        let trait_src = |t: &str| {
            self.find(t)
                .filter(|src| src.contains(&format!("trait {t}")))
        };
        let f = std::fs::read_to_string(self.root.join("app/Models").join(format!("{name}.php")))
            .ok()
            .map(|src| model_facts(&src, &trait_src))
            .map(|f| match name {
                // Someone's account is theirs by definition.
                "User" => ModelFacts {
                    owner: Some("a user account".into()),
                    ..f
                },
                _ => f,
            });
        self.facts.borrow_mut().insert(name.to_string(), f.clone());
        f
    }

    fn request_authorizes(&self, name: &str) -> bool {
        self.find(name)
            .filter(|src| src.contains("FormRequest") || src.contains("function authorize"))
            .is_some_and(|src| form_request_authorizes(&src))
    }

    /// The unchecked lookups in one controller's source.
    pub fn scan(&self, src: &str, routes: &[RouteInfo]) -> Vec<Finding> {
        let Some(class) = class_of(src) else {
            return Vec::new();
        };
        unchecked_lookups(src, &class, routes, &|m| self.model(m), &|r| {
            self.request_authorizes(r)
        })
    }
}

/// Every unchecked lookup in the project's controllers, by file.
/// Blocking: reads `app/Http/Controllers`, the models and the FormRequests.
pub fn scan_project(
    root: &std::path::Path,
    routes: &[RouteInfo],
) -> Vec<(std::path::PathBuf, Finding)> {
    let project = Project::new(root);
    let mut controllers = Vec::new();
    collect_php(&root.join("app/Http/Controllers"), &mut controllers);
    controllers.sort();
    let mut out = Vec::new();
    for file in controllers {
        let Ok(src) = std::fs::read_to_string(&file) else {
            continue;
        };
        for f in project.scan(&src, routes) {
            out.push((file.clone(), f));
        }
    }
    out
}

fn collect_php(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for e in read.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_php(&p, out);
        } else if p.extension().is_some_and(|x| x == "php") {
            out.push(p);
        }
    }
}

/// Whether a FormRequest's `authorize()` does more than `return true;` (an
/// absent `authorize()` allows everything, like `return true`).
pub fn form_request_authorizes(src: &str) -> bool {
    methods(src)
        .into_iter()
        .find(|m| m.name == "authorize")
        .is_some_and(|m| {
            let body: String = m.body.chars().filter(|c| !c.is_whitespace()).collect();
            body != "{returntrue;}"
        })
}

/// The fully qualified class a PHP file declares: `namespace` + `class`.
pub fn class_of(src: &str) -> Option<String> {
    let ns = src.lines().find_map(|l| {
        l.trim()
            .strip_prefix("namespace ")
            .map(|n| n.trim_end_matches(';').trim().to_string())
    });
    let class = src.lines().find_map(|l| {
        let t = l.trim();
        let t = ["final ", "abstract ", "readonly "]
            .iter()
            .fold(t, |t, m| t.strip_prefix(m).unwrap_or(t));
        t.strip_prefix("class ").map(|r| {
            r.chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect::<String>()
        })
    })?;
    Some(match ns {
        Some(ns) => format!("{ns}\\{class}"),
        None => class,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(methods: &str, uri: &str, action: &str, middleware: &str) -> RouteInfo {
        RouteInfo {
            name: String::new(),
            uri: uri.into(),
            methods: methods.into(),
            action: action.into(),
            middleware: middleware.into(),
        }
    }

    const CLASS: &str = "App\\Http\\Controllers\\OrderController";

    fn routes() -> Vec<RouteInfo> {
        let a = |m: &str| format!("{CLASS}@{m}");
        vec![
            route("GET|HEAD", "orders/{order}", &a("show"), "web,auth"),
            route("GET|HEAD", "orders/{id}/invoice", &a("invoice"), "web,auth"),
            route("PUT", "orders/{order}", &a("update"), "web,auth"),
            route(
                "DELETE",
                "orders/{order}",
                &a("destroy"),
                "web,auth,can:delete,order",
            ),
            route("GET|HEAD", "orders", &a("index"), "web,auth"),
            route("GET|HEAD", "shop/{order}", &a("public"), "web"),
            route("GET|HEAD", "mine/{order}", &a("mine"), "web,auth"),
            route("GET|HEAD", "checked/{order}", &a("checked"), "web,auth"),
            route("PATCH", "orders/{order}/note", &a("note"), "web,auth"),
        ]
    }

    const SRC: &str = r#"<?php

namespace App\Http\Controllers;

use App\Models\Order;

class OrderController extends Controller
{
    public function show(Order $order)
    {
        return view('orders.show', compact('order'));
    }

    public function invoice(int $id)
    {
        $order = Order::findOrFail($id);
        return $order->invoice;
    }

    public function update(UpdateOrderRequest $request, Order $order)
    {
        $order->update($request->validated());
    }

    public function destroy(Order $order)
    {
        $order->delete();
    }

    public function index()
    {
        return Order::all();
    }

    public function public(Order $order)
    {
        return $order;
    }

    public function mine(Request $request, $order)
    {
        return $request->user()->orders()->findOrFail($order);
    }

    public function checked(Order $order)
    {
        $this->authorize('view', $order); // "can't" in a comment {
        return $order;
    }

    public function note(OpenRequest $request, Order $order)
    {
        $order->note = $request->note;
    }
}
"#;

    fn findings(src: &str) -> Vec<Finding> {
        let order = |c: &str| {
            (c == "Order").then(|| ModelFacts {
                owner: Some("user_id".into()),
                scoped: false,
            })
        };
        unchecked_lookups(src, CLASS, &routes(), &order, &|r| {
            r == "UpdateOrderRequest"
        })
    }

    #[test]
    fn flags_unchecked_lookups_only() {
        let f = findings(SRC);
        let names: Vec<&str> = f.iter().map(|f| f.method.as_str()).collect();
        // update: its FormRequest authorizes; destroy: can: middleware;
        // index: no parameter; public: no auth; mine: scoped to the user;
        // checked: authorize().
        assert_eq!(names, ["show", "invoice", "note"]);
        assert_eq!(f[0].lookups, [("order".to_string(), "Order".to_string())]);
        assert_eq!(f[0].route, "GET orders/{order}");
        assert_eq!(f[0].line, 8);
        assert_eq!(f[1].lookups, [("id".to_string(), "Order".to_string())]);
        assert!(f[0]
            .message()
            .starts_with("GET orders/{order} hands any signed-in user the Order {order} they ask for, and it belongs to someone (user_id)"));
    }

    #[test]
    fn a_helper_that_checks_the_model_counts_as_a_check() {
        let src = SRC.replace(
            "return view('orders.show', compact('order'));",
            "$this->belongsToWorkspace($order);\n        return view('orders.show', compact('order'));",
        );
        assert!(!findings(&src).iter().any(|f| f.method == "show"));
        // A helper that doesn't get the model checks something else.
        let src = SRC.replace(
            "return view('orders.show', compact('order'));",
            "$this->requireFeature();\n        return view('orders.show', compact('order'));",
        );
        assert!(findings(&src).iter().any(|f| f.method == "show"));
    }

    #[test]
    fn models_say_who_owns_them_and_whether_a_scope_confines_them() {
        let none = |_: &str| None;
        let owned = model_facts(
            "class Order extends Model {\n    protected $fillable = ['user_id', 'total'];\n}",
            &none,
        );
        assert_eq!(owned.owner.as_deref(), Some("user_id"));
        assert!(!owned.scoped);
        let team = model_facts(
            "class Task extends Model {\n    use HasFactory;\n    public function team() { return $this->belongsTo(Team::class); }\n}",
            &none,
        );
        assert_eq!(team.owner.as_deref(), Some("team"));
        assert!(
            model_facts("class Page extends Model {\n    use HasFactory;\n}", &none)
                .owner
                .is_none()
        );
        // The other table's key isn't this one's owner.
        assert!(model_facts(
            "class Team extends Model {\n    public function members() { return $this->hasMany(Member::class, 'user_id'); }\n}",
            &none
        )
        .owner
        .is_none());
        // Who made a row isn't whose it is.
        assert!(model_facts(
            "class Contact extends Model {\n    public function creator() { return $this->belongsTo(User::class, 'created_by'); }\n}",
            &none
        )
        .owner
        .is_none());
        assert_eq!(
            model_facts(
                "class Doc extends Model {\n    public function owner() { return $this->belongsTo(User::class, 'owner_id'); }\n}",
                &none
            )
            .owner
            .as_deref(),
            Some("owner_id")
        );
        // A trait of the app's that adds a global scope.
        let scoped = model_facts(
            "class Task extends Model {\n    use BelongsToCrew, HasFactory;\n    protected $fillable = ['crew_id', 'user_id'];\n}",
            &|t| (t == "BelongsToCrew").then(|| "trait BelongsToCrew { static::addGlobalScope('crew', fn () => 1); }".to_string()),
        );
        assert!(scoped.scoped);
        assert!(model_facts(
            "class X extends Model {\n    use BelongsToTenant;\n    protected $fillable = ['user_id'];\n}",
            &none
        )
        .scoped);
    }

    #[test]
    fn a_controller_authorized_as_a_whole_is_left_alone() {
        let src = SRC.replace(
            "{\n    public function show",
            "{\n    public function __construct()\n    {\n        $this->authorizeResource(Order::class);\n    }\n\n    public function show",
        );
        assert!(findings(&src).is_empty());
    }

    #[test]
    fn reads_route_params_and_form_requests() {
        assert_eq!(
            route_params("posts/{post:slug}/comments/{comment?}"),
            ["post", "comment"]
        );
        assert!(route_params("orders").is_empty());
        assert!(!form_request_authorizes(
            "class A extends FormRequest {\n public function authorize(): bool\n {\n  return true;\n }\n}"
        ));
        assert!(form_request_authorizes(
            "class A extends FormRequest {\n public function authorize(): bool\n {\n  return $this->user()->can('update', $this->order);\n }\n}"
        ));
        assert!(!form_request_authorizes(
            "class A extends FormRequest { public function rules() { return []; } }"
        ));
        assert_eq!(
            class_of("<?php\nnamespace App\\Http\\Controllers;\n\nfinal class OrderController extends Controller\n{"),
            Some("App\\Http\\Controllers\\OrderController".to_string())
        );
    }
}
