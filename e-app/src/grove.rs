//! Grove — the local development environment `e` pairs with — as a data source.
//!
//! Grove is the proxy in front of every `*.test` site, so it already has what
//! the Runtime panel wants, with nothing installed in the app: the request
//! timeline, each request's causal chain (the SQL it issued when `grove
//! sql-capture` is on, the mail it sent), and the matching error-log entries
//! with their stacktraces. It also knows each site's real hostname and whether
//! it has HTTPS, which the `https://<folder>.test` guess got wrong for plain
//! `http://` sites.
//!
//! Everything here shells out to the `grove` CLI with `--json` (the CLI is a
//! thin client over the daemon's socket, and its JSON is the daemon's own
//! response types). The parsers are pure and unit-tested against real captures.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

/// A site Grove serves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Site {
    pub name: String,
    pub hostname: String,
    pub path: PathBuf,
    pub secure: bool,
    pub driver: String,
}

impl Site {
    /// The URL the app answers on — `http://` for a site without HTTPS.
    pub fn base_url(&self) -> String {
        let scheme = if self.secure { "https" } else { "http" };
        format!("{scheme}://{}", self.hostname)
    }
}

/// One entry of the request timeline (`grove requests`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub id: u64,
    pub time: String,
    pub epoch_ms: u128,
    pub site: String,
    pub method: String,
    pub path: String,
    pub status: u16,
    pub duration_ms: u64,
}

/// An error-log entry Grove matched to a request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogEntry {
    pub level: String,
    pub datetime: String,
    pub message: String,
    /// Stacktrace / JSON context, when the entry had one.
    pub context: Option<String>,
}

/// `grove explain <id>`: everything Grove knows about one request.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Explain {
    pub summary: String,
    pub is_error: bool,
    pub method: String,
    pub host: String,
    pub path: String,
    pub status: u16,
    /// Credentials are already `[redacted]` by Grove.
    pub headers: Vec<(String, String)>,
    pub body: String,
    pub body_truncated: bool,
    /// SQL issued within the request's window (only with `grove sql-capture on`).
    pub queries: Vec<String>,
    /// Mail sent within the window, as `subject → recipients`.
    pub emails: Vec<String>,
    pub logs: Vec<LogEntry>,
}

// ---- Running the CLI ------------------------------------------------------------

/// Where `grove` is. A GUI launched from the Dock inherits a minimal `PATH`, so
/// the usual install locations are tried after it.
pub fn binary() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join("grove");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    let mut candidates = Vec::new();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        candidates.push(home.join(".grove/bin/grove"));
    }
    candidates.push(PathBuf::from(
        "/Applications/Grove.app/Contents/MacOS/grove",
    ));
    candidates.into_iter().find(|c| c.is_file())
}

/// Is Grove installed on this machine?
pub fn available() -> bool {
    binary().is_some()
}

/// Run `grove <args> --json` and return the `data` of a successful reply.
fn run(args: &[&str]) -> Option<Value> {
    let bin = binary()?;
    let out = Command::new(bin).args(args).arg("--json").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let v: Value = serde_json::from_slice(&out.stdout).ok()?;
    if v.get("ok").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    v.get("data").cloned()
}

/// Every site Grove serves.
pub fn sites() -> Vec<Site> {
    run(&["list"]).map(|d| parse_sites(&d)).unwrap_or_default()
}

/// The site whose directory is `root`, if Grove serves it.
pub fn site_for(root: &Path) -> Option<Site> {
    find_site(&sites(), root)
}

/// Recent requests to `site`, newest first.
pub fn requests(site: &str, limit: usize) -> Option<Vec<Request>> {
    let limit = limit.to_string();
    run(&["requests", site, "--limit", &limit]).map(|d| parse_requests(&d))
}

/// The debugging bundle for one request.
pub fn explain(id: u64) -> Option<Explain> {
    run(&["explain", &id.to_string()]).and_then(|d| parse_explain(&d))
}

