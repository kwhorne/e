//! Continuous "Runtime" panel state (Clockwork request capture).
//!
//! The view lives in [`crate::runtime_view`]; this module owns the request model
//! and the `AppState` methods that poll Clockwork and hand a request to the
//! agent. Extracted from the former `state.rs` god-module so the feature lives
//! in one place (same pattern as [`crate::debug`]).

use floem::ext_event::create_ext_action;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};

use crate::state::AppState;

/// One captured request in the continuous Runtime panel (from Clockwork).
#[derive(Clone)]
pub struct RuntimeReq {
    pub id: String,
    pub method: String,
    pub uri: String,
    pub status: u16,
    pub duration_ms: f64,
    pub queries: Vec<(String, String)>,
    pub cache_hits: usize,
    pub cache_misses: usize,
    pub mails: Vec<String>,
    pub events: usize,
}

/// Parse a Clockwork `/__clockwork/latest` payload into a [`RuntimeReq`].
fn parse_clockwork_latest(v: &serde_json::Value) -> Option<RuntimeReq> {
    let id = v.get("id")?.as_str()?.to_string();
    let method = v
        .get("method")
        .and_then(|x| x.as_str())
        .unwrap_or("GET")
        .to_string();
    let uri = v
        .get("uri")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let status = v
        .get("responseStatus")
        .and_then(|x| x.as_u64())
        .unwrap_or(0) as u16;
    let duration_ms = v
        .get("responseDuration")
        .and_then(|x| x.as_f64())
        .unwrap_or(0.0);
    let queries = v
        .get("databaseQueries")
        .and_then(|q| q.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|q| {
                    let sql = q.get("query").and_then(|s| s.as_str())?.to_string();
                    let dur = q.get("duration").map(|d| d.to_string()).unwrap_or_default();
                    Some((sql, dur))
                })
                .collect()
        })
        .unwrap_or_default();
    let (mut cache_hits, mut cache_misses) = (0, 0);
    if let Some(arr) = v.get("cacheQueries").and_then(|c| c.as_array()) {
        for c in arr {
            match c.get("type").and_then(|t| t.as_str()) {
                Some("hit") => cache_hits += 1,
                Some("miss") => cache_misses += 1,
                _ => {}
            }
        }
    }
    let mails = v
        .get("emailsData")
        .or_else(|| v.get("emails"))
        .and_then(|e| e.as_array())
        .map(|arr| {
            arr.iter()
                .map(|m| {
                    m.get("data")
                        .and_then(|d| d.get("subject"))
                        .or_else(|| m.get("subject"))
                        .and_then(|s| s.as_str())
                        .unwrap_or("(email)")
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default();
    let events = v
        .get("events")
        .and_then(|e| e.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    Some(RuntimeReq {
        id,
        method,
        uri,
        status,
        duration_ms,
        queries,
        cache_hits,
        cache_misses,
        mails,
        events,
    })
}

impl AppState {
    // ---- Runtime insight -----------------------------------------------

    pub fn toggle_runtime(&self) {
        let open = !self.runtime_open.get_untracked();
        self.runtime_open.set(open);
        if open {
            self.poll_runtime();
        }
    }

    pub fn clear_runtime(&self) {
        self.runtime_reqs.set(Vec::new());
    }

    /// Poll Clockwork for the latest request and prepend it if it's new.
    /// Called on the idle tick while the Runtime panel is open.
    pub fn poll_runtime(&self) {
        if self.runtime_polling.get_untracked() {
            return;
        }
        self.runtime_polling.set(true);
        let base = self.app_base();
        let reqs = self.runtime_reqs;
        let polling = self.runtime_polling;
        let send = create_ext_action(self.cx, move |req: Option<RuntimeReq>| {
            polling.set(false);
            if let Some(req) = req {
                reqs.update(|list| {
                    if !list.iter().any(|r| r.id == req.id) {
                        list.insert(0, req);
                        list.truncate(50);
                    }
                });
            }
        });
        std::thread::spawn(move || {
            let out = std::process::Command::new("curl")
                .args(["-sk", "--max-time", "8"])
                .arg(format!("{base}/__clockwork/latest"))
                .output();
            let req = out.ok().and_then(|o| {
                serde_json::from_slice::<serde_json::Value>(&o.stdout)
                    .ok()
                    .and_then(|v| parse_clockwork_latest(&v))
            });
            send(req);
        });
    }

    /// Send a captured request to the agent for analysis.
    pub fn runtime_explain(&self, id: &str) {
        let req = self
            .runtime_reqs
            .with_untracked(|list| list.iter().find(|r| r.id == id).cloned());
        if let Some(r) = req {
            self.send_to_agent(&format!(
                "Analyze this request captured from the running app. {} {} responded {} in {:.0}ms, \
                 running {} SQL queries ({} cache hits, {} misses, {} mails, {} events). \
                 Point out N+1 problems, slow queries, and anything to improve.",
                r.method,
                r.uri,
                r.status,
                r.duration_ms,
                r.queries.len(),
                r.cache_hits,
                r.cache_misses,
                r.mails.len(),
                r.events
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_clockwork_latest;

    #[test]
    fn parses_clockwork_payload() {
        let v: serde_json::Value = serde_json::json!({
            "id": "abc123",
            "method": "POST",
            "uri": "/orders",
            "responseStatus": 201,
            "responseDuration": 84.5,
            "databaseQueries": [
                {"query": "select * from users where id = 1", "duration": 2},
                {"query": "select * from orders where user_id = 1", "duration": 3}
            ],
            "cacheQueries": [{"type": "hit"}, {"type": "miss"}, {"type": "hit"}],
            "emailsData": [{"data": {"subject": "Order shipped"}}],
            "events": [{"event": "OrderPlaced"}]
        });
        let r = parse_clockwork_latest(&v).unwrap();
        assert_eq!(r.id, "abc123");
        assert_eq!(r.method, "POST");
        assert_eq!(r.status, 201);
        assert_eq!(r.queries.len(), 2);
        assert_eq!(r.cache_hits, 2);
        assert_eq!(r.cache_misses, 1);
        assert_eq!(r.mails, vec!["Order shipped"]);
        assert_eq!(r.events, 1);
    }
}
