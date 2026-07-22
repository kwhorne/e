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
