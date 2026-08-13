//! The measurement core of the "verify the fix" loop.
//!
//! Given a request captured from the running app (before a change) and one
//! captured after, this crate computes comparable [`RequestMetrics`] and a
//! [`Comparison`] verdict — did the change make it faster, cut queries, fix an
//! N+1, or break the response? It's pure and synchronous so the whole thing is
//! unit-testable without a database, a browser, or a GUI.
//!
//! The editor feeds it request samples (from the runtime capture) as JSON; the
//! shapes are mirrored in [`RequestSample::from_json`].

use std::collections::HashMap;

use serde_json::Value;

/// A single SQL query executed during a request.
#[derive(Clone, Debug, PartialEq)]
pub struct Query {
    pub sql: String,
    pub duration_ms: f64,
}

/// One request captured from the running app.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RequestSample {
    pub status: u16,
    pub duration_ms: f64,
    pub queries: Vec<Query>,
    /// An optional stable description of the response body (e.g. a shape hash or
    /// content-type + length) used to flag when a change altered the output.
    pub response_shape: Option<String>,
}

impl RequestSample {
    /// Parse the JSON shape the runtime capture uses:
    /// `{ "status": 200, "duration": 42.0, "queries": [{"query": "...",
    /// "duration": 3}], "shape": "..." }`. `duration_ms` is also accepted.
    pub fn from_json(v: &Value) -> Self {
        let status = v.get("status").and_then(Value::as_u64).unwrap_or(0) as u16;
        let duration_ms = num(v.get("duration"))
            .or_else(|| num(v.get("duration_ms")))
            .unwrap_or(0.0);
        let queries = v
            .get("queries")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .map(|q| Query {
                        sql: q
                            .get("query")
                            .or_else(|| q.get("sql"))
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        duration_ms: num(q.get("duration"))
                            .or_else(|| num(q.get("duration_ms")))
                            .unwrap_or(0.0),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let response_shape = v.get("shape").and_then(Value::as_str).map(str::to_string);
        RequestSample {
            status,
            duration_ms,
            queries,
            response_shape,
        }
    }
}

fn num(v: Option<&Value>) -> Option<f64> {
    v.and_then(|x| x.as_f64().or_else(|| x.as_i64().map(|n| n as f64)))
}

/// A group of queries that share a normalized skeleton (same shape, differing
/// only by literals) — the signature of an N+1 when the count is high.
#[derive(Clone, Debug, PartialEq)]
pub struct QueryGroup {
    pub skeleton: String,
    pub count: usize,
    pub total_ms: f64,
}

/// Comparable metrics derived from a [`RequestSample`].
#[derive(Clone, Debug, PartialEq)]
pub struct RequestMetrics {
    pub status: u16,
    pub ms: f64,
    pub query_count: usize,
    pub distinct_queries: usize,
    /// Repeated query shapes (count ≥ 2), sorted by count descending.
    pub groups: Vec<QueryGroup>,
    pub slowest_query_ms: f64,
    pub shape: Option<String>,
}

/// A repeated query shape at or above this count is treated as an N+1.
pub const N1_THRESHOLD: usize = 3;

impl RequestMetrics {
    /// True if any query shape repeats enough to look like an N+1.
    pub fn has_n_plus_one(&self) -> bool {
        self.groups.iter().any(|g| g.count >= N1_THRESHOLD)
    }

    /// The worst repeated group (highest count), if any looks like an N+1.
    pub fn worst_n_plus_one(&self) -> Option<&QueryGroup> {
        self.groups
            .iter()
            .filter(|g| g.count >= N1_THRESHOLD)
            .max_by_key(|g| g.count)
    }
}

/// Compute [`RequestMetrics`] from a captured request.
pub fn metrics_of(sample: &RequestSample) -> RequestMetrics {
    let mut groups: HashMap<String, (usize, f64)> = HashMap::new();
    let mut slowest = 0.0_f64;
    for q in &sample.queries {
        let sk = skeleton(&q.sql);
        let e = groups.entry(sk).or_insert((0, 0.0));
        e.0 += 1;
        e.1 += q.duration_ms;
        slowest = slowest.max(q.duration_ms);
    }
    let distinct_queries = groups.len();
    let mut groups: Vec<QueryGroup> = groups
        .into_iter()
        .filter(|(_, (count, _))| *count >= 2)
        .map(|(skeleton, (count, total_ms))| QueryGroup {
            skeleton,
            count,
            total_ms,
        })
        .collect();
    groups.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then(b.total_ms.total_cmp(&a.total_ms))
            .then(a.skeleton.cmp(&b.skeleton))
    });

