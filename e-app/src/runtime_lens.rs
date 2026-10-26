//! Runtime lenses: what the running app just did, at the end of the line of
//! the controller method that did it —
//! `⚡ 42 ms · 14 queries · N+1 orders ×12 · 2m ago`.
//!
//! The requests come from the Runtime panel's capture (Grove's timeline, or
//! Clockwork). Each is matched to the route the router would pick, the route
//! to its action (`OrderController@index`), the action to its file; the
//! newest request per action is the one shown. ⌘. on the line opens it in the
//! Runtime panel, or hands it to the agent to fix and measure.
//!
//! Matching and labelling are pure and tested here; the polling, the
//! per-buffer placement and the code actions are in the `impl AppState`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use e_laravel::laravel::{route_for, RouteInfo};
use e_verify::metrics_of;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use floem::views::editor::text::Document;

use crate::runtime::RuntimeReq;
use crate::state::AppState;

/// A request slower than this is worth a warning colour.
const SLOW_MS: f64 = 1000.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LensLevel {
    Info,
    /// An N+1, or slow.
    Warn,
    /// A server error.
    Error,
}

/// Where in a file a lens goes: a method's `function` line, or — for an
/// action that is a class (an invokable controller, a Livewire page) — its
/// `class` line.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum LensAnchor {
    Method(String),
    Class(String),
}

impl LensAnchor {
    /// The anchor a line of PHP declares, if any.
    pub fn of_line(line: &str) -> Option<LensAnchor> {
        let ident = |s: &str| -> String {
            s.chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect()
        };
        let t = line.trim_start();
        if let Some(at) = t.find("function ") {
            // `public function index(`, `public static function make(` — not
            // a closure (`function (`) or a call that merely mentions one.
            let before = &t[..at];
            if before.split_whitespace().all(|w| {
                matches!(
                    w,
                    "public" | "protected" | "private" | "static" | "final" | "abstract"
                )
            }) {
                let name = ident(t[at + 9..].trim_start());
                return (!name.is_empty()).then_some(LensAnchor::Method(name));
            }
        }
        let rest = ["final ", "abstract ", "readonly "]
            .iter()
            .fold(t, |t, m| t.strip_prefix(m).unwrap_or(t));
        let name = ident(rest.strip_prefix("class ")?);
        (!name.is_empty()).then_some(LensAnchor::Class(name))
    }
}

/// One lens: the file and anchor it belongs at, and what it says.
#[derive(Clone, Debug, PartialEq)]
pub struct Lens {
    pub file: PathBuf,
    pub anchor: LensAnchor,
    pub label: String,
    pub level: LensLevel,
    /// The Runtime panel's id for the request shown.
    pub req_id: String,
}

/// The file and anchor of a route action, if it is a class of the app's:
/// `App\Http\Controllers\OrderController@index` →
/// `app/Http/Controllers/OrderController.php`, method `index`.
pub fn action_target(root: &Path, action: &str) -> Option<(PathBuf, LensAnchor)> {
    let (class, method) = match action.split_once('@') {
        Some((c, m)) => (c, Some(m)),
        None => (action, None),
    };
    let rel = class.trim_start_matches('\\').strip_prefix("App\\")?;
    let file = root
        .join("app")
        .join(format!("{}.php", rel.replace('\\', "/")));
    let anchor = match method {
        Some(m) => LensAnchor::Method(m.to_string()),
        None => LensAnchor::Class(rel.rsplit('\\').next().unwrap_or(rel).to_string()),
    };
    Some((file, anchor))
}

/// "just now", "45s ago", "2m ago", "3h ago".
pub fn age(ms: u128) -> String {
    let s = ms / 1000;
    match s {
        0..=4 => "just now".into(),
        5..=59 => format!("{s}s ago"),
        60..=3599 => format!("{}m ago", s / 60),
        _ => format!("{}h ago", s / 3600),
    }
}

/// The table an N+1's query shape reads from (`select * from \`orders\` where
/// …` → `orders`).
fn table_of(skeleton: &str) -> Option<String> {
    let at = skeleton.find(" from ")? + 6;
    let name: String = skeleton[at..]
        .trim_start()
        .chars()
        .skip_while(|c| *c == '`' || *c == '"')
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '.')
        .collect();
    (!name.is_empty()).then_some(name)
}

/// What a request says on its lens. `queries_visible` is false when nobody
/// could see its SQL (no Clockwork, Grove's capture off or not loaded yet):
/// then the query part is left out rather than shown as zero.
pub fn label(
    req: &RuntimeReq,
    queries_visible: bool,
    hits: usize,
    now_ms: u128,
) -> (String, LensLevel) {
    let mut parts = vec![format!("{:.0} ms", req.duration_ms)];
    if req.status >= 400 {
        parts.insert(0, req.status.to_string());
    }
    let mut level = if req.status >= 500 {
        LensLevel::Error
    } else if req.duration_ms >= SLOW_MS {
        LensLevel::Warn
    } else {
        LensLevel::Info
    };
    if queries_visible {
        let sample = crate::verify::sample_from_replay(req.status, req.duration_ms, &req.queries);
        let m = metrics_of(&sample);
        parts.push(format!(
            "{} quer{}",
            m.query_count,
            if m.query_count == 1 { "y" } else { "ies" }
        ));
        if let Some(g) = m.worst_n_plus_one() {
            let on = table_of(&g.skeleton)
                .map(|t| format!(" {t}"))
                .unwrap_or_default();
            parts.push(format!("N+1{on} ×{}", g.count));
            if level == LensLevel::Info {
                level = LensLevel::Warn;
            }
        }
    }
    if hits > 1 {
        parts.push(format!("{hits} requests"));
    }
    parts.push(age(now_ms.saturating_sub(req.at_ms)));
    (format!("⚡ {}", parts.join(" · ")), level)
}

