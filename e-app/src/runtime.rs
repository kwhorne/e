//! Continuous "Runtime" panel state: the requests hitting the running app.
//!
//! Two sources feed it. With **Grove** serving the project, the timeline comes
//! from Grove's proxy — every request, framework-agnostic, with nothing
//! installed in the app — and each entry can be enriched with its causal chain
//! (SQL when `grove sql-capture` is on, mail sent) and the matching error-log
//! entries. Without Grove, the panel polls **Clockwork** (`/__clockwork/latest`)
//! as before.
//!
//! The view lives in [`crate::runtime_view`]; this module owns the request model
//! and the `AppState` methods that poll and hand a request to the agent.

use floem::ext_event::create_ext_action;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};

use crate::grove;
use crate::state::AppState;

/// One captured request in the continuous Runtime panel.
#[derive(Clone)]
pub struct RuntimeReq {
    pub id: String,
    pub method: String,
    pub uri: String,
    pub status: u16,
    pub duration_ms: f64,
    /// `(sql, duration)`; the duration is empty when the source doesn't time
    /// queries (Grove's general-log capture doesn't).
    pub queries: Vec<(String, String)>,
    pub cache_hits: usize,
    pub cache_misses: usize,
    pub mails: Vec<String>,
    pub events: usize,
    /// Grove's id for the entry, when it came from Grove's timeline.
    pub grove_id: Option<u64>,
    /// Error-log entries Grove matched to the request, one line each.
    pub logs: Vec<String>,
    /// Whether the side effects (queries, mail, logs) have been fetched. Always
    /// true for Clockwork, which delivers everything in one payload.
    pub chain_loaded: bool,
}

/// One line of a request's expanded detail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DetailLine {
    Query { sql: String, duration: String },
    Mail(String),
    Log(String),
    Note(String),
}

impl RuntimeReq {
    /// A Grove timeline entry, with its chain when already fetched.
    pub fn from_grove(r: grove::Request, detail: Option<grove::Explain>) -> Self {
        let mut req = RuntimeReq {
            id: format!("g{}", r.id),
            method: r.method,
            uri: r.path,
            status: r.status,
            duration_ms: r.duration_ms as f64,
            queries: Vec::new(),
            cache_hits: 0,
            cache_misses: 0,
            mails: Vec::new(),
            events: 0,
            grove_id: Some(r.id),
            logs: Vec::new(),
            chain_loaded: false,
        };
        if let Some(d) = detail {
            req.absorb(d);
        }
        req
    }

    /// Fold a fetched chain into the entry.
    pub fn absorb(&mut self, d: grove::Explain) {
        self.queries = d.queries.into_iter().map(|q| (q, String::new())).collect();
        self.mails = d.emails;
        self.logs = d
            .logs
            .iter()
            .map(|l| {
                let first = l.message.lines().next().unwrap_or("").trim();
                format!("{} {}", l.level, first)
            })
            .collect();
        self.chain_loaded = true;
    }

    /// What the expanded row shows. `sql_capture` is Grove's capture state,
    /// so an empty query list can say why it is empty.
    pub fn detail_lines(&self, sql_capture: Option<bool>) -> Vec<DetailLine> {
        let mut out = Vec::new();
        if self.grove_id.is_some() && !self.chain_loaded {
            out.push(DetailLine::Note("Loading…".into()));
            return out;
        }
        for (sql, duration) in &self.queries {
            out.push(DetailLine::Query {
                sql: sql.clone(),
                duration: duration.clone(),
            });
        }
        if self.queries.is_empty() && self.grove_id.is_some() && sql_capture == Some(false) {
            out.push(DetailLine::Note(
                "No SQL: turn on SQL capture (header) to see this request's queries".into(),
            ));
        }
        for m in &self.mails {
            out.push(DetailLine::Mail(m.clone()));
        }
        for l in &self.logs {
            out.push(DetailLine::Log(l.clone()));
        }
        out
    }
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
        grove_id: None,
        logs: Vec::new(),
        chain_loaded: true,
    })
}