/// Whether Grove correlates SQL with the timeline right now.
pub fn sql_capture() -> Option<bool> {
    run(&["sql-capture", "status"]).and_then(|d| parse_sql_capture(&d))
}

/// Turn SQL capture on or off; returns the state afterwards.
pub fn set_sql_capture(on: bool) -> Option<bool> {
    let _ = run(&["sql-capture", if on { "on" } else { "off" }]);
    sql_capture()
}

// ---- Parsers (pure) ---------------------------------------------------------------

fn str_of(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

pub fn parse_sites(data: &Value) -> Vec<Site> {
    data.get("sites")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|s| {
                    let name = s.get("name")?.as_str()?.to_string();
                    Some(Site {
                        name,
                        hostname: str_of(s, "hostname"),
                        path: PathBuf::from(str_of(s, "path")),
                        secure: s.get("secure").and_then(Value::as_bool).unwrap_or(false),
                        driver: str_of(s, "driver"),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Match a project root to a site: by canonical path, then by path ignoring
/// case (macOS file systems usually do), then by the folder's name.
pub fn find_site(sites: &[Site], root: &Path) -> Option<Site> {
    let canon = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    if let Some(s) = sites
        .iter()
        .find(|s| s.path.canonicalize().map(|p| p == canon).unwrap_or(false))
    {
        return Some(s.clone());
    }
    let lower = canon.to_string_lossy().to_lowercase();
    if let Some(s) = sites
        .iter()
        .find(|s| s.path.to_string_lossy().to_lowercase() == lower)
    {
        return Some(s.clone());
    }
    let name = root.file_name()?.to_string_lossy().to_lowercase();
    sites
        .iter()
        .find(|s| s.name.to_lowercase() == name)
        .cloned()
}

pub fn parse_requests(data: &Value) -> Vec<Request> {
    data.get("requests")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(parse_request).collect())
        .unwrap_or_default()
}

fn parse_request(r: &Value) -> Option<Request> {
    Some(Request {
        id: r.get("id")?.as_u64()?,
        time: str_of(r, "time"),
        epoch_ms: r.get("epoch_ms").and_then(Value::as_u64).unwrap_or(0) as u128,
        site: str_of(r, "site"),
        method: str_of(r, "method"),
        path: str_of(r, "path"),
        status: r.get("status").and_then(Value::as_u64).unwrap_or(0) as u16,
        duration_ms: r.get("duration_ms").and_then(Value::as_u64).unwrap_or(0),
    })
}

pub fn parse_sql_capture(data: &Value) -> Option<bool> {
    data.pointer("/sql_capture/enabled")
        .and_then(Value::as_bool)
}

pub fn parse_explain(data: &Value) -> Option<Explain> {
    let e = data.get("explain")?;
    let req = e.get("request").cloned().unwrap_or(Value::Null);
    let chain = e.get("chain").cloned().unwrap_or(Value::Null);
    let headers = req
        .get("headers")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|h| {
                    let pair = h.as_array()?;
                    Some((
                        pair.first()?.as_str()?.to_string(),
                        pair.get(1)?.as_str()?.to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    let queries = chain
        .get("queries")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().map(|q| str_of(q, "sql")).collect())
        .unwrap_or_default();
    let emails = chain
        .get("emails")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .map(|m| {
                    let to: Vec<String> = m
                        .get("to")
                        .and_then(Value::as_array)
                        .map(|t| {
                            t.iter()
                                .filter_map(Value::as_str)
                                .map(str::to_string)
                                .collect()
                        })
                        .unwrap_or_default();
                    format!("{} → {}", str_of(m, "subject"), to.join(", "))
                })
                .collect()
        })
        .unwrap_or_default();
    let logs = e
        .get("logs")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .map(|l| LogEntry {
                    level: str_of(l, "level"),
                    datetime: str_of(l, "datetime"),
                    message: str_of(l, "message"),
                    context: l.get("context").and_then(Value::as_str).map(str::to_string),
                })
                .collect()
        })
        .unwrap_or_default();
    Some(Explain {
        summary: str_of(e, "summary"),
        is_error: e.get("is_error").and_then(Value::as_bool).unwrap_or(false),
        method: str_of(&req, "method"),
        host: str_of(&req, "host"),
        path: str_of(&req, "path"),
        status: req.get("status").and_then(Value::as_u64).unwrap_or(0) as u16,
        headers,
        body: str_of(&req, "body"),
        body_truncated: req
            .get("body_truncated")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        queries,
        emails,
        logs,
    })
}

// ---- For the agent ----------------------------------------------------------------

/// Trim `s` to about `max` bytes on a character boundary, marking the cut.
fn clip(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}… [truncated]", &s[..end])
}

/// The prompt that hands a request to the agent: what Grove saw, structured so
/// the agent can go straight to the cause instead of asking what happened.
pub fn explain_prompt(e: &Explain, sql_capture_on: Option<bool>) -> String {
    let mut out = String::new();
    out.push_str(if e.is_error {
        "This request failed in the running app. Grove (the local proxy) captured it; \
         find the cause and fix it.\n\n"
    } else {
        "Analyze this request captured by Grove (the local proxy) from the running app. \
         Point out N+1 problems, slow or redundant queries, and anything to improve.\n\n"
    });
    out.push_str(&format!(
        "{}\n{} {}{} → {}\n",
        e.summary, e.method, e.host, e.path, e.status
    ));

    if !e.headers.is_empty() {
        out.push_str("\nRequest headers (credentials redacted by Grove):\n");
        for (k, v) in &e.headers {
            out.push_str(&format!("  {k}: {v}\n"));
        }
    }
    if !e.body.trim().is_empty() {
        out.push_str("\nRequest body:\n");
        out.push_str(&clip(&e.body, 2000));
        if e.body_truncated {
            out.push_str(" [truncated by Grove]");
        }
        out.push('\n');
    }

    if e.queries.is_empty() {
        match sql_capture_on {
            Some(false) => out.push_str(
                "\nSQL: not captured — `grove sql-capture on` correlates queries with requests.\n",
            ),
            _ => out.push_str("\nSQL: no queries captured in the request's window.\n"),
        }
    } else {
        out.push_str(&format!("\nSQL ({} queries, in order):\n", e.queries.len()));
        for (i, q) in e.queries.iter().enumerate() {
            out.push_str(&format!("  {}. {}\n", i + 1, clip(q.trim(), 600)));
        }
    }
    if !e.emails.is_empty() {
        out.push_str(&format!("\nMail sent ({}):\n", e.emails.len()));
        for m in &e.emails {
            out.push_str(&format!("  ✉ {m}\n"));
        }
    }
    if !e.logs.is_empty() {
        out.push_str(&format!(
            "\nMatching error log ({} entries, newest last):\n",
            e.logs.len()
        ));
        for l in &e.logs {
            out.push_str(&format!(
                "[{}] {} {}\n",
                l.level,
                l.datetime,
                l.message.trim()
            ));
            if let Some(ctx) = &l.context {
                out.push_str(&clip(ctx.trim(), 3000));
                out.push('\n');
            }
        }
    }
    out
}

// ---- Mail-catcher and webhook hub ---------------------------------------------------

/// One captured email (`grove mail`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Email {
    pub id: u64,
    pub from: String,
    pub to: Vec<String>,
    pub subject: String,
    pub received_at: String,
    pub size: usize,
}