    RequestMetrics {
        status: sample.status,
        ms: sample.duration_ms,
        query_count: sample.queries.len(),
        distinct_queries,
        groups,
        slowest_query_ms: slowest,
        shape: sample.response_shape.clone(),
    }
}

/// Overall judgement of a before → after change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Fewer queries / faster / an N+1 removed, response intact.
    Improved,
    /// No meaningful difference.
    NoChange,
    /// More queries or slower, response intact.
    Regressed,
    /// The response itself changed for the worse (error status / altered shape).
    Broke,
}

/// The before/after comparison a UI renders.
#[derive(Clone, Debug, PartialEq)]
pub struct Comparison {
    pub ms_before: f64,
    pub ms_after: f64,
    pub ms_delta: f64,
    pub ms_ratio: f64,
    pub queries_before: usize,
    pub queries_after: usize,
    pub query_delta: i64,
    pub n1_before: bool,
    pub n1_after: bool,
    pub n1_fixed: bool,
    pub faster: bool,
    pub slower: bool,
    pub status_changed: bool,
    pub shape_changed: bool,
    pub verdict: Verdict,
}

/// A timing change smaller than this fraction is treated as noise.
const MS_MARGIN: f64 = 0.05;

/// Compare two measured requests and produce a verdict.
pub fn compare(before: &RequestMetrics, after: &RequestMetrics) -> Comparison {
    let ms_delta = after.ms - before.ms;
    let ms_ratio = if before.ms > 0.0 {
        after.ms / before.ms
    } else {
        1.0
    };
    let faster = ms_ratio < 1.0 - MS_MARGIN;
    let slower = ms_ratio > 1.0 + MS_MARGIN;
    let query_delta = after.query_count as i64 - before.query_count as i64;
    let n1_before = before.has_n_plus_one();
    let n1_after = after.has_n_plus_one();
    let n1_fixed = n1_before && !n1_after;
    let status_changed = before.status != after.status;
    let shape_changed = match (&before.shape, &after.shape) {
        (Some(a), Some(b)) => a != b,
        _ => false,
    };
    // "Broke" = the response went from OK to an error, or its shape changed.
    let broke = (after.status >= 400 && before.status < 400) || shape_changed;

    let verdict = if broke {
        Verdict::Broke
    } else if n1_fixed || query_delta < 0 || faster {
        Verdict::Improved
    } else if query_delta > 0 || slower {
        Verdict::Regressed
    } else {
        Verdict::NoChange
    };

    Comparison {
        ms_before: before.ms,
        ms_after: after.ms,
        ms_delta,
        ms_ratio,
        queries_before: before.query_count,
        queries_after: after.query_count,
        query_delta,
        n1_before,
        n1_after,
        n1_fixed,
        faster,
        slower,
        status_changed,
        shape_changed,
        verdict,
    }
}

