//! A feature test from a captured request: the Runtime panel's 🧪, the
//! runtime lens's code action, and `request_test` on the agent socket. The
//! test is made by [`e_laravel::request_test`]; this fetches what Grove
//! captured, puts the test in its file and opens it there.

use std::sync::mpsc::Sender;

use e_laravel::laravel::{route_for, RouteInfo};
use e_laravel::request_test::{for_project, Captured, Written};
use floem::reactive::{SignalGet, SignalWith};
use serde_json::{json, Value};

use crate::grove;
use crate::runtime::RuntimeReq;
use crate::state::AppState;

/// Grove's record of a request, as the generator takes it.
fn from_explain(e: grove::Explain) -> Result<Captured, String> {
    if e.body_truncated {
        return Err("its body was too big for Grove to keep whole".into());
    }
    Ok(Captured {
        method: e.method,
        path: e.path,
        status: e.status,
        headers: e.headers,
        body: e.body,
        queries: e.queries,
    })
}

/// What was captured of a request: Grove's whole record when it has one,
/// otherwise what the panel knows (Clockwork keeps no body). Blocking.
fn captured(r: &RuntimeReq) -> Result<Captured, String> {
    if let Some(gid) = r.grove_id {
        let e = grove::explain(gid).ok_or("Grove no longer has the request")?;
        return from_explain(e);
    }
    Ok(Captured {
        method: r.method.clone(),
        path: r.uri.clone(),
        status: r.status,
        queries: r.queries.iter().map(|(q, _)| q.clone()).collect(),
        ..Captured::default()
    })
}

/// Whether a captured request is the one asked for by `uri`: the same path,
/// or — for a route's URI with its `{parameters}` — any request to that
/// route.
fn wanted(
    routes: &[RouteInfo],
    uri: &str,
    method: Option<&str>,
    r_method: &str,
    r_path: &str,
) -> bool {
    if method.is_some_and(|m| !m.eq_ignore_ascii_case(r_method)) {
        return false;
    }
    let path = |p: &str| {
        let p = p.split('?').next().unwrap_or(p);
        format!("/{}", p.trim_matches('/'))
    };
    if uri.contains('{') {
        return route_for(routes, r_method, r_path).is_some_and(|r| path(&r.uri) == path(uri));
    }
    path(r_path) == path(uri)
}

impl AppState {
    /// Write a feature test from a captured request — replay it on a fresh
    /// database and assert what it did — and open it.
    pub fn runtime_test(&self, id: &str) {
        let Some(r) = self
            .runtime_reqs
            .with_untracked(|list| list.iter().find(|r| r.id == id).cloned())
        else {
            return;
        };
        let root = self.root.get_untracked();
        let routes = self
            .laravel
            .get_untracked()
            .map(|d| d.all_routes.clone())
            .unwrap_or_default();
        let what = format!("{} {}", r.method, r.uri);
        let app = *self;
        self.spawn_bg(
            move || captured(&r).and_then(|c| for_project(&root, &routes, &c)),
            move |res: Result<Written, String>| match res {
                Ok(w) => app.place_test(w),
                Err(e) => Self::notify(&format!("No test from {what}: {e}")),
            },
        );
    }

    /// Put a written test in its file, and show it.
    fn place_test(&self, w: Written) {
        let root = self.root.get_untracked();
        let rel = w
            .path
            .strip_prefix(&root)
            .unwrap_or(&w.path)
            .display()
            .to_string();
        let open = self.buffers.with_untracked(|bs| {
            bs.iter()
                .find(|b| b.file.path.as_deref() == Some(w.path.as_path()))
                .cloned()
        });
        if open.as_ref().is_some_and(|b| b.dirty.get_untracked()) {
            Self::notify(&format!("Save {rel} first: the test goes at its end"));
            return;
        }
        if let Some(dir) = w.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if !Self::write_or_notify(&w.path, &w.content) {
            return;
        }
        if let Some(b) = open {
            self.reload_buffer(&b);
        }
        self.goto_location(w.path.clone(), w.line.saturating_sub(1), 0);
        let mut msg = format!("Wrote “{}” in {rel}", w.name);
        for n in &w.notes {
            msg.push_str(&format!("\n• {n}"));
        }
        Self::notify(&msg);
    }

