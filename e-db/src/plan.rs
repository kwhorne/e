//! A query plan as a tree (DB-602): each engine's own structured plan —
//! SQLite's `EXPLAIN QUERY PLAN` rows, MySQL's `EXPLAIN FORMAT=JSON` (both
//! of its JSON layouts), PostgreSQL's `EXPLAIN (FORMAT JSON)` — read into
//! one shape, with full scans marked and, where the engine gives them, each
//! node's estimated cost and rows.

use serde_json::Value;

/// One step of a plan.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlanNode {
    /// What it does: `Seq Scan on orders`, `Full scan on orders`, `SCAN orders`.
    pub label: String,
    /// The table it reads, if it reads one.
    pub table: Option<String>,
    /// Its conditions, keys and the like, a line each.
    pub details: Vec<String>,
    /// Estimated cost, as the engine gives it: PostgreSQL's counts the
    /// steps below, MySQL's a table's counts the tables joined before it.
    pub cost: Option<f64>,
    /// Estimated cost of this step alone, where the engine lets it be told.
    pub own_cost: Option<f64>,
    /// Estimated rows.
    pub rows: Option<f64>,
    /// It reads a whole table.
    pub full_scan: bool,
    pub children: Vec<PlanNode>,
}

impl PlanNode {
    fn new(label: impl Into<String>) -> Self {
        PlanNode {
            label: label.into(),
            ..Default::default()
        }
    }

    /// Every step, depth first, with its depth.
    pub fn flatten(&self) -> Vec<(usize, &PlanNode)> {
        fn walk<'a>(n: &'a PlanNode, depth: usize, out: &mut Vec<(usize, &'a PlanNode)>) {
            out.push((depth, n));
            for c in &n.children {
                walk(c, depth + 1, out);
            }
        }
        let mut out = Vec::new();
        walk(self, 0, &mut out);
        out
    }

    /// The step that costs the most on its own, when costs are known and
    /// there's more than one step.
    pub fn costliest(&self) -> Option<&PlanNode> {
        let all = self.flatten();
        if all.len() < 2 {
            return None;
        }
        all.into_iter()
            .map(|(_, n)| n)
            .filter(|n| n.own_cost.is_some_and(|c| c > 0.0))
            .max_by(|a, b| a.own_cost.partial_cmp(&b.own_cost).unwrap())
    }
}

/// SQLite's `EXPLAIN QUERY PLAN` rows (`id`, `parent`, `notused`,
/// `detail`) as a tree under one root.
pub fn from_sqlite(rows: &[Vec<Option<String>>]) -> Option<PlanNode> {
    let parsed: Vec<(i64, i64, String)> = rows
        .iter()
        .filter_map(|r| {
            let num = |i: usize| r.get(i)?.as_deref()?.trim().parse::<i64>().ok();
            Some((num(0)?, num(1)?, r.last()?.clone()?))
        })
        .collect();
    if parsed.is_empty() {
        return None;
    }
    fn build(parent: i64, rows: &[(i64, i64, String)]) -> Vec<PlanNode> {
        rows.iter()
            .filter(|(_, p, _)| *p == parent)
            .map(|(id, _, detail)| {
                let d = detail.trim();
                let words: Vec<&str> = d.split_whitespace().collect();
                let table = match words.as_slice() {
                    ["SCAN" | "SEARCH", "TABLE", t, ..] | ["SCAN" | "SEARCH", t, ..] => {
                        Some(t.to_string())
                    }
                    _ => None,
                };
                PlanNode {
                    label: d.to_string(),
                    full_scan: d.starts_with("SCAN") && !d.contains("USING"),
                    table,
                    children: build(*id, rows),
                    ..Default::default()
                }
            })
            .collect()
    }
    let mut root = PlanNode::new("Query plan");
    root.children = build(0, &parsed);
    Some(root)
}

/// PostgreSQL's `EXPLAIN (FORMAT JSON)`: `[{"Plan": {…}}]`.
pub fn from_postgres(json: &str) -> Option<PlanNode> {
    let v: Value = serde_json::from_str(json).ok()?;
    let plan = v.get(0)?.get("Plan")?;
    let mut node = pg_node(plan);
    own_from_totals(&mut node);
    Some(node)
}

/// Each step's own cost, where costs count the steps below: its cost less
/// its children's.
fn own_from_totals(n: &mut PlanNode) {
    let below: f64 = n.children.iter().filter_map(|c| c.cost).sum();
    n.own_cost = n.cost.map(|c| (c - below).max(0.0));
    for c in &mut n.children {
        own_from_totals(c);
    }
}

