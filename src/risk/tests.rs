//! Port of the Go risk tests. Same 10 cases, same numbers.

use pretty_assertions::assert_eq;

use super::{Config, Intent, Portfolio, Position, evaluate};

const SEED_RISK_CONFIG: &str = r#"{
  "single_name_delta_notional_pct": 15,
  "options_premium_aggregate_pct": 20,
  "cash_floor_pct": 20,
  "drawdown_brake": [
    {"at_pct": -10, "size_mult": 0.5},
    {"at_pct": -20, "halt_new": true}
  ],
  "subsector_cluster_pct": 35,
  "concurrent_positions": 7
}"#;

fn cfg() -> Config {
    serde_json::from_str(SEED_RISK_CONFIG).unwrap()
}

fn base_portfolio() -> Portfolio {
    Portfolio { total_value: 100_000.0, cash_pct: 50.0, drawdown_pct: 0.0 }
}

fn contains(rs: &[String], sub: &str) -> bool {
    rs.iter().any(|r| r.contains(sub))
}

#[test]
fn passes_under_all_limits() {
    let d = evaluate(
        &Intent {
            symbol: "NVDA".into(),
            cluster: "logic_accelerators".into(),
            instrument: "equity".into(),
            delta_notional: 8_000.0,
            ..Default::default()
        },
        &[],
        base_portfolio(),
        &cfg(),
    );
    assert!(!d.veto, "clean entry must not veto: {d:?}");
    assert_eq!(d.size_mult, 1.0);
}

#[test]
fn vetoes_over_single_name_cap() {
    // 12% existing + 4% new = 16% > 15%.
    let existing = vec![Position {
        symbol: "NVDA".into(),
        cluster: "logic_accelerators".into(),
        instrument: "equity".into(),
        delta_notional: 12_000.0,
        ..Default::default()
    }];
    let d = evaluate(
        &Intent {
            symbol: "NVDA".into(),
            cluster: "logic_accelerators".into(),
            instrument: "equity".into(),
            delta_notional: 4_000.0,
            ..Default::default()
        },
        &existing,
        base_portfolio(),
        &cfg(),
    );
    assert!(d.veto);
    assert!(contains(&d.reasons, "single_name_delta_notional_pct"));
}

#[test]
fn allows_at_exactly_single_name_cap() {
    let existing = vec![Position { symbol: "NVDA".into(), delta_notional: 10_000.0, ..Default::default() }];
    let d = evaluate(
        &Intent { symbol: "NVDA".into(), instrument: "equity".into(), delta_notional: 5_000.0, ..Default::default() },
        &existing,
        base_portfolio(),
        &cfg(),
    );
    assert!(!d.veto, "exactly 15% should pass: {d:?}");
}

#[test]
fn vetoes_over_options_aggregate() {
    // 10k + 8k existing + 3k new = 21% > 20%.
    let existing = vec![
        Position { symbol: "AAPL".into(), instrument: "leaps".into(), premium_at_risk: 10_000.0, ..Default::default() },
        Position { symbol: "MU".into(),   instrument: "leaps".into(), premium_at_risk:  8_000.0, ..Default::default() },
    ];
    let d = evaluate(
        &Intent { symbol: "NVDA".into(), instrument: "leaps".into(), premium_at_risk: 3_000.0, ..Default::default() },
        &existing,
        base_portfolio(),
        &cfg(),
    );
    assert!(d.veto);
    assert!(contains(&d.reasons, "options_premium_aggregate_pct"));
}

#[test]
fn vetoes_below_cash_floor() {
    let mut p = base_portfolio();
    p.cash_pct = 18.0;
    let d = evaluate(
        &Intent { symbol: "NVDA".into(), instrument: "equity".into(), delta_notional: 1_000.0, ..Default::default() },
        &[],
        p,
        &cfg(),
    );
    assert!(d.veto);
    assert!(contains(&d.reasons, "cash_floor_pct"));
}

#[test]
fn vetoes_if_entry_would_breach_cash_floor() {
    let mut p = base_portfolio();
    p.cash_pct = 22.0;
    let d = evaluate(
        &Intent { symbol: "NVDA".into(), instrument: "equity".into(), delta_notional: 5_000.0, ..Default::default() },
        &[],
        p,
        &cfg(),
    );
    assert!(d.veto, "entry that would drop cash below 20% must veto: {d:?}");
}

#[test]
fn drawdown_brake_halves_size() {
    let mut p = base_portfolio();
    p.drawdown_pct = -12.0;
    let d = evaluate(
        &Intent { symbol: "NVDA".into(), instrument: "equity".into(), delta_notional: 5_000.0, ..Default::default() },
        &[],
        p,
        &cfg(),
    );
    assert!(!d.veto, "drawdown -12% should warn + scale, not veto: {d:?}");
    assert_eq!(d.size_mult, 0.5);
    assert!(!d.warnings.is_empty());
}

#[test]
fn drawdown_brake_halts_below_second_tier() {
    let mut p = base_portfolio();
    p.drawdown_pct = -21.0;
    let d = evaluate(
        &Intent { symbol: "NVDA".into(), instrument: "equity".into(), delta_notional: 1_000.0, ..Default::default() },
        &[],
        p,
        &cfg(),
    );
    assert!(d.veto);
}

#[test]
fn subsector_cluster_is_warning_not_veto() {
    let existing = vec![
        Position { symbol: "NVDA".into(), cluster: "logic_accelerators".into(), instrument: "equity".into(), delta_notional: 14_000.0, ..Default::default() },
        Position { symbol: "AMD".into(),  cluster: "logic_accelerators".into(), instrument: "equity".into(), delta_notional: 14_000.0, ..Default::default() },
        Position { symbol: "AVGO".into(), cluster: "logic_accelerators".into(), instrument: "equity".into(), delta_notional:  7_000.0, ..Default::default() },
    ];
    let d = evaluate(
        &Intent {
            symbol: "TSM".into(),
            cluster: "logic_accelerators".into(),
            instrument: "equity".into(),
            delta_notional: 2_000.0,
            ..Default::default()
        },
        &existing,
        base_portfolio(),
        &cfg(),
    );
    assert!(!d.veto, "sub-sector cap is SOFT");
    assert!(!d.warnings.is_empty());
}

#[test]
fn concurrent_positions_cap() {
    let pos: Vec<Position> = (0..7)
        .map(|i| Position {
            symbol: format!("POS{i}"),
            instrument: "equity".into(),
            delta_notional: 1_000.0,
            ..Default::default()
        })
        .collect();
    let d = evaluate(
        &Intent { symbol: "EIGHTH".into(), instrument: "equity".into(), delta_notional: 1_000.0, ..Default::default() },
        &pos,
        base_portfolio(),
        &cfg(),
    );
    assert!(d.veto);
    assert!(contains(&d.reasons, "concurrent_positions"));
}