/// The lenses for captured requests (newest first): one per action, from its
/// newest request. `visible` says whether a request's queries can be seen.
pub fn lenses(
    root: &Path,
    routes: &[RouteInfo],
    reqs: &[RuntimeReq],
    visible: impl Fn(&RuntimeReq) -> bool,
    now_ms: u128,
) -> Vec<Lens> {
    // action -> (newest request, how many)
    let mut by_action: HashMap<&str, (&RuntimeReq, usize)> = HashMap::new();
    let mut order: Vec<&str> = Vec::new();
    for r in reqs {
        let Some(route) = route_for(routes, &r.method, &r.uri) else {
            continue;
        };
        let entry = by_action.entry(route.action.as_str()).or_insert_with(|| {
            order.push(route.action.as_str());
            (r, 0)
        });
        entry.1 += 1;
    }
    order
        .into_iter()
        .filter_map(|action| {
            let (req, hits) = by_action[action];
            let (file, anchor) = action_target(root, action)?;
            let (label, level) = label(req, visible(req), hits, now_ms);
            Some(Lens {
                file,
                anchor,
                label,
                level,
                req_id: req.id.clone(),
            })
        })
        .collect()
}

impl AppState {
    /// Recompute the lenses from the captured requests and repaint the
    /// buffers whose lenses changed. Cheap: the requests are in memory and
    /// the routes are loaded; only matching happens here.
    pub fn refresh_runtime_lenses(&self) {
        let fresh: Vec<Lens> = if self.settings.get_untracked().runtime_lenses {
            let routes = self
                .laravel
                .get_untracked()
                .map(|d| d.all_routes.clone())
                .unwrap_or_default();
            let sql_capture = self.grove_sql_capture.get_untracked();
            let root = self.root.get_untracked();
            self.runtime_reqs.with_untracked(|reqs| {
                lenses(
                    &root,
                    &routes,
                    reqs,
                    |r| r.chain_loaded && (r.grove_id.is_none() || sql_capture == Some(true)),
                    crate::state::now_ms(),
                )
            })
        } else {
            Vec::new()
        };
        let old = self.runtime_lenses.get_untracked();
        if *old == fresh {
            return;
        }
        // Repaint the buffers a lens left or arrived in.
        let touched: Vec<PathBuf> = old
            .iter()
            .chain(fresh.iter())
            .map(|l| l.file.clone())
            .collect();
        self.runtime_lenses.set(Arc::new(fresh));
        self.buffers.with_untracked(|bs| {
            for b in bs {
                if b.file.path.as_ref().is_some_and(|p| touched.contains(p)) {
                    b.doc.cache_rev().update(|r| *r += 1);
                }
            }
        });
    }

    /// The lens on `line` of the active buffer, if there is one.
    pub fn runtime_lens_at(&self, path: &Path, line_text: &str) -> Option<Lens> {
        let anchor = LensAnchor::of_line(line_text)?;
        self.runtime_lenses.with_untracked(|ls| {
            ls.iter()
                .find(|l| l.file == path && l.anchor == anchor)
                .cloned()
        })
    }

    /// Keep the lenses' data coming while the Runtime panel is closed: with
    /// Grove serving the project and a file under `app/` open, poll its
    /// timeline. Called on the idle tick.
    pub fn poll_runtime_for_lenses(&self) {
        if !self.settings.get_untracked().runtime_lenses
            || self.runtime_open.get_untracked()
            || self.grove_site().is_none()
        {
            return;
        }
        let app = self.root.get_untracked().join("app");
        let wanted = self.buffers.with_untracked(|bs| {
            bs.iter()
                .any(|b| b.file.path.as_ref().is_some_and(|p| p.starts_with(&app)))
        });
        if wanted {
            self.poll_runtime();
        }
    }