/// Normalize a SQL statement to a skeleton for grouping: literals become `?`,
/// case and whitespace are normalized, and repeated `?` lists (`IN (?, ?, ?)`)
/// collapse to `(?)`. Two queries that differ only by their bound values map to
/// the same skeleton — which is exactly how an N+1 shows up.
pub fn skeleton(sql: &str) -> String {
    let mut out = String::new();
    let mut chars = sql.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' | '"' | '`' => {
                // A quoted literal (or identifier) — collapse to a placeholder.
                let quote = c;
                out.push('?');
                for n in chars.by_ref() {
                    if n == quote {
                        break;
                    }
                }
            }
            c if c.is_ascii_digit() => {
                let prev = out.chars().last().unwrap_or(' ');
                if prev.is_alphanumeric() || prev == '_' {
                    // Part of an identifier like `col2`.
                    out.push(c);
                } else {
                    out.push('?');
                    while let Some(&n) = chars.peek() {
                        if n.is_ascii_digit() || n == '.' {
                            chars.next();
                        } else {
                            break;
                        }
                    }
                }
            }
            c if c.is_whitespace() => {
                if !out.ends_with(' ') {
                    out.push(' ');
                }
            }
            c => out.push(c.to_ascii_lowercase()),
        }
    }

    let mut s = out.trim().to_string();
    // Collapse `?, ?, ?` (and `?,?`) runs to a single `?`.
    loop {
        let next = s.replace("?, ?", "?").replace("?,?", "?");
        if next == s {
            break s;
        }
        s = next;
    }
}

#[cfg(test)]
mod tests {
    use super::{evidence_markdown, RouteEvidence};

    fn metrics(status: u16, ms: f64, queries: usize, n1: bool) -> super::RequestMetrics {
        let sample = super::RequestSample {
            status,
            duration_ms: ms,
            queries: (0..queries)
                .map(|i| super::Query {
                    // All the same shape when we want an N+1, distinct otherwise.
                    sql: if n1 {
                        "select * from items where id = 1".into()
                    } else {
                        format!("select * from t{i}")
                    },
                    duration_ms: 1.0,
                })
                .collect(),
            response_shape: None,
        };
        super::metrics_of(&sample)
    }

    #[test]
    fn a_before_and_after_row_shows_the_change_not_the_state() {
        let rows = vec![RouteEvidence {
            label: "GET /orders".into(),
            baseline: Some(metrics(200, 340.0, 42, true)),
            metrics: Some(metrics(200, 95.0, 4, false)),
            queries_visible: true,
            note: None,
        }];
        let md = evidence_markdown(&rows, 0, &[]);
        assert!(md.contains("| Verdict |"), "{md}");
        assert!(md.contains("340 → 95 ms"), "{md}");
        assert!(md.contains("42 → 4"), "{md}");
        assert!(
            md.contains("**removed**"),
            "n+1 removal must be called out: {md}"
        );
        assert!(md.contains("improved"), "{md}");
    }

    #[test]
    fn a_regression_and_an_introduced_n1_are_called_out() {
        let rows = vec![RouteEvidence {
            label: "GET /orders".into(),
            baseline: Some(metrics(200, 90.0, 4, false)),
            metrics: Some(metrics(200, 350.0, 40, true)),
            queries_visible: true,
            note: None,
        }];
        let md = evidence_markdown(&rows, 0, &[]);
        assert!(md.contains("**introduced**"), "{md}");
        assert!(md.contains("**regressed**"), "{md}");
    }

    #[test]
    fn a_route_that_started_failing_shows_both_statuses_and_breaks() {
        let rows = vec![RouteEvidence {
            label: "GET /orders".into(),
            baseline: Some(metrics(200, 90.0, 4, false)),
            metrics: Some(metrics(500, 5.0, 0, false)),
            queries_visible: true,
            note: None,
        }];
        let md = evidence_markdown(&rows, 0, &[]);
        assert!(md.contains("200 → 500"), "{md}");
        assert!(md.contains("**broke**"), "{md}");
    }

    #[test]
    fn a_route_with_no_baseline_stays_in_the_before_after_table() {
        // One route measured both ways, one only after: the second must not be
        // dropped just because the table gained a Verdict column.
        let rows = vec![
            RouteEvidence {
                label: "GET /orders".into(),
                baseline: Some(metrics(200, 340.0, 42, true)),
                metrics: Some(metrics(200, 95.0, 4, false)),
                queries_visible: true,
                note: None,
            },
            RouteEvidence {
                label: "GET /new-page".into(),
                baseline: None,
                metrics: Some(metrics(200, 20.0, 2, false)),
                queries_visible: true,
                note: Some("added by this change".into()),
            },
        ];
        let md = evidence_markdown(&rows, 0, &[]);
        assert!(md.contains("`GET /new-page`"), "{md}");
        assert!(md.contains("added by this change"), "{md}");
        assert!(md.contains("Measured 2 of 2"), "{md}");
    }

