//! Port of the Go goalpost tests — same 14 cases, identical semantics.

use pretty_assertions::assert_eq;

use super::{Condition, analyze, expr_is_looser_than};

fn conds(json: &str) -> Vec<Condition> {
    serde_json::from_str(json).expect("conds parse")
}

// ---------- baseline ----------

#[test]
fn identical_is_clean() {
    let c = conds(r#"[{"type":"quantitative","name":"gm_collapse","expr":"gross_margin < 45"}]"#);
    let r = analyze(&c, &c);
    assert!(!r.weakened, "{r:?}");
    assert!(!r.needs_review);
}

#[test]
fn empty_original_no_weakening() {
    let r = analyze(&[], &conds(r#"[{"type":"quantitative","name":"new","expr":"x < 10"}]"#));
    assert!(!r.weakened);
}

#[test]
fn all_conditions_dropped_is_weakened() {
    let original = conds(r#"[{"type":"quantitative","name":"gm","expr":"gross_margin < 45"}]"#);
    let r = analyze(&original, &[]);
    assert!(r.weakened);
    assert_eq!(r.dropped, vec!["gm"]);
}

// ---------- dropped ----------

#[test]
fn dropped_condition_is_weakened() {
    let original = conds(
        r#"[
        {"type":"quantitative","name":"gm","expr":"gross_margin < 45"},
        {"type":"narrative","name":"hyperscale_cut","assertion":"Top-3 hyperscalers cut capex >15%"}
      ]"#,
    );
    let updated = conds(r#"[{"type":"quantitative","name":"gm","expr":"gross_margin < 45"}]"#);
    let r = analyze(&original, &updated);
    assert!(r.weakened);
    assert_eq!(r.dropped, vec!["hyperscale_cut"]);
}

// ---------- loosened thresholds ----------

#[test]
fn loosening_lt_is_weakened() {
    let original = conds(r#"[{"type":"quantitative","name":"gm","expr":"gross_margin < 45"}]"#);
    let updated = conds(r#"[{"type":"quantitative","name":"gm","expr":"gross_margin < 30"}]"#);
    let r = analyze(&original, &updated);
    assert!(r.weakened, "'< 45' → '< 30' is a loosening: {r:?}");
    assert_eq!(r.loosened, vec!["gm"]);
}

#[test]
fn loosening_gt_is_weakened() {
    let original = conds(r#"[{"type":"quantitative","name":"churn","expr":"churn_rate > 12"}]"#);
    let updated = conds(r#"[{"type":"quantitative","name":"churn","expr":"churn_rate > 20"}]"#);
    let r = analyze(&original, &updated);
    assert!(r.weakened, "'> 12' → '> 20' is a loosening: {r:?}");
}

#[test]
fn tightening_gt_is_clean() {
    let original = conds(r#"[{"type":"quantitative","name":"churn","expr":"churn_rate > 12"}]"#);
    let updated = conds(r#"[{"type":"quantitative","name":"churn","expr":"churn_rate > 8"}]"#);
    let r = analyze(&original, &updated);
    assert!(!r.weakened, "'> 12' → '> 8' is a tightening: {r:?}");
}

#[test]
fn tightening_lt_is_clean() {
    let original = conds(r#"[{"type":"quantitative","name":"gm","expr":"gross_margin < 45"}]"#);
    let updated = conds(r#"[{"type":"quantitative","name":"gm","expr":"gross_margin < 50"}]"#);
    let r = analyze(&original, &updated);
    assert!(!r.weakened);
}

// ---------- added ----------

#[test]
fn added_condition_is_clean() {
    let original = conds(r#"[{"type":"quantitative","name":"gm","expr":"gross_margin < 45"}]"#);
    let updated = conds(
        r#"[
        {"type":"quantitative","name":"gm","expr":"gross_margin < 45"},
        {"type":"quantitative","name":"capex","expr":"capex_growth < 0"}
      ]"#,
    );
    let r = analyze(&original, &updated);
    assert!(!r.weakened, "{r:?}");
    assert_eq!(r.added, vec!["capex"]);
}

// ---------- narrative ----------

#[test]
fn narrative_change_needs_review() {
    let original = conds(
        r#"[{"type":"narrative","name":"moat","assertion":"AMD cannot match HBM scale before 2027"}]"#,
    );
    let updated = conds(
        r#"[{"type":"narrative","name":"moat","assertion":"AMD cannot match HBM scale before mid-2028"}]"#,
    );
    let r = analyze(&original, &updated);
    assert!(r.needs_review);
}

#[test]
fn narrative_identical_no_review() {
    let c = conds(
        r#"[{"type":"narrative","name":"moat","assertion":"AMD cannot match HBM scale before 2027"}]"#,
    );
    let r = analyze(&c, &c);
    assert!(!r.needs_review);
}

// ---------- pure rewrite ----------

#[test]
fn pure_rewrite_flags_review() {
    let original = conds(r#"[{"type":"quantitative","name":"gm","expr":"gross_margin < 45"}]"#);
    let updated = conds(
        r#"[{"type":"quantitative","name":"yoy_rev","expr":"yoy_revenue_growth < 5"}]"#,
    );
    let r = analyze(&original, &updated);
    assert!(r.needs_review);
    assert!(r.weakened, "drop without retain IS weakening: {r:?}");
}

// ---------- mixed signals ----------

#[test]
fn mixed_signals_weakens_if_any_weaken() {
    let original = conds(
        r#"[
        {"type":"quantitative","name":"gm","expr":"gross_margin < 45"},
        {"type":"quantitative","name":"capex","expr":"capex_growth < 0"}
      ]"#,
    );
    let updated = conds(
        r#"[
        {"type":"quantitative","name":"gm","expr":"gross_margin < 30"},
        {"type":"quantitative","name":"capex","expr":"capex_growth < 0"},
        {"type":"quantitative","name":"share_loss","expr":"market_share_pct < 30"}
      ]"#,
    );
    let r = analyze(&original, &updated);
    assert!(r.weakened);
    assert_eq!(r.loosened.len(), 1);
    assert_eq!(r.added.len(), 1);
}

// ---------- expression parsing ----------

#[test]
fn expr_loosening_table() {
    let cases: &[(&str, &str, bool)] = &[
        // < raised RHS = looser
        ("x < 45", "x < 30", true),
        ("x < 45", "x < 50", false),
        ("x < 45", "x < 45", false),
        ("x <= 45", "x <= 30", true),
        // > raised RHS = looser (value must climb further)
        ("x > 12", "x > 20", true),
        ("x > 12", "x > 8", false),
        ("x >= 12", "x >= 20", true),
        // different field → can't determine
        ("x < 45", "y < 45", false),
        // == / != have no looser/stricter
        ("x == 5", "x == 6", false),
    ];
    for (old, new, looser) in cases {
        assert_eq!(
            expr_is_looser_than(new, old),
            *looser,
            "exprIsLooserThan({new:?} vs {old:?})"
        );
    }
}