    /// Open the lens's request in the Runtime panel, expanded.
    pub fn runtime_lens_open(&self, req_id: &str) {
        if !self.runtime_open.get_untracked() {
            self.toggle_runtime();
        }
        self.runtime_expanded.set(Some(req_id.to_string()));
        self.runtime_load_chain(req_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(
        id: &str,
        method: &str,
        uri: &str,
        ms: f64,
        queries: &[&str],
        at_ms: u128,
    ) -> RuntimeReq {
        RuntimeReq {
            id: id.into(),
            method: method.into(),
            uri: uri.into(),
            status: 200,
            duration_ms: ms,
            queries: queries
                .iter()
                .map(|q| (q.to_string(), String::new()))
                .collect(),
            cache_hits: 0,
            cache_misses: 0,
            mails: Vec::new(),
            events: 0,
            grove_id: None,
            logs: Vec::new(),
            chain_loaded: true,
            at_ms,
        }
    }

    fn route(methods: &str, uri: &str, action: &str) -> RouteInfo {
        RouteInfo {
            name: String::new(),
            uri: uri.into(),
            methods: methods.into(),
            action: action.into(),
            middleware: String::new(),
        }
    }

    #[test]
    fn lines_name_their_anchor() {
        let m = |s: &str| LensAnchor::Method(s.into());
        assert_eq!(
            LensAnchor::of_line("    public function index(Request $r)"),
            Some(m("index"))
        );
        assert_eq!(
            LensAnchor::of_line("    public static function make()"),
            Some(m("make"))
        );
        assert_eq!(
            LensAnchor::of_line("function helper() {"),
            Some(m("helper"))
        );
        assert_eq!(
            LensAnchor::of_line("final class OrderController extends Controller"),
            Some(LensAnchor::Class("OrderController".into()))
        );
        assert_eq!(
            LensAnchor::of_line("class Show extends Component"),
            Some(LensAnchor::Class("Show".into()))
        );
        assert_eq!(LensAnchor::of_line("    $fn = function ($x) {"), None);
        assert_eq!(
            LensAnchor::of_line("    return array_map(function ($x) {"),
            None
        );
        assert_eq!(LensAnchor::of_line("    // the class is"), None);
    }

    #[test]
    fn actions_point_at_app_classes() {
        let root = Path::new("/p");
        assert_eq!(
            action_target(root, "App\\Http\\Controllers\\OrderController@index"),
            Some((
                PathBuf::from("/p/app/Http/Controllers/OrderController.php"),
                LensAnchor::Method("index".into())
            ))
        );
        assert_eq!(
            action_target(root, "App\\Livewire\\Pages\\Show"),
            Some((
                PathBuf::from("/p/app/Livewire/Pages/Show.php"),
                LensAnchor::Class("Show".into())
            ))
        );
        assert_eq!(action_target(root, "Closure"), None);
        assert_eq!(
            action_target(root, "Illuminate\\Routing\\ViewController"),
            None
        );
    }

    #[test]
    fn a_label_says_time_queries_n_plus_one_and_age() {
        let n1 = [
            "select * from `users`",
            "select * from `orders` where `user_id` = 1",
            "select * from `orders` where `user_id` = 2",
            "select * from `orders` where `user_id` = 3",
        ];
        let r = req("g1", "GET", "/orders", 42.4, &n1, 1_000);
        let (text, level) = label(&r, true, 1, 121_000);
        assert_eq!(text, "⚡ 42 ms · 4 queries · N+1 orders ×3 · 2m ago");
        assert_eq!(level, LensLevel::Warn);

        // Unseen queries are left out, not shown as zero.
        let (text, level) = label(&r, false, 3, 3_000);
        assert_eq!(text, "⚡ 42 ms · 3 requests · just now");
        assert_eq!(level, LensLevel::Info);

        let mut err = req("g2", "GET", "/x", 12.0, &[], 0);
        err.status = 500;
        let (text, level) = label(&err, true, 1, 0);
        assert_eq!(text, "⚡ 500 · 12 ms · 0 queries · just now");
        assert_eq!(level, LensLevel::Error);
    }

    #[test]
    fn one_lens_per_action_from_its_newest_request() {
        let routes = [
            route(
                "GET|HEAD",
                "orders",
                "App\\Http\\Controllers\\OrderController@index",
            ),
            route(
                "GET|HEAD",
                "orders/{order}",
                "App\\Http\\Controllers\\OrderController@show",
            ),
            route("GET|HEAD", "/", "Illuminate\\Routing\\ViewController"),
        ];
        // Newest first, as the Runtime panel keeps them.
        let reqs = [
            req("g3", "GET", "/orders", 30.0, &[], 3_000),
            req("g2", "GET", "/orders/7", 80.0, &[], 2_000),
            req("g1", "GET", "/orders", 90.0, &[], 1_000),
            req("g0", "GET", "/", 5.0, &[], 500),
            req("gx", "GET", "/nowhere", 5.0, &[], 400),
        ];
        let ls = lenses(Path::new("/p"), &routes, &reqs, |_| false, 3_000);
        assert_eq!(ls.len(), 2);
        assert_eq!(ls[0].anchor, LensAnchor::Method("index".into()));
        assert_eq!(ls[0].req_id, "g3");
        assert_eq!(ls[0].label, "⚡ 30 ms · 2 requests · just now");
        assert_eq!(ls[1].anchor, LensAnchor::Method("show".into()));
        assert_eq!(ls[1].req_id, "g2");
    }

    #[test]
    fn ages_read_like_a_person_would_say_them() {
        assert_eq!(age(1_000), "just now");
        assert_eq!(age(45_000), "45s ago");
        assert_eq!(age(150_000), "2m ago");
        assert_eq!(age(7_300_000), "2h ago");
    }
}
