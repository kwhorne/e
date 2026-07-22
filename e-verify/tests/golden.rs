//! Golden-file test: replay a recorded before/after request pair (an N+1 fixed
//! by eager-loading) through the measurement core and assert the verdict. This
//! exercises parsing + metrics + comparison end to end, without a DB or a GUI.

use e_verify::{compare, metrics_of, RequestSample, Verdict};

fn sample(json: &str) -> RequestSample {
    let v: serde_json::Value = serde_json::from_str(json).unwrap();
    RequestSample::from_json(&v)
}

#[test]
fn eager_load_fix_is_reported_as_improved() {
    let before = metrics_of(&sample(include_str!("fixtures/before.json")));
    let after = metrics_of(&sample(include_str!("fixtures/after.json")));

    // Before: a users lookup + 15 near-identical per-user order queries = N+1.
    assert_eq!(before.query_count, 16);
    assert!(before.has_n_plus_one());
    assert_eq!(before.worst_n_plus_one().unwrap().count, 15);

    // After: users lookup + one batched `IN (...)` query. No N+1.
    assert_eq!(after.query_count, 2);
    assert!(!after.has_n_plus_one());

    let c = compare(&before, &after);
    assert!(c.n1_fixed, "the N+1 should be reported as fixed");
    assert_eq!(c.query_delta, -14);
    assert!(c.faster, "284ms -> 38ms should read as faster");
    assert!(!c.shape_changed, "same response shape -> not broken");
    assert_eq!(c.verdict, Verdict::Improved);
}