fn pg_node(p: &Value) -> PlanNode {
    let s = |k: &str| p.get(k).and_then(Value::as_str);
    let kind = s("Node Type").unwrap_or("?");
    let mut label = match s("Join Type") {
        Some(j) if kind.ends_with("Join") || kind == "Nested Loop" => format!("{kind} ({j})"),
        _ => kind.to_string(),
    };
    let table = s("Relation Name").map(str::to_string);
    if let Some(t) = &table {
        label.push_str(&format!(" on {t}"));
        if let Some(alias) = s("Alias").filter(|a| a != t) {
            label.push_str(&format!(" {alias}"));
        }
    }
    if let Some(i) = s("Index Name") {
        label.push_str(&format!(" using {i}"));
    }
    let mut details = Vec::new();
    for (key, name) in [
        ("Index Cond", "index condition"),
        ("Recheck Cond", "recheck"),
        ("Hash Cond", "hash condition"),
        ("Merge Cond", "merge condition"),
        ("Join Filter", "join filter"),
        ("Filter", "filter"),
    ] {
        if let Some(c) = s(key) {
            details.push(format!("{name}: {c}"));
        }
    }
    for (key, name) in [("Sort Key", "sort"), ("Group Key", "group")] {
        if let Some(keys) = p.get(key).and_then(Value::as_array) {
            let keys: Vec<&str> = keys.iter().filter_map(Value::as_str).collect();
            details.push(format!("{name}: {}", keys.join(", ")));
        }
    }
    PlanNode {
        full_scan: kind == "Seq Scan",
        label,
        table,
        details,
        cost: p.get("Total Cost").and_then(Value::as_f64),
        own_cost: None,
        rows: p.get("Plan Rows").and_then(Value::as_f64),
        children: p
            .get("Plans")
            .and_then(Value::as_array)
            .map(|ps| ps.iter().map(pg_node).collect())
            .unwrap_or_default(),
    }
}

/// MySQL's `EXPLAIN FORMAT=JSON`: the classic layout (`query_block`, with
/// `table`, `nested_loop`, `ordering_operation` …) or the newer one
/// (`explain_json_format_version=2`: `operation`, `inputs`).
pub fn from_mysql(json: &str) -> Option<PlanNode> {
    let v: Value = serde_json::from_str(json).ok()?;
    if let Some(qb) = v.get("query_block") {
        return Some(mysql_query_block(qb));
    }
    v.get("operation").map(|_| {
        let mut node = mysql_v2(&v);
        own_from_totals(&mut node);
        node
    })
}

/// A number MySQL may write as a string (`"cost_info": {"query_cost": "1.25"}`).
fn num(v: Option<&Value>) -> Option<f64> {
    let v = v?;
    v.as_f64().or_else(|| v.as_str()?.parse().ok())
}

fn mysql_query_block(qb: &Value) -> PlanNode {
    let id = qb.get("select_id").and_then(Value::as_i64);
    let mut node = PlanNode::new(match id {
        Some(id) => format!("SELECT #{id}"),
        None => "SELECT".to_string(),
    });
    node.cost = num(qb.get("cost_info").and_then(|c| c.get("query_cost")));
    if let Some(m) = qb.get("message").and_then(Value::as_str) {
        node.details.push(m.to_string());
    }
    node.children = mysql_steps(qb);
    node
}

/// The steps an object holds, in MySQL's classic layout.
fn mysql_steps(v: &Value) -> Vec<PlanNode> {
    let mut out = Vec::new();
    if let Some(t) = v.get("table") {
        out.push(mysql_table(t));
    }
    if let Some(loops) = v.get("nested_loop").and_then(Value::as_array) {
        let mut n = PlanNode::new("Nested loop");
        n.children = loops.iter().flat_map(mysql_steps).collect();
        n.cost = n
            .children
            .iter()
            .filter_map(|c| c.cost)
            .fold(None, |m, c| Some(m.map_or(c, |m: f64| m.max(c))));
        out.push(n);
    }
    let wrappers = [
        ("ordering_operation", "Sort"),
        ("grouping_operation", "Group"),
        ("duplicates_removal", "Distinct"),
        ("windowing", "Window"),
    ];
    for (key, name) in wrappers {
        if let Some(op) = v.get(key) {
            let mut label = name.to_string();
            if op.get("using_filesort").and_then(Value::as_bool) == Some(true) {
                label.push_str(" (filesort)");
            }
            if op.get("using_temporary_table").and_then(Value::as_bool) == Some(true) {
                label.push_str(" (temporary table)");
            }
            let mut n = PlanNode::new(label);
            n.children = mysql_steps(op);
            out.push(n);
        }
    }
    if let Some(u) = v.get("union_result") {
        let mut n = PlanNode::new("Union");
        n.children = u
            .get("query_specifications")
            .and_then(Value::as_array)
            .map(|qs| {
                qs.iter()
                    .filter_map(|q| q.get("query_block"))
                    .map(mysql_query_block)
                    .collect()
            })
            .unwrap_or_default();
        out.push(n);
    }
    out
}