/// How many of the newest unseen Grove entries get their chain fetched per
/// poll. The rest load when expanded; a fresh panel shouldn't fire fifty
/// `grove explain` calls at once.
const EAGER_CHAINS_PER_POLL: usize = 3;

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

    /// Find out, once, whether Grove serves this project — and if so, the real
    /// host, whether it has HTTPS, and whether SQL capture is on. Off the UI
    /// thread; until it lands the Runtime panel falls back to Clockwork.
    pub fn resolve_grove_site(&self) {
        let root = self.root.get_untracked();
        let site_sig = self.grove_site;
        let sql_sig = self.grove_sql_capture;
        self.spawn_bg(
            move || {
                if !grove::available() {
                    return (None, None);
                }
                let site = grove::site_for(&root);
                let sql = if site.is_some() {
                    grove::sql_capture()
                } else {
                    None
                };
                (site, sql)
            },
            move |(site, sql): (Option<grove::Site>, Option<bool>)| {
                if let Some(s) = &site {
                    eprintln!(
                        "e: Grove serves this project as {} ({})",
                        s.name,
                        s.base_url()
                    );
                }
                site_sig.set(Some(site));
                sql_sig.set(sql);
            },
        );
    }

    /// The Grove site for this project, if resolved and served.
    pub fn grove_site(&self) -> Option<grove::Site> {
        self.grove_site.get_untracked().flatten()
    }

    /// Poll for new requests. Called on the idle tick while the panel is open.
    pub fn poll_runtime(&self) {
        if self.runtime_polling.get_untracked() {
            return;
        }
        match self.grove_site() {
            Some(site) => self.poll_runtime_grove(site),
            None => self.poll_runtime_clockwork(),
        }
    }

    /// Grove's timeline for this site: merge what we haven't seen, newest on top.
    fn poll_runtime_grove(&self, site: grove::Site) {
        self.runtime_polling.set(true);
        let known: Vec<u64> = self
            .runtime_reqs
            .with_untracked(|l| l.iter().filter_map(|r| r.grove_id).collect());
        let reqs = self.runtime_reqs;
        let polling = self.runtime_polling;
        self.spawn_bg(
            move || {
                let list = grove::requests(&site.name, 50).unwrap_or_default();
                let mut fresh = Vec::new();
                for (i, r) in list
                    .into_iter()
                    .filter(|r| !known.contains(&r.id))
                    .enumerate()
                {
                    let detail = if i < EAGER_CHAINS_PER_POLL {
                        grove::explain(r.id)
                    } else {
                        None
                    };
                    fresh.push(RuntimeReq::from_grove(r, detail));
                }
                fresh
            },
            move |fresh: Vec<RuntimeReq>| {
                polling.set(false);
                if fresh.is_empty() {
                    return;
                }
                reqs.update(|list| {
                    // Grove lists newest first; insert oldest-first at the top
                    // so the newest ends up on top.
                    for r in fresh.into_iter().rev() {
                        if !list.iter().any(|x| x.id == r.id) {
                            list.insert(0, r);
                        }
                    }
                    list.truncate(50);
                });
            },
        );
    }

    /// Poll Clockwork for the latest request and prepend it if it's new.
    fn poll_runtime_clockwork(&self) {
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

    /// Fetch a Grove entry's chain on demand (a backlog row being expanded).
    pub fn runtime_load_chain(&self, id: &str) {
        let req = self
            .runtime_reqs
            .with_untracked(|list| list.iter().find(|r| r.id == id).cloned());
        let Some(req) = req else {
            return;
        };
        let Some(gid) = req.grove_id else {
            return;
        };
        if req.chain_loaded {
            return;
        }
        let reqs = self.runtime_reqs;
        let id = id.to_string();
        self.spawn_bg(
            move || grove::explain(gid),
            move |detail: Option<grove::Explain>| {
                let Some(d) = detail else {
                    return;
                };
                reqs.update(|list| {
                    if let Some(r) = list.iter_mut().find(|r| r.id == id) {
                        r.absorb(d);
                    }
                });
            },
        );
    }

    /// Turn Grove's SQL capture on or off (MySQL general log ↔ timeline).
    pub fn toggle_grove_sql_capture(&self) {
        let on = !self.grove_sql_capture.get_untracked().unwrap_or(false);
        let sig = self.grove_sql_capture;
        self.spawn_bg(
            move || grove::set_sql_capture(on),
            move |state: Option<bool>| {
                sig.set(state);
                if state == Some(on) {
                    Self::notify(if on {
                        "SQL capture on — new requests show their queries"
                    } else {
                        "SQL capture off"
                    });
                } else {
                    Self::notify("Could not change SQL capture (MySQL only — see `grove sql-capture status`)");
                }
            },
        );
    }

    /// Send a captured request to the agent for analysis. With Grove, the agent
    /// gets the whole `grove explain` bundle: the request, its SQL and mail, and
    /// the matching stacktrace from the Laravel log.
    pub fn runtime_explain(&self, id: &str) {
        let req = self
            .runtime_reqs
            .with_untracked(|list| list.iter().find(|r| r.id == id).cloned());
        let Some(r) = req else {
            return;
        };
        if let Some(gid) = r.grove_id {
            let app = *self;
            let sql_capture = self.grove_sql_capture.get_untracked();
            let fallback = clockwork_prompt(&r);
            self.spawn_bg(
                move || grove::explain(gid),
                move |detail: Option<grove::Explain>| {
                    let prompt = match detail {
                        Some(d) => grove::explain_prompt(&d, sql_capture),
                        None => fallback,
                    };
                    app.send_to_agent(&prompt);
                },
            );
            return;
        }
        self.send_to_agent(&clockwork_prompt(&r));
    }
}

