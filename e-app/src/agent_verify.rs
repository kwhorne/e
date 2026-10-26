//! `verify_route` on the agent socket: an agent measures a route itself —
//! status, time, queries, N+1 — before it starts and again when it thinks it
//! is done, and gets the verdict from the same `e-verify` comparison the
//! verify panel and the ship gate use. "Fixed the N+1" becomes something the
//! agent has measured rather than something it claims.
//!
//! The first measurement of a URL is its baseline; every later one is
//! compared with it, until `reset` makes the new one the baseline. Nothing is
//! stashed or checkpointed: the agent is editing the tree, and moving its work
//! out from under it would be worse than no baseline at all.

use std::collections::HashMap;
use std::sync::Mutex;

use e_verify::{compare, metrics_of, Comparison, RequestMetrics, N1_THRESHOLD};
use floem::ext_event::create_ext_action;
use floem::reactive::SignalGet;
use serde_json::{json, Value};

use crate::state::AppState;

/// Requests per measurement. One request's time is mostly noise; the median of
/// three, after a warm-up the result of which is thrown away, is steadier.
const RUNS: usize = 3;

/// A baseline: its measurement, and whether its queries were visible.
type Baseline = (RequestMetrics, bool);

/// Baselines by URL, for this editor process.
static BASELINES: Mutex<Option<HashMap<String, Baseline>>> = Mutex::new(None);

/// What to replay, from the request: `uri` (a path, or a URL on the app) or
/// `route` (a route name). Only routes that read — replaying a POST to see how
/// fast it is would change data — and only with every parameter filled in.
fn target(
    req: &Value,
    routes: &[e_laravel::laravel::RouteInfo],
    base: &str,
) -> Result<String, String> {
    let uri = if let Some(name) = req.get("route").and_then(Value::as_str) {
        let route = routes
            .iter()
            .find(|r| r.name == name)
            .ok_or_else(|| format!("no route named {name}"))?;
        if route.is_write() {
            return Err(format!(
                "{name} is {}: only routes that read are replayed",
                route.methods
            ));
        }
        route.uri.clone()
    } else {
        req.get("uri")
            .and_then(Value::as_str)
            .ok_or("missing uri (or route)")?
            .to_string()
    };
    if uri.contains('{') {
        return Err(format!(
            "{uri} has parameters: pass the uri with values, e.g. /orders/1"
        ));
    }
    if uri.contains("://") && !uri.starts_with(&format!("{base}/")) && uri != base {
        return Err(format!("{uri} is not on the app ({base})"));
    }
    Ok(crate::verify::replay_url(base, &uri))
}

/// The run with the median time, as the measurement of a route.
fn median(mut runs: Vec<RequestMetrics>) -> Option<RequestMetrics> {
    runs.sort_by(|a, b| a.ms.total_cmp(&b.ms));
    let mid = runs.len() / 2;
    runs.into_iter().nth(mid)
}

/// Record `m` as the baseline for `url` if it has none (or `reset`), and
/// return the baseline it should be compared with otherwise.
fn baseline_for(url: &str, m: &RequestMetrics, visible: bool, reset: bool) -> Option<Baseline> {
    let mut guard = BASELINES.lock().unwrap_or_else(|e| e.into_inner());
    let map = guard.get_or_insert_with(HashMap::new);
    match map.get(url) {
        Some(before) if !reset => Some(before.clone()),
        _ => {
            map.insert(url.to_string(), (m.clone(), visible));
            None
        }
    }
}

fn metrics_json(m: &RequestMetrics, queries_visible: bool) -> Value {
    let mut o = json!({
        "status": m.status,
        "ms": m.ms.round(),
    });
    if queries_visible {
        o["queries"] = json!(m.query_count);
        o["slowest_query_ms"] = json!(m.slowest_query_ms);
        o["n_plus_one"] = json!(m
            .groups
            .iter()
            .filter(|g| g.count >= N1_THRESHOLD)
            .map(|g| json!({"sql": g.skeleton, "count": g.count}))
            .collect::<Vec<_>>());
    }
    o
}