fn mysql_table(t: &Value) -> PlanNode {
    let s = |k: &str| t.get(k).and_then(Value::as_str);
    let name = s("table_name").unwrap_or("?").to_string();
    let access = s("access_type").unwrap_or("");
    let key = s("key");
    let label = match (access, key) {
        ("ALL", _) => format!("Full scan on {name}"),
        ("index", Some(k)) => format!("Index scan on {name} using {k}"),
        ("range", Some(k)) => format!("Range on {name} using {k}"),
        ("ref" | "eq_ref" | "ref_or_null", Some(k)) => format!("Lookup on {name} using {k}"),
        ("const" | "system", _) => format!("Single row from {name}"),
        (a, Some(k)) => format!("{a} on {name} using {k}"),
        (a, None) if !a.is_empty() => format!("{a} on {name}"),
        _ => name.clone(),
    };
    let mut details = Vec::new();
    if let Some(c) = s("attached_condition") {
        details.push(format!("filter: {c}"));
    }
    if let Some(keys) = t.get("possible_keys").and_then(Value::as_array) {
        let keys: Vec<&str> = keys.iter().filter_map(Value::as_str).collect();
        if !keys.is_empty() && key.is_none() {
            details.push(format!("possible keys, none used: {}", keys.join(", ")));
        }
    }
    let mut children = Vec::new();
    if let Some(qb) = t
        .get("materialized_from_subquery")
        .and_then(|m| m.get("query_block"))
    {
        children.push(mysql_query_block(qb));
    }
    for sub in t
        .get("attached_subqueries")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(qb) = sub.get("query_block") {
            children.push(mysql_query_block(qb));
        }
    }
    let cost_info = t.get("cost_info");
    let part = |k: &str| num(cost_info.and_then(|c| c.get(k)));
    PlanNode {
        label,
        table: Some(name),
        details,
        cost: part("prefix_cost"),
        // Reading the table and checking its rows; prefix_cost adds the
        // tables joined before it.
        own_cost: match (part("read_cost"), part("eval_cost")) {
            (None, None) => None,
            (r, e) => Some(r.unwrap_or(0.0) + e.unwrap_or(0.0)),
        },
        rows: num(t.get("rows_examined_per_scan")),
        full_scan: access == "ALL",
        children,
    }
}