/// A captured email in full (`grove mail show <id>`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EmailBody {
    pub text: Option<String>,
    pub html: Option<String>,
    pub raw: String,
}

impl EmailBody {
    /// The most readable rendering we have: plain text, else HTML with its
    /// tags stripped, else the raw message.
    pub fn readable(&self) -> String {
        if let Some(t) = self.text.as_ref().filter(|t| !t.trim().is_empty()) {
            return t.clone();
        }
        if let Some(h) = self.html.as_ref().filter(|h| !h.trim().is_empty()) {
            return strip_tags(h);
        }
        self.raw.clone()
    }
}

/// Drop HTML tags and collapse whitespace — enough to read a mail's HTML body.
fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Captured emails, newest first.
pub fn mail() -> Vec<Email> {
    run(&["mail"]).map(|d| parse_mail(&d)).unwrap_or_default()
}

/// One email in full.
pub fn mail_show(id: u64) -> Option<EmailBody> {
    run(&["mail", "show", &id.to_string()]).and_then(|d| parse_mail_body(&d))
}

/// Captured inbound webhooks (requests to `/__grove/hooks/<bucket>`), newest first.
pub fn hooks(limit: usize) -> Vec<Request> {
    let limit = limit.to_string();
    run(&["hooks", "--limit", &limit])
        .map(|d| parse_hooks(&d))
        .unwrap_or_default()
}

