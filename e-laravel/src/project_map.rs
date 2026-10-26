//! The whole project in one answer, for an agent: routes, models with their
//! live columns and relationships, Livewire components, Inertia pages and what
//! renders them, views and Blade components.
//!
//! An agent finds all of this out by grepping — routes files, models,
//! migrations, controllers — at the cost of many tool calls and many thousands
//! of tokens, and still guesses at the columns. The editor already knows it.
//! Paths are relative to the project root and columns are `name:type`
//! (`?` when nullable), to keep the answer small.

use std::collections::HashMap;
use std::path::Path;

use e_db::ColumnInfo;
use serde_json::{json, Map, Value};

use crate::laravel::LaravelData;

/// The sections [`build`] can include; all of them when none are asked for.
pub const SECTIONS: &[&str] = &[
    "routes",
    "models",
    "livewire",
    "inertia",
    "views",
    "components",
];

/// What to put in the map.
#[derive(Clone, Debug, Default)]
pub struct Request {
    /// Sections to include; empty for all.
    pub include: Vec<String>,
    /// Keep only entries whose name, URI, class, table or file contains this
    /// (case-insensitive); empty for everything.
    pub filter: String,
}

impl Request {
    fn wants(&self, section: &str) -> bool {
        self.include.is_empty() || self.include.iter().any(|s| s == section)
    }
}

/// Build the map. `schema` is the live database's columns by table (empty
/// without a database: models then come without columns). Blocking — it
/// reads the project's PHP and page files.
pub fn build(
    root: &Path,
    data: &LaravelData,
    schema: &HashMap<String, Vec<ColumnInfo>>,
    req: &Request,
) -> Value {
    let needle = req.filter.to_lowercase();
    let hit = |fields: &[&str]| {
        needle.is_empty() || fields.iter().any(|f| f.to_lowercase().contains(&needle))
    };
    let rel = |p: &Path| {
        p.strip_prefix(root)
            .unwrap_or(p)
            .to_string_lossy()
            .into_owned()
    };
    let mut out = Map::new();

    if req.wants("routes") {
        let (package, app): (Vec<_>, Vec<_>) = data.all_routes.iter().partition(|r| r.is_package());
        let routes: Vec<Value> = app
            .into_iter()
            .filter(|r| hit(&[&r.uri, &r.name, &r.action]))
            .map(|r| {
                let mut o = json!({
                    "method": r.methods,
                    "uri": r.uri,
                    "action": r.action.strip_prefix("App\\Http\\Controllers\\").unwrap_or(&r.action),
                    "middleware": short_middleware(&r.middleware),
                });
                if !r.name.is_empty() {
                    o["name"] = json!(r.name);
                }
                if r.is_unprotected() {
                    o["unprotected"] = json!(true);
                }
                o
            })
            .collect();
        out.insert("routes".into(), json!(routes));
        if !package.is_empty() {
            out.insert("package_routes".into(), json!(package.len()));
        }
    }

    if req.wants("models") {
        let models: Vec<Value> = crate::relations::models(root, &[])
            .into_iter()
            .filter(|m| hit(&[&m.name, &m.table]))
            .map(|m| {
                let mut o = json!({
                    "class": m.name,
                    "table": m.table,
                    "file": rel(&m.file),
                });
                if let Some(cols) = schema.get(&m.table) {
                    o["columns"] = json!(cols.iter().map(column).collect::<Vec<_>>());
                }
                if !m.relations.is_empty() {
                    o["relations"] = json!(m
                        .relations
                        .iter()
                        .map(|r| format!("{}: {} {}", r.method, r.kind, r.target)
                            .trim_end()
                            .to_string())
                        .collect::<Vec<_>>());
                }
                o
            })
            .collect();
        out.insert("models".into(), json!(models));
        if schema.is_empty() {
            out.insert(
                "note".into(),
                json!("no database schema: models are listed without columns"),
            );
        }
    }

    if req.wants("livewire") {
        let components: Vec<Value> = crate::livewire::components(root)
            .into_iter()
            .filter(|(name, file)| hit(&[name, &rel(file)]))
            .map(|(name, file)| {
                let class_file = crate::livewire::resolve(root, &file)
                    .map(|c| c.class_file)
                    .unwrap_or_else(|| file.clone());
                let src = std::fs::read_to_string(&class_file).unwrap_or_default();
                let class = crate::livewire::class_part(&src);
                json!({
                    "name": name,
                    "file": rel(&file),
                    "properties": crate::livewire::properties(class),
                    "actions": crate::livewire::actions(class),
                })
            })
            .collect();
        out.insert("livewire".into(), json!(components));
    }

    if req.wants("inertia") {
        let pages: Vec<Value> = crate::contract::page_renders(root)
            .into_iter()
            .filter(|p| hit(&[&p.page]))
            .map(|p| {
                let mut o = json!({ "page": p.page });
                if let Some(f) = crate::inertia::resolve_page(root, &p.page) {
                    o["file"] = json!(rel(&f));
                }
                if let Some(c) = &p.controller {
                    o["controller"] = json!(rel(c));
                    o["props"] = json!(p.props);
                }
                o
            })
            .collect();
        out.insert("inertia".into(), json!(pages));
    }

    if req.wants("views") {
        let views: Vec<&str> = data
            .views
            .iter()
            .map(|v| v.name.as_str())
            .filter(|n| hit(&[n]))
            .collect();
        out.insert("views".into(), json!(views));
    }

    if req.wants("components") {
        let components: Vec<&str> = data
            .components
            .iter()
            .map(String::as_str)
            .filter(|n| hit(&[n]))
            .collect();
        out.insert("components".into(), json!(components));
    }

    Value::Object(out)
}