    #[test]
    fn comparison_is_available_when_both_halves_exist() {
        let both = RouteEvidence {
            label: "x".into(),
            baseline: Some(metrics(200, 100.0, 10, false)),
            metrics: Some(metrics(200, 50.0, 5, false)),
            queries_visible: true,
            note: None,
        };
        assert_eq!(
            both.comparison().map(|c| c.verdict),
            Some(Verdict::Improved)
        );
        let after_only = RouteEvidence {
            label: "x".into(),
            baseline: None,
            metrics: Some(metrics(200, 50.0, 5, false)),
            queries_visible: true,
            note: None,
        };
        assert!(after_only.comparison().is_none());
    }

    #[test]
    fn evidence_reports_what_was_measured_and_what_was_not() {
        let rows = vec![
            RouteEvidence {
                label: "GET /orders".into(),
                metrics: Some(metrics(200, 95.0, 4, false)),
                baseline: None,
                queries_visible: true,
                note: None,
            },
            RouteEvidence {
                label: "PATCH /orders/{order}".into(),
                metrics: None,
                baseline: None,
                queries_visible: true,
                note: Some("not replayed: would write".into()),
            },
        ];
        let md = evidence_markdown(&rows, 3, &[]);
        assert!(
            md.contains("| `GET /orders` | 200 | 95 ms | 4 | no |"),
            "{md}"
        );
        assert!(md.contains("not replayed: would write"), "{md}");
        assert!(md.contains("Measured 1 of 2 attributed route(s)"), "{md}");
        assert!(md.contains("3 changed file(s) could not be traced"), "{md}");
    }

    #[test]
    fn an_n_plus_one_is_called_out() {
        let rows = vec![RouteEvidence {
            label: "GET /orders".into(),
            metrics: Some(metrics(200, 340.0, 42, true)),
            baseline: None,
            queries_visible: true,
            note: None,
        }];
        assert!(evidence_markdown(&rows, 0, &[]).contains("**yes**"));
    }

    #[test]
    fn a_broken_route_shows_its_status() {
        let rows = vec![RouteEvidence {
            label: "GET /orders".into(),
            metrics: Some(metrics(500, 12.0, 0, false)),
            baseline: None,
            queries_visible: true,
            note: None,
        }];
        assert!(evidence_markdown(&rows, 0, &[]).contains("| 500 |"));
    }

    #[test]
    fn nothing_traced_says_so_rather_than_going_quiet() {
        let md = evidence_markdown(&[], 7, &[]);
        assert!(md.contains("No route could be traced"), "{md}");
        assert!(md.contains("7 changed file(s)"), "{md}");
    }

    #[test]
    fn invisible_queries_are_blank_not_zero() {
        // Without Clockwork the replay sees no queries. Printing "42 → 0" or
        // "N+1: no" would be a confident lie about data we never had.
        let rows = vec![RouteEvidence {
            label: "GET /orders".into(),
            baseline: Some(metrics(200, 340.0, 0, false)),
            metrics: Some(metrics(200, 95.0, 0, false)),
            queries_visible: false,
            note: None,
        }];
        let md = evidence_markdown(&rows, 0, &[]);
        assert!(md.contains("340 → 95 ms"), "timing is still real: {md}");
        assert!(md.contains("| — |"), "query cell must be blank: {md}");
        assert!(md.contains("not visible"), "{md}");
        assert!(
            !md.contains("0 → 0"),
            "must not report a count it never had: {md}"
        );
    }