/// Re-deliver a captured webhook to `to` (the app's handler URL).
pub fn hook_replay(id: u64, to: &str) -> bool {
    run(&["hooks", "replay", &id.to_string(), "--to", to]).is_some()
}

/// The site served on `host` (`felagi.test`), if any.
pub fn site_by_host(host: &str) -> Option<Site> {
    let host = host.trim().trim_end_matches('/').to_lowercase();
    sites()
        .into_iter()
        .find(|s| s.hostname.to_lowercase() == host)
}

pub fn parse_mail(data: &Value) -> Vec<Email> {
    data.get("mail")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|m| {
                    Some(Email {
                        id: m.get("id")?.as_u64()?,
                        from: str_of(m, "from"),
                        to: m
                            .get("to")
                            .and_then(Value::as_array)
                            .map(|t| {
                                t.iter()
                                    .filter_map(Value::as_str)
                                    .map(str::to_string)
                                    .collect()
                            })
                            .unwrap_or_default(),
                        subject: str_of(m, "subject"),
                        received_at: str_of(m, "received_at"),
                        size: m.get("size").and_then(Value::as_u64).unwrap_or(0) as usize,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn parse_mail_body(data: &Value) -> Option<EmailBody> {
    let m = data.get("mail_message")?;
    if m.is_null() {
        return None;
    }
    Some(EmailBody {
        text: m.get("text").and_then(Value::as_str).map(str::to_string),
        html: m.get("html").and_then(Value::as_str).map(str::to_string),
        raw: str_of(m, "raw"),
    })
}

pub fn parse_hooks(data: &Value) -> Vec<Request> {
    data.get("hooks")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(parse_request).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// `grove list --json`, as captured on 2026-09-05.
    fn sites_json() -> Value {
        json!({ "sites": [
            { "name": "felagi", "hostname": "felagi.test", "path": "/Users/kh/Code/felagi",
              "document_root": "/Users/kh/Code/felagi/public", "driver": "laravel", "php": "8.5",
              "secure": false, "kind": "linked", "front_controller": "index.php" },
            { "name": "elyra-web", "hostname": "elyra-web.test", "path": "/Users/kh/Code/elyra-web",
              "driver": "laravel", "php": "8.5", "node": "24", "secure": true, "kind": "linked" }
        ]})
    }

    #[test]
    fn parses_sites_and_builds_the_right_scheme() {
        let sites = parse_sites(&sites_json());
        assert_eq!(sites.len(), 2);
        assert_eq!(sites[0].name, "felagi");
        assert!(!sites[0].secure);
        // An insecure site is `http://`; the old `https://<folder>.test` guess
        // would have replayed against a port that isn't listening.
        assert_eq!(sites[0].base_url(), "http://felagi.test");
        assert_eq!(sites[1].base_url(), "https://elyra-web.test");
    }

    #[test]
    fn finds_a_site_by_path_case_insensitively_or_by_name() {
        let sites = parse_sites(&sites_json());
        // Different case in the path (macOS file systems don't care).
        let s = find_site(&sites, Path::new("/Users/kh/code/felagi")).unwrap();
        assert_eq!(s.name, "felagi");
        // A path Grove doesn't know, but whose folder matches a site's name.
        let s = find_site(&sites, Path::new("/tmp/checkouts/elyra-web")).unwrap();
        assert_eq!(s.hostname, "elyra-web.test");
        assert!(find_site(&sites, Path::new("/tmp/nothing-here")).is_none());
    }

    #[test]
    fn parses_the_request_timeline() {
        let v = json!({ "requests": [
            { "id": 299, "time": "09:30:04.107", "epoch_ms": 1788600604107u64, "site": "elyra-web",
              "method": "GET", "path": "/datagrid", "status": 200, "duration_ms": 233, "https": true },
            { "id": 298, "time": "09:28:04.552", "epoch_ms": 1788600484552u64, "site": "elyra-web",
              "method": "POST", "path": "/_boost/browser-logs", "status": 200, "duration_ms": 21, "https": true }
        ]});
        let reqs = parse_requests(&v);
        assert_eq!(reqs.len(), 2);
        assert_eq!(reqs[0].id, 299);
        assert_eq!(reqs[0].method, "GET");
        assert_eq!(reqs[0].path, "/datagrid");
        assert_eq!(reqs[0].status, 200);
        assert_eq!(reqs[0].duration_ms, 233);
        assert_eq!(reqs[1].site, "elyra-web");
    }

    #[test]
    fn parses_sql_capture_status() {
        let v = json!({ "sql_capture": { "enabled": false, "note": "off — enable with …" } });
        assert_eq!(parse_sql_capture(&v), Some(false));
        assert_eq!(parse_sql_capture(&json!({})), None);
    }

    /// `grove explain 299 --json`, as captured, with a chain and a log entry
    /// added the way the daemon shapes them.
    fn explain_json() -> Value {
        json!({ "explain": {
            "summary": "GET /datagrid → 500 on elyra-web · 2 queries, 1 emails · 1 error log entries",
            "is_error": true,
            "request": {
                "id": 299, "method": "GET", "host": "elyra-web.test", "path": "/datagrid", "https": true,
                "status": 500,
                "headers": [["user-agent", "curl/8.7.1"], ["authorization", "[redacted]"]],
                "body": "", "body_truncated": false
            },
            "chain": {
                "request": { "id": 299, "time": "09:30:04.107", "epoch_ms": 1788600604107u64, "site": "elyra-web",
                             "method": "GET", "path": "/datagrid", "status": 500, "duration_ms": 233, "https": true },
                "window_start_ms": 1788600603874u64, "window_end_ms": 1788600604107u64,
                "emails": [ { "id": 7, "from": "app@x.test", "to": ["kh@gets.no"], "subject": "Feil i datagrid",
                              "received_at": "…", "received_ms": 1788600604000u64, "size": 1200 } ],
                "queries": [ { "epoch_ms": 1788600603900u64, "engine": "mysql", "sql": "select * from `users` where `id` = 1" },
                             { "epoch_ms": 1788600603950u64, "engine": "mysql", "sql": "select * from `orders` where `user_id` = 1" } ],
                "metrics": { "duration_ms": 233, "email_count": 1, "query_count": 2 }
            },
            "logs": [ { "level": "ERROR", "datetime": "2026-09-05 09:30:04",
                        "message": "Call to undefined method App\\\\Models\\\\Order::totall()",
                        "context": "#0 /app/Http/Controllers/DatagridController.php(42): …" } ]
        }})
    }

    #[test]
    fn parses_an_explain_bundle() {
        let e = parse_explain(&explain_json()).unwrap();
        assert!(e.is_error);
        assert_eq!(e.status, 500);
        assert_eq!(e.headers[1], ("authorization".into(), "[redacted]".into()));
        assert_eq!(e.queries.len(), 2);
        assert!(e.queries[1].contains("orders"));
        assert_eq!(e.emails, vec!["Feil i datagrid → kh@gets.no"]);
        assert_eq!(e.logs.len(), 1);
        assert!(e.logs[0]
            .context
            .as_deref()
            .unwrap()
            .contains("DatagridController.php(42)"));
    }

    #[test]
    fn the_agent_prompt_carries_the_stacktrace_and_the_sql() {
        let e = parse_explain(&explain_json()).unwrap();
        let p = explain_prompt(&e, Some(true));
        assert!(p.starts_with("This request failed"));
        assert!(p.contains("GET elyra-web.test/datagrid → 500"));
        assert!(p.contains("authorization: [redacted]"));
        assert!(p.contains("1. select * from `users`"));
        assert!(p.contains("✉ Feil i datagrid"));
        assert!(p.contains("[ERROR] 2026-09-05 09:30:04 Call to undefined method"));
        assert!(p.contains("DatagridController.php(42)"));
        // With capture off and no queries, the prompt says how to get them.
        let mut quiet = e.clone();
        quiet.queries.clear();
        assert!(explain_prompt(&quiet, Some(false)).contains("grove sql-capture on"));
    }

    #[test]
    fn parses_mail_list_and_body() {
        let list = json!({ "mail": [
            { "id": 7, "from": "app@felagi.test", "to": ["kh@gets.no"], "subject": "Kvittering",
              "received_at": "2026-09-05T09:30:04Z", "received_ms": 1788600604000u64, "size": 1200 }
        ]});
        let mail = parse_mail(&list);
        assert_eq!(mail.len(), 1);
        assert_eq!(mail[0].subject, "Kvittering");
        assert_eq!(mail[0].to, vec!["kh@gets.no"]);

        let body = json!({ "mail_message": {
            "id": 7, "from": "a", "to": [], "subject": "s", "received_at": "t", "received_ms": 0, "size": 1,
            "raw": "Subject: s\r\n\r\n<p>Hei <b>Bjørn</b></p>", "text": null, "html": "<p>Hei <b>Bjørn</b></p>"
        }});
        let b = parse_mail_body(&body).unwrap();
        assert_eq!(b.readable(), "Hei Bjørn");
        assert!(parse_mail_body(&json!({ "mail_message": null })).is_none());
    }

    #[test]
    fn parses_webhooks_as_requests() {
        let v = json!({ "hooks": [
            { "id": 12, "time": "10:00:00.000", "epoch_ms": 1u64, "site": "felagi", "method": "POST",
              "path": "/__grove/hooks/stripe", "status": 200, "duration_ms": 3, "https": true }
        ]});
        let hooks = parse_hooks(&v);
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0].path, "/__grove/hooks/stripe");
        assert_eq!(hooks[0].method, "POST");
    }

    #[test]
    fn strip_tags_keeps_the_words() {
        assert_eq!(strip_tags("<div>a<br>b  <i>c</i></div>"), "a b c");
    }

    /// Against the real CLI when Grove is installed: the JSON contract holds.
    #[test]
    fn live_cli_contract_when_grove_is_installed() {
        if !available() {
            eprintln!("skipping live_cli_contract_when_grove_is_installed: grove not installed");
            return;
        }
        // `grove list --json` parses (the machine may have zero sites), and
        // `sql-capture status --json` answers with a definite state.
        let _sites = sites();
        assert!(sql_capture().is_some(), "grove sql-capture status --json");
        // A limit of 1 is honoured on the timeline (any site name is fine here).
        if let Some(first) = sites().first() {
            let reqs = requests(&first.name, 1).expect("grove requests --json");
            assert!(reqs.len() <= 1);
        }
    }

    #[test]
    fn clip_never_splits_a_character() {
        let s = "Håndter feil i bestillingen";
        let c = clip(s, 2);
        assert!(c.starts_with("H…"));
        assert_eq!(clip("kort", 100), "kort");
    }
}