fn mysql_v2(v: &Value) -> PlanNode {
    let s = |k: &str| v.get(k).and_then(Value::as_str);
    let label = s("operation").unwrap_or("?").to_string();
    let mut details = Vec::new();
    // A Filter step's condition is its name already.
    if let Some(c) = s("condition").filter(|_| !label.starts_with("Filter")) {
        details.push(format!("filter: {c}"));
    }
    PlanNode {
        label,
        table: s("table_name").map(str::to_string),
        details,
        cost: num(v.get("estimated_total_cost")),
        own_cost: None,
        rows: num(v.get("estimated_rows")),
        full_scan: s("access_type") == Some("table"),
        children: v
            .get("inputs")
            .and_then(Value::as_array)
            .map(|i| i.iter().map(mysql_v2).collect())
            .unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(n: &PlanNode) -> Vec<String> {
        n.flatten()
            .into_iter()
            .map(|(d, n)| format!("{}{}", "  ".repeat(d), n.label))
            .collect()
    }

    #[test]
    fn sqlite_rows_become_a_tree() {
        let row = |id: &str, parent: &str, detail: &str| {
            vec![
                Some(id.to_string()),
                Some(parent.to_string()),
                Some("0".to_string()),
                Some(detail.to_string()),
            ]
        };
        let plan = from_sqlite(&[
            row("3", "0", "SCAN orders"),
            row("5", "0", "SEARCH users USING INTEGER PRIMARY KEY (rowid=?)"),
            row("9", "0", "CORRELATED SCALAR SUBQUERY 1"),
            row("12", "9", "SCAN TABLE comments"),
            row("20", "0", "USE TEMP B-TREE FOR ORDER BY"),
        ])
        .unwrap();
        assert_eq!(
            labels(&plan),
            [
                "Query plan",
                "  SCAN orders",
                "  SEARCH users USING INTEGER PRIMARY KEY (rowid=?)",
                "  CORRELATED SCALAR SUBQUERY 1",
                "    SCAN TABLE comments",
                "  USE TEMP B-TREE FOR ORDER BY",
            ]
        );
        let scans: Vec<_> = plan
            .flatten()
            .into_iter()
            .filter(|(_, n)| n.full_scan)
            .map(|(_, n)| n.table.clone().unwrap())
            .collect();
        assert_eq!(scans, ["orders", "comments"]);
        assert_eq!(plan.costliest(), None);
    }

    #[test]
    fn postgres_json_becomes_a_tree() {
        let json = r#"[{"Plan": {
            "Node Type": "Sort", "Total Cost": 30.5, "Plan Rows": 6,
            "Sort Key": ["orders.created_at DESC"],
            "Plans": [{
                "Node Type": "Hash Join", "Join Type": "Inner", "Total Cost": 29.0,
                "Plan Rows": 6, "Hash Cond": "(orders.user_id = users.id)",
                "Plans": [
                    {"Node Type": "Seq Scan", "Relation Name": "orders", "Alias": "orders",
                     "Total Cost": 25.0, "Plan Rows": 6,
                     "Filter": "(status = 'paid'::text)"},
                    {"Node Type": "Hash", "Total Cost": 1.02, "Plan Rows": 2,
                     "Plans": [{"Node Type": "Index Scan", "Relation Name": "users",
                                "Alias": "u", "Index Name": "users_pkey",
                                "Total Cost": 1.02, "Plan Rows": 2}]}
                ]
            }]
        }}]"#;
        let plan = from_postgres(json).unwrap();
        assert_eq!(
            labels(&plan),
            [
                "Sort",
                "  Hash Join (Inner)",
                "    Seq Scan on orders",
                "    Hash",
                "      Index Scan on users u using users_pkey",
            ]
        );
        assert_eq!(plan.details, ["sort: orders.created_at DESC"]);
        let scan = &plan.children[0].children[0];
        assert!(scan.full_scan);
        assert_eq!(scan.details, ["filter: (status = 'paid'::text)"]);
        assert_eq!(scan.rows, Some(6.0));
        // The scan costs the most on its own: 25 of the join's 29.
        assert_eq!(plan.costliest().unwrap().label, "Seq Scan on orders");
        assert!(from_postgres("not json").is_none());
    }

    #[test]
    fn mysql_classic_json_becomes_a_tree() {
        let json = r#"{"query_block": {
            "select_id": 1, "cost_info": {"query_cost": "31.25"},
            "ordering_operation": {"using_filesort": true,
              "nested_loop": [
                {"table": {"table_name": "orders", "access_type": "ALL",
                  "possible_keys": ["orders_user_id_foreign"],
                  "rows_examined_per_scan": 300,
                  "cost_info": {"read_cost": "0.25", "eval_cost": "30.00",
                                "prefix_cost": "30.25"},
                  "attached_condition": "(`shop`.`orders`.`status` = 'paid')"}},
                {"table": {"table_name": "users", "access_type": "eq_ref",
                  "key": "PRIMARY", "rows_examined_per_scan": 1,
                  "cost_info": {"read_cost": "0.90", "eval_cost": "0.10",
                                "prefix_cost": "31.25"}}}
              ]}
        }}"#;
        let plan = from_mysql(json).unwrap();
        assert_eq!(
            labels(&plan),
            [
                "SELECT #1",
                "  Sort (filesort)",
                "    Nested loop",
                "      Full scan on orders",
                "      Lookup on users using PRIMARY",
            ]
        );
        assert_eq!(plan.cost, Some(31.25));
        // Its own cost, not the join's running total, makes the scan the
        // costliest step.
        assert_eq!(plan.costliest().unwrap().label, "Full scan on orders");
        let orders = &plan.children[0].children[0].children[0];
        assert!(orders.full_scan);
        assert_eq!(orders.rows, Some(300.0));
        assert_eq!(
            orders.details,
            [
                "filter: (`shop`.`orders`.`status` = 'paid')",
                "possible keys, none used: orders_user_id_foreign",
            ]
        );
    }

    #[test]
    fn mysql_v2_json_becomes_a_tree() {
        let json = r#"{"query": "…", "operation": "Sort: orders.created_at DESC",
            "estimated_total_cost": 30.5, "estimated_rows": 30,
            "inputs": [{"operation": "Filter: (orders.status = 'paid')",
              "estimated_total_cost": 30.5, "estimated_rows": 30,
              "inputs": [{"operation": "Table scan on orders", "access_type": "table",
                "table_name": "orders", "estimated_total_cost": 30.5,
                "estimated_rows": 300}]}]}"#;
        let plan = from_mysql(json).unwrap();
        assert_eq!(
            labels(&plan),
            [
                "Sort: orders.created_at DESC",
                "  Filter: (orders.status = 'paid')",
                "    Table scan on orders",
            ]
        );
        let scan = &plan.children[0].children[0];
        assert!(scan.full_scan);
        assert_eq!(scan.table.as_deref(), Some("orders"));
    }
}