    #[test]
    fn invisible_queries_are_blank_in_the_after_only_table_too() {
        let rows = vec![RouteEvidence {
            label: "GET /orders".into(),
            baseline: None,
            metrics: Some(metrics(200, 95.0, 0, false)),
            queries_visible: false,
            note: None,
        }];
        let md = evidence_markdown(&rows, 0, &[]);
        assert!(md.contains("not visible"), "{md}");
        assert!(!md.contains("| 0 |"), "{md}");
    }

    #[test]
    fn a_caveat_is_stated_next_to_the_numbers() {
        let rows = vec![RouteEvidence {
            label: "GET /orders".into(),
            baseline: Some(metrics(200, 340.0, 42, true)),
            metrics: Some(metrics(200, 95.0, 4, false)),
            queries_visible: true,
            note: None,
        }];
        let md = evidence_markdown(
            &rows,
            0,
            &["the changeset includes a migration".to_string()],
        );
        assert!(md.contains("> the changeset includes a migration"), "{md}");
        // After the table, so it qualifies numbers the reader has already seen.
        assert!(md.find("340 → 95").unwrap() < md.find("> the changeset").unwrap());
    }

    #[test]
    fn no_changes_at_all_produces_no_section() {
        assert_eq!(evidence_markdown(&[], 0, &[]), "");
    }

    use super::*;

    fn q(sql: &str, ms: f64) -> Query {
        Query {
            sql: sql.to_string(),
            duration_ms: ms,
        }
    }

    #[test]
    fn skeleton_normalizes_literals() {
        assert_eq!(
            skeleton("SELECT * FROM users WHERE id = 1"),
            "select * from users where id = ?"
        );
        assert_eq!(
            skeleton("select * from users where id = 42"),
            skeleton("SELECT * FROM users WHERE id = 1")
        );
        assert_eq!(
            skeleton("select * from t where name = 'alice'"),
            skeleton("select * from t where name = 'bob'")
        );
    }

    #[test]
    fn skeleton_collapses_in_lists() {
        assert_eq!(
            skeleton("select * from orders where user_id in (1, 2, 3, 4)"),
            "select * from orders where user_id in (?)"
        );
    }

    #[test]
    fn skeleton_keeps_identifier_digits() {
        assert_eq!(skeleton("select col2 from t"), "select col2 from t");
    }

    #[test]
    fn detects_n_plus_one() {
        let mut sample = RequestSample {
            status: 200,
            duration_ms: 120.0,
            queries: vec![q("select * from users where id = 1", 2.0)],
            response_shape: None,
        };
        for i in 1..=20 {
            sample
                .queries
                .push(q(&format!("select * from orders where user_id = {i}"), 1.5));
        }
        let m = metrics_of(&sample);
        assert_eq!(m.query_count, 21);
        assert!(m.has_n_plus_one());
        let worst = m.worst_n_plus_one().unwrap();
        assert_eq!(worst.count, 20);
        assert!(worst.skeleton.contains("orders"));
    }

    #[test]
    fn no_n_plus_one_for_distinct_queries() {
        let sample = RequestSample {
            status: 200,
            duration_ms: 10.0,
            queries: vec![
                q("select * from users", 1.0),
                q("select * from orders", 1.0),
                q("select * from products", 1.0),
            ],
            response_shape: None,
        };
        let m = metrics_of(&sample);
        assert!(!m.has_n_plus_one());
        assert_eq!(m.distinct_queries, 3);
        assert!(m.groups.is_empty());
    }

    #[test]
    fn compare_flags_n1_fix_and_speedup() {
        let mut before = RequestSample {
            status: 200,
            duration_ms: 300.0,
            queries: vec![q("select * from users where id = 1", 2.0)],
            response_shape: Some("json:users".into()),
        };
        for i in 1..=30 {
            before
                .queries
                .push(q(&format!("select * from orders where user_id = {i}"), 2.0));
        }
        let after = RequestSample {
            status: 200,
            duration_ms: 40.0,
            queries: vec![
                q("select * from users where id = 1", 2.0),
                q("select * from orders where user_id in (1, 2, 3)", 5.0),
            ],
            response_shape: Some("json:users".into()),
        };

        let c = compare(&metrics_of(&before), &metrics_of(&after));
        assert!(c.n1_before && !c.n1_after && c.n1_fixed);
        assert_eq!(c.query_delta, -29);
        assert!(c.faster);
        assert!(!c.shape_changed);
        assert_eq!(c.verdict, Verdict::Improved);
    }