/// The agent prompt for a request we only have summary numbers for.
fn clockwork_prompt(r: &RuntimeReq) -> String {
    format!(
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
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(r.chain_loaded);
        assert_eq!(r.grove_id, None);
    }

    fn grove_req() -> grove::Request {
        grove::Request {
            id: 42,
            time: "09:30:04.107".into(),
            epoch_ms: 1,
            site: "felagi".into(),
            method: "GET".into(),
            path: "/bestillinger".into(),
            status: 500,
            duration_ms: 233,
        }
    }

    #[test]
    fn a_grove_entry_without_its_chain_says_it_is_loading() {
        let r = RuntimeReq::from_grove(grove_req(), None);
        assert_eq!(r.id, "g42");
        assert!(!r.chain_loaded);
        assert_eq!(
            r.detail_lines(Some(true)),
            vec![DetailLine::Note("Loading…".into())]
        );
    }

    #[test]
    fn absorbing_a_chain_fills_queries_mail_and_log_lines() {
        let mut r = RuntimeReq::from_grove(grove_req(), None);
        r.absorb(grove::Explain {
            queries: vec!["select 1".into()],
            emails: vec!["Kvittering → kh@gets.no".into()],
            logs: vec![grove::LogEntry {
                level: "ERROR".into(),
                datetime: "now".into(),
                message: "Boom\nsecond line".into(),
                context: None,
            }],
            ..Default::default()
        });
        assert!(r.chain_loaded);
        let lines = r.detail_lines(Some(true));
        assert_eq!(
            lines,
            vec![
                DetailLine::Query {
                    sql: "select 1".into(),
                    duration: String::new()
                },
                DetailLine::Mail("Kvittering → kh@gets.no".into()),
                DetailLine::Log("ERROR Boom".into()),
            ]
        );
    }

    #[test]
    fn an_empty_chain_explains_itself_when_capture_is_off() {
        let mut r = RuntimeReq::from_grove(grove_req(), None);
        r.absorb(grove::Explain::default());
        let lines = r.detail_lines(Some(false));
        assert!(matches!(lines.first(), Some(DetailLine::Note(n)) if n.contains("SQL capture")));
        // With capture on, an empty chain is just empty.
        assert!(r.detail_lines(Some(true)).is_empty());
    }
}