    /// `request_test` on the agent socket: the test for the newest captured
    /// request to `uri` (a path, or a route's URI with its parameters), or
    /// the Runtime panel's `id`. Returns the test file as it would be with
    /// the test added; writes nothing.
    pub fn agent_request_test(&self, req: &Value, reply: Sender<Value>) {
        let root = self.root.get_untracked();
        let routes = self
            .laravel
            .get_untracked()
            .map(|d| d.all_routes.clone())
            .unwrap_or_default();
        let id = req.get("id").and_then(Value::as_str).map(str::to_string);
        let uri = req.get("uri").and_then(Value::as_str).map(str::to_string);
        let method = req
            .get("http_method")
            .and_then(Value::as_str)
            .map(str::to_string);
        if id.is_none() && uri.is_none() {
            let _ = reply.send(json!({"ok": false, "error": "missing uri (or id)"}));
            return;
        }
        // What the panel has, for Clockwork (Grove is asked afresh below).
        let panel: Vec<RuntimeReq> = self.runtime_reqs.get_untracked();
        let site = self.grove_site();
        std::thread::spawn(move || {
            let found: Result<Captured, String> = (|| {
                if let Some(id) = &id {
                    let gid = id.trim_start_matches('g').parse::<u64>().ok();
                    if let Some(r) = panel.iter().find(|r| r.id == *id) {
                        return captured(r);
                    }
                    let gid = gid.ok_or(format!("no captured request {id}"))?;
                    return from_explain(
                        grove::explain(gid).ok_or(format!("Grove has no request {id}"))?,
                    );
                }
                let uri = uri.as_deref().unwrap_or("");
                let m = method.as_deref();
                if let Some(site) = &site {
                    let list = grove::requests(&site.name, 300).unwrap_or_default();
                    if let Some(r) = list
                        .iter()
                        .find(|r| wanted(&routes, uri, m, &r.method, &r.path))
                    {
                        return from_explain(grove::explain(r.id).ok_or("Grove lost the request")?);
                    }
                }
                panel
                    .iter()
                    .find(|r| wanted(&routes, uri, m, &r.method, &r.uri))
                    .map(captured)
                    .unwrap_or_else(|| {
                        Err(format!(
                            "no captured request to {uri}: make it in the browser first, with the app served by Grove"
                        ))
                    })
            })();
            let resp = match found.and_then(|c| for_project(&root, &routes, &c)) {
                Ok(w) => json!({
                    "ok": true,
                    "file": w.path.to_string_lossy(),
                    "line": w.line,
                    "test": w.name,
                    "content": w.content,
                    "notes": w.notes,
                }),
                Err(e) => json!({"ok": false, "error": e}),
            };
            let _ = reply.send(resp);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_is_found_by_its_path_or_its_route() {
        let routes = vec![RouteInfo {
            name: String::new(),
            uri: "orders/{order}".into(),
            methods: "GET|HEAD|PUT".into(),
            action: "App\\Http\\Controllers\\OrderController@show".into(),
            middleware: "web".into(),
        }];
        assert!(wanted(&routes, "/orders/4", None, "GET", "/orders/4?tab=a"));
        assert!(!wanted(&routes, "/orders/4", None, "GET", "/orders/5"));
        assert!(wanted(
            &routes,
            "orders/{order}",
            Some("put"),
            "PUT",
            "/orders/5"
        ));
        assert!(!wanted(
            &routes,
            "orders/{order}",
            Some("PUT"),
            "GET",
            "/orders/5"
        ));
    }
}