/// The comparison, with the query side left out when either measurement
/// couldn't see queries: zero against zero would read as "no change".
fn comparison_json(c: &Comparison, queries_visible: bool) -> Value {
    let mut o = json!({
        "verdict": crate::verify::verdict_label(c.verdict),
        "ms_before": c.ms_before.round(),
        "ms_after": c.ms_after.round(),
        "status_changed": c.status_changed,
    });
    if queries_visible {
        o["summary"] = json!(crate::verify::summary(c));
        o["queries_before"] = json!(c.queries_before);
        o["queries_after"] = json!(c.queries_after);
        o["n_plus_one_fixed"] = json!(c.n1_fixed);
        o["n_plus_one_after"] = json!(c.n1_after);
    } else {
        o["summary"] = json!(format!("{:.0}ms → {:.0}ms", c.ms_before, c.ms_after));
    }
    o
}

/// The answer for a measurement of `url`, given its baseline (if any).
fn answer(
    url: &str,
    m: &RequestMetrics,
    queries_visible: bool,
    before: Option<&Baseline>,
) -> Value {
    let mut o = json!({
        "ok": true,
        "url": url,
        "metrics": metrics_json(m, queries_visible),
    });
    match before {
        None => o["baseline"] = json!(true),
        Some((b, before_visible)) => {
            // Without query data on both sides, a comparison of counts would
            // read zero against zero as "no change": time and status only.
            let both = queries_visible && *before_visible;
            let c = if both {
                compare(b, m)
            } else {
                compare(
                    &RequestMetrics {
                        query_count: 0,
                        groups: Vec::new(),
                        ..b.clone()
                    },
                    &RequestMetrics {
                        query_count: 0,
                        groups: Vec::new(),
                        ..m.clone()
                    },
                )
            };
            o["comparison"] = comparison_json(&c, both);
        }
    }
    if !queries_visible {
        o["note"] = json!(
            "queries not visible: the app has no Clockwork, and Grove's SQL capture is off \
             or saw none of the app's statements, so only status and time were measured"
        );
    }
    o
}