    #[test]
    fn compare_flags_broken_response() {
        let before = metrics_of(&RequestSample {
            status: 200,
            duration_ms: 50.0,
            queries: vec![],
            response_shape: Some("json:ok".into()),
        });
        let after = metrics_of(&RequestSample {
            status: 500,
            duration_ms: 5.0,
            queries: vec![],
            response_shape: Some("error".into()),
        });
        let c = compare(&before, &after);
        assert!(c.status_changed);
        assert!(c.shape_changed);
        assert_eq!(c.verdict, Verdict::Broke);
    }

    #[test]
    fn compare_flags_regression() {
        let before = metrics_of(&RequestSample {
            status: 200,
            duration_ms: 20.0,
            queries: vec![q("select 1", 1.0)],
            response_shape: None,
        });
        let after = metrics_of(&RequestSample {
            status: 200,
            duration_ms: 60.0,
            queries: vec![q("select 1", 1.0), q("select 2", 1.0), q("select 3", 1.0)],
            response_shape: None,
        });
        let c = compare(&before, &after);
        assert!(c.slower);
        assert_eq!(c.query_delta, 2);
        assert_eq!(c.verdict, Verdict::Regressed);
    }

    #[test]
    fn parses_runtime_json() {
        let v = serde_json::json!({
            "status": 201,
            "duration": 42.5,
            "queries": [
                {"query": "select * from users where id = 1", "duration": 2},
                {"query": "select * from orders where user_id = 1", "duration": 3}
            ]
        });
        let s = RequestSample::from_json(&v);
        assert_eq!(s.status, 201);
        assert_eq!(s.duration_ms, 42.5);
        assert_eq!(s.queries.len(), 2);
        assert_eq!(s.queries[1].duration_ms, 3.0);
    }
}

// ── evidence for a changeset ────────────────────────────────────────────

/// What one route measured, for the record attached to a pull request.
#[derive(Debug, Clone, PartialEq)]
pub struct RouteEvidence {
    /// `GET /orders`, as a reviewer would recognise it.
    pub label: String,
    /// The measurement with the change applied. Absent when the route was
    /// attributed but not replayed.
    pub metrics: Option<RequestMetrics>,
    /// The same route measured *without* the change. Present only when the
    /// working tree could be stashed and restored around a second replay —
    /// which is what turns "this route costs 95 ms" into "this change took it
    /// from 340 ms to 95 ms".
    pub baseline: Option<RequestMetrics>,
    /// Whether query data was visible at all. False means the app exposed no
    /// Clockwork, so an empty query list means "we could not see" rather than
    /// "there were none" — and the query and N+1 columns must say so instead of
    /// reporting a confident zero.
    pub queries_visible: bool,
    /// Why it wasn't measured, when it wasn't.
    pub note: Option<String>,
}

impl RouteEvidence {
    /// The before/after verdict, when both halves were measured.
    pub fn comparison(&self) -> Option<Comparison> {
        Some(compare(self.baseline.as_ref()?, self.metrics.as_ref()?))
    }
}

/// How the N+1 column reads when both sides were measured.
fn n1_cell(before: bool, after: bool) -> &'static str {
    match (before, after) {
        (true, false) => "**removed**",
        (false, true) => "**introduced**",
        (true, true) => "yes",
        (false, false) => "no",
    }
}

/// One word for a verdict, for the table.
fn verdict_word(v: Verdict) -> &'static str {
    match v {
        Verdict::Improved => "improved",
        Verdict::NoChange => "no change",
        Verdict::Regressed => "**regressed**",
        Verdict::Broke => "**broke**",
    }
}