/// A middleware stack with class names cut to the class
/// (`Illuminate\Auth\Middleware\Authenticate` → `Authenticate`), keeping
/// any parameters (`…\ThrottleRequests:api` → `ThrottleRequests:api`).
fn short_middleware(stack: &str) -> String {
    stack
        .split(',')
        .map(|m| {
            let (name, params) = m.split_once(':').map_or((m, None), |(n, p)| (n, Some(p)));
            let name = name.rsplit('\\').next().unwrap_or(name);
            match params {
                Some(p) => format!("{name}:{p}"),
                None => name.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// `name:type`, with `?` for a nullable column.
fn column(c: &ColumnInfo) -> String {
    let q = if c.nullable { "?" } else { "" };
    format!("{}:{}{q}", c.name, c.data_type)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::laravel::RouteInfo;

    fn project() -> std::path::PathBuf {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!("e-project-map-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let write = |rel: &str, src: &str| {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, src).unwrap();
        };
        write(
            "app/Models/Order.php",
            "<?php\nnamespace App\\Models;\nclass Order extends Model {\n    public function user() { return $this->belongsTo(User::class); }\n}\n",
        );
        write(
            "app/Models/User.php",
            "<?php\nnamespace App\\Models;\nclass User extends Model {}\n",
        );
        write(
            "app/Livewire/OrderTable.php",
            "<?php\nclass OrderTable extends Component {\n    public $search = '';\n    public function delete($id) {}\n    public function render() {}\n}\n",
        );
        write(
            "app/Http/Controllers/OrderController.php",
            "<?php\nclass OrderController {\n    public function index() { return Inertia::render('Orders/Index', ['orders' => Order::all()]); }\n}\n",
        );
        write(
            "resources/js/Pages/Orders/Index.vue",
            "<template></template>\n",
        );
        write("resources/js/Pages/Welcome.vue", "<template></template>\n");
        root
    }

    fn data(root: &Path) -> LaravelData {
        let route = |methods: &str, uri: &str, name: &str, mw: &str| RouteInfo {
            name: name.into(),
            uri: uri.into(),
            methods: methods.into(),
            action: "App\\Http\\Controllers\\OrderController@index".into(),
            middleware: mw.into(),
        };
        LaravelData {
            root: root.to_path_buf(),
            routes: Vec::new(),
            all_routes: vec![
                route("GET|HEAD", "orders", "orders.index", "web,auth"),
                route("POST", "webhooks", "", "api"),
                RouteInfo {
                    action: "Flux\\AssetManager@fluxJs".into(),
                    ..route("GET|HEAD", "flux/flux.js", "", "")
                },
            ],
            views: Vec::new(),
            configs: Vec::new(),
            envs: Vec::new(),
            translations: Vec::new(),
            components: vec!["alert".into()],
        }
    }

    #[test]
    fn middleware_classes_are_cut_to_their_name() {
        assert_eq!(
            short_middleware("web,Illuminate\\Auth\\Middleware\\Authenticate,can:update,Illuminate\\Routing\\Middleware\\ThrottleRequests:api,App\\Http\\Middleware\\EnsureUserIsAdmin"),
            "web,Authenticate,can:update,ThrottleRequests:api,EnsureUserIsAdmin"
        );
    }

    fn col(name: &str, ty: &str, nullable: bool) -> ColumnInfo {
        ColumnInfo {
            name: name.into(),
            data_type: ty.into(),
            nullable,
            key: String::new(),
        }
    }

    #[test]
    fn maps_routes_models_components_and_pages() {
        let root = project();
        let schema = HashMap::from([(
            "orders".to_string(),
            vec![
                col("id", "bigint", false),
                col("paid_at", "timestamp", true),
            ],
        )]);
        let map = build(&root, &data(&root), &schema, &Request::default());

        let routes = map["routes"].as_array().unwrap();
        assert_eq!(routes.len(), 2);
        assert_eq!(map["package_routes"], 1);
        assert_eq!(routes[0]["name"], "orders.index");
        assert_eq!(routes[0]["action"], "OrderController@index");
        assert!(routes[0].get("unprotected").is_none());
        assert!(routes[1].get("name").is_none());
        assert_eq!(routes[1]["unprotected"], true);

        let models = map["models"].as_array().unwrap();
        let order = models.iter().find(|m| m["class"] == "Order").unwrap();
        assert_eq!(order["file"], "app/Models/Order.php");
        assert_eq!(order["columns"], json!(["id:bigint", "paid_at:timestamp?"]));
        assert_eq!(order["relations"], json!(["user: belongsTo User"]));
        // A model without relations is still a model; without a table in the
        // schema it just has no columns.
        let user = models.iter().find(|m| m["class"] == "User").unwrap();
        assert!(user.get("columns").is_none());
        assert!(map.get("note").is_none());

        let lw = &map["livewire"][0];
        assert_eq!(lw["name"], "order-table");
        assert_eq!(lw["properties"], json!(["search"]));
        assert_eq!(lw["actions"], json!(["delete"]));

        let pages = map["inertia"].as_array().unwrap();
        let index = pages.iter().find(|p| p["page"] == "Orders/Index").unwrap();
        assert_eq!(index["file"], "resources/js/Pages/Orders/Index.vue");
        assert_eq!(
            index["controller"],
            "app/Http/Controllers/OrderController.php"
        );
        assert_eq!(index["props"], json!(["orders"]));
        // A page nothing renders says so by having no controller.
        let welcome = pages.iter().find(|p| p["page"] == "Welcome").unwrap();
        assert!(welcome.get("controller").is_none());

        assert_eq!(map["components"], json!(["alert"]));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn include_and_filter_narrow_the_map() {
        let root = project();
        let req = Request {
            include: vec!["routes".into(), "models".into()],
            filter: "ORDER".into(),
        };
        let map = build(&root, &data(&root), &HashMap::new(), &req);
        let keys: Vec<&String> = map.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["models", "note", "package_routes", "routes"]);
        // Both routes go to OrderController, so both match on the action.
        assert_eq!(map["routes"].as_array().unwrap().len(), 2);
        let models: Vec<&str> = map["models"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["class"].as_str().unwrap())
            .collect();
        assert_eq!(models, ["Order"]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