impl AppState {
    /// Handle `verify_route`: measure, compare with the baseline, answer.
    pub fn agent_verify_route(&self, req: &Value, reply: std::sync::mpsc::Sender<Value>) {
        let base = self.app_base();
        let routes = self
            .laravel
            .get_untracked()
            .map(|d| d.all_routes.clone())
            .unwrap_or_default();
        let url = match target(req, &routes, &base) {
            Ok(u) => u,
            Err(e) => {
                let _ = reply.send(json!({"ok": false, "error": e}));
                return;
            }
        };
        let reset = req.get("reset").and_then(Value::as_bool).unwrap_or(false);
        let state = *self;
        let log = create_ext_action(self.cx, move |line: String| {
            state.agent_log_push("verify_route", line);
        });
        std::thread::spawn(move || {
            // The agent has usually just written the code: let PHP's opcache
            // see it, or this measures the code from before.
            std::thread::sleep(crate::evidence::SETTLE);
            let mut visible = true;
            let mut runs = Vec::new();
            for i in 0..=RUNS {
                let (status, ms, queries, v) = crate::state::replay_for_verify(&base, &url);
                if status == 0 {
                    let _ = reply.send(json!({
                        "ok": false,
                        "error": format!("no answer from {url}: is the app running?"),
                    }));
                    return;
                }
                if i == 0 {
                    continue; // warm-up
                }
                visible &= v;
                runs.push(metrics_of(&crate::verify::sample_from_replay(
                    status, ms, &queries,
                )));
            }
            let Some(m) = median(runs) else { return };
            let before = baseline_for(&url, &m, visible, reset);
            let resp = answer(&url, &m, visible, before.as_ref());
            log(match resp.get("comparison") {
                Some(c) => format!("{url}: {}", c["summary"].as_str().unwrap_or("")),
                None => format!("{url}: baseline {:.0} ms", m.ms),
            });
            let _ = reply.send(resp);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use e_laravel::laravel::RouteInfo;

    fn route(methods: &str, uri: &str, name: &str) -> RouteInfo {
        RouteInfo {
            name: name.into(),
            uri: uri.into(),
            methods: methods.into(),
            action: String::new(),
            middleware: String::new(),
        }
    }

    const BASE: &str = "https://shop.test";

    #[test]
    fn targets_paths_routes_and_urls_on_the_app_only() {
        let routes = [
            route("GET|HEAD", "orders", "orders.index"),
            route("GET|HEAD", "orders/{order}", "orders.show"),
            route("POST", "orders", "orders.store"),
        ];
        let t = |v: Value| target(&v, &routes, BASE);
        assert_eq!(
            t(json!({"uri": "/orders"})).unwrap(),
            "https://shop.test/orders"
        );
        assert_eq!(
            t(json!({"route": "orders.index"})).unwrap(),
            "https://shop.test/orders"
        );
        assert_eq!(
            t(json!({"uri": "https://shop.test/orders?page=2"})).unwrap(),
            "https://shop.test/orders?page=2"
        );
        assert!(t(json!({"route": "orders.store"}))
            .unwrap_err()
            .contains("POST"));
        assert!(t(json!({"route": "orders.show"}))
            .unwrap_err()
            .contains("parameters"));
        assert!(t(json!({"uri": "/orders/{order}"})).is_err());
        assert!(t(json!({"route": "nope"})).is_err());
        assert!(t(json!({"uri": "https://evil.test/x"})).is_err());
        assert!(t(json!({"uri": "https://shop.test.evil.test/x"})).is_err());
        assert!(t(json!({})).is_err());
    }

    fn metrics(ms: f64, queries: &[&str]) -> RequestMetrics {
        let q: Vec<(String, String)> = queries
            .iter()
            .map(|s| (s.to_string(), "1".to_string()))
            .collect();
        metrics_of(&crate::verify::sample_from_replay(200, ms, &q))
    }

    #[test]
    fn the_median_run_is_the_measurement() {
        let m = median(vec![
            metrics(90.0, &[]),
            metrics(20.0, &[]),
            metrics(30.0, &[]),
        ])
        .unwrap();
        assert_eq!(m.ms, 30.0);
    }

    #[test]
    fn first_measurement_is_the_baseline_later_ones_compare_with_it() {
        let url = "https://baseline.test/orders";
        let n1 = [
            "select * from users",
            "select * from orders where user_id = 1",
            "select * from orders where user_id = 2",
            "select * from orders where user_id = 3",
        ];
        let slow = metrics(120.0, &n1);
        assert!(baseline_for(url, &slow, true, false).is_none());
        let first = answer(url, &slow, true, None);
        assert_eq!(first["baseline"], true);
        assert_eq!(first["metrics"]["n_plus_one"][0]["count"], 3);

        let fixed = metrics(
            40.0,
            &[
                "select * from users",
                "select * from orders where user_id in (1, 2, 3)",
            ],
        );
        let before = baseline_for(url, &fixed, true, false).unwrap();
        let second = answer(url, &fixed, true, Some(&before));
        assert_eq!(second["comparison"]["verdict"], "Improved");
        assert_eq!(second["comparison"]["n_plus_one_fixed"], true);
        assert_eq!(second["comparison"]["queries_before"], 4);

        // The baseline stays put until reset.
        assert_eq!(baseline_for(url, &fixed, true, false).unwrap().0.ms, 120.0);
        assert!(baseline_for(url, &fixed, true, true).is_none());
        assert_eq!(baseline_for(url, &slow, true, false).unwrap().0.ms, 40.0);
    }

    #[test]
    fn without_query_data_only_time_and_status_are_compared() {
        let before = metrics(100.0, &[]);
        let after = metrics(100.0, &[]);
        let a = answer("u", &after, false, Some(&(before.clone(), false)));
        assert!(a["metrics"].get("queries").is_none());
        assert_eq!(a["comparison"]["verdict"], "No change");
        assert!(a["comparison"].get("queries_after").is_none());
        assert_eq!(a["comparison"]["summary"], "100ms → 100ms");
        // Visible now but not in the baseline: still no query comparison.
        let b = answer("u", &after, true, Some(&(before, false)));
        assert!(b["comparison"].get("queries_before").is_none());
        assert!(a["note"].as_str().unwrap().contains("not visible"));
    }
}