/// Render measured routes as the "Evidence" section of a pull request.
///
/// The point of the section is that a reviewer can tell measurement from
/// assertion. Routes that were *not* measured stay in the table with their
/// reason rather than being dropped, and `unattributed` is stated outright:
/// "we measured 3 routes" reads very differently when 40 files went untraced.
pub fn evidence_markdown(
    rows: &[RouteEvidence],
    unattributed: usize,
    caveats: &[String],
) -> String {
    if rows.is_empty() && unattributed == 0 {
        return String::new();
    }
    let mut s = String::from("## Evidence\n\n");
    if rows.is_empty() {
        s.push_str("No route could be traced to these changes, so nothing was measured.\n");
    } else if rows.iter().any(|r| r.baseline.is_some()) {
        // Before/after: here the *change* is the subject, not the current state.
        s.push_str(
            "| Route | Status | Time | Queries | N+1 | Verdict |\n| --- | ---: | ---: | ---: | --- | --- |\n",
        );
        for r in rows {
            match (&r.baseline, &r.metrics) {
                (Some(b), Some(a)) => {
                    let c = compare(b, a);
                    s.push_str(&format!(
                        "| `{}` | {} | {:.0} → {:.0} ms | {} | {} | {} |\n",
                        r.label,
                        if c.status_changed {
                            format!("{} → {}", b.status, a.status)
                        } else {
                            a.status.to_string()
                        },
                        b.ms,
                        a.ms,
                        if r.queries_visible {
                            format!("{} → {}", b.query_count, a.query_count)
                        } else {
                            "—".into()
                        },
                        if r.queries_visible {
                            n1_cell(c.n1_before, c.n1_after).to_string()
                        } else {
                            "not visible".into()
                        },
                        verdict_word(c.verdict),
                    ));
                }
                // Measured after the change but not before — the stash failed,
                // or this route was added by the change and has no "before".
                (None, Some(a)) => s.push_str(&format!(
                    "| `{}` | {} | {:.0} ms | {} | {} | {} |\n",
                    r.label,
                    a.status,
                    a.ms,
                    if r.queries_visible {
                        a.query_count.to_string()
                    } else {
                        "—".into()
                    },
                    if r.queries_visible {
                        if a.has_n_plus_one() { "**yes**" } else { "no" }.to_string()
                    } else {
                        "not visible".into()
                    },
                    r.note.as_deref().unwrap_or("after only"),
                )),
                _ => s.push_str(&format!(
                    "| `{}` | — | — | — | — | {} |\n",
                    r.label,
                    r.note.as_deref().unwrap_or("not measured"),
                )),
            }
        }
    } else {
        s.push_str(
            "| Route | Status | Time | Queries | N+1 |\n| --- | ---: | ---: | ---: | --- |\n",
        );
        for r in rows {
            match &r.metrics {
                Some(m) => s.push_str(&format!(
                    "| `{}` | {} | {:.0} ms | {} | {} |\n",
                    r.label,
                    m.status,
                    m.ms,
                    if r.queries_visible {
                        m.query_count.to_string()
                    } else {
                        "—".into()
                    },
                    if r.queries_visible {
                        if m.has_n_plus_one() { "**yes**" } else { "no" }.to_string()
                    } else {
                        "not visible".into()
                    },
                )),
                None => s.push_str(&format!(
                    "| `{}` | — | — | — | {} |\n",
                    r.label,
                    r.note.as_deref().unwrap_or("not measured"),
                )),
            }
        }
    }
    let measured = rows.iter().filter(|r| r.metrics.is_some()).count();
    s.push_str(&format!(
        "\nMeasured {measured} of {} attributed route(s)",
        rows.len()
    ));
    if unattributed > 0 {
        s.push_str(&format!(
            "; {unattributed} changed file(s) could not be traced to a route"
        ));
    }
    s.push_str(".\n");
    // Anything that makes the numbers less trustworthy is stated next to them,
    // not left for the reader to work out.
    for c in caveats {
        s.push_str(&format!("\n> {c}\n"));
    }
    s
}
