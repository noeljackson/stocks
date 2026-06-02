//! Port of the Go regime tests + expression table.

use std::collections::HashMap;

use pretty_assertions::assert_eq;

use super::expr::eval;
use super::service::{regime_evidence_polarity, regime_evidence_strength, regime_evidence_summary};
use super::{Config, classify};
use crate::platform::domain::Regime;

const SEED_CONFIG: &str = r#"{
  "states": ["risk_on","neutral","risk_off"],
  "rules": {
    "risk_on":  {"spx_vs_sma12m": ">0", "hy_oas_pct": "<5", "breadth_pct_above_200d": ">50"},
    "risk_off": {"spx_vs_sma12m": "<0", "hy_oas_pct": ">7", "breadth_pct_above_200d": "<35"}
  },
  "capitulation": {"any_of": ["vix>25", "put_call>1.10"]}
}"#;

fn cfg() -> Config {
    serde_json::from_str(SEED_CONFIG).unwrap()
}

fn inputs(pairs: &[(&str, f64)]) -> HashMap<String, f64> {
    pairs.iter().map(|(k, v)| ((*k).to_string(), *v)).collect()
}

#[test]
fn classify_risk_on_full_signals() {
    let r = classify(
        &cfg(),
        &inputs(&[
            ("spx_vs_sma12m", 5.0),
            ("hy_oas_pct", 3.2),
            ("breadth_pct_above_200d", 65.0),
            ("vix", 14.0),
        ]),
    );
    assert_eq!(r.regime, Regime::RiskOn);
    assert!(!r.capitulation);
}

#[test]
fn classify_risk_off() {
    let r = classify(
        &cfg(),
        &inputs(&[
            ("spx_vs_sma12m", -2.0),
            ("hy_oas_pct", 8.5),
            ("breadth_pct_above_200d", 30.0),
            ("vix", 30.0),
        ]),
    );
    assert_eq!(r.regime, Regime::RiskOff);
    assert!(r.capitulation);
}

#[test]
fn classify_degrades_to_neutral_without_spx() {
    let r = classify(&cfg(), &inputs(&[("hy_oas_pct", 3.0)]));
    assert_eq!(r.regime, Regime::Neutral);
}

#[test]
fn classify_tie_goes_neutral() {
    let r = classify(&cfg(), &inputs(&[("hy_oas_pct", 6.0)]));
    assert_eq!(r.regime, Regime::Neutral);
}

#[test]
fn regime_evidence_summary_names_transition_and_trigger() {
    let summary = regime_evidence_summary(
        Some(Regime::Neutral),
        Regime::RiskOff,
        false,
        true,
        "hy_oas_pct",
        8.25,
    );

    assert_eq!(
        summary,
        "Market regime changed neutral -> risk_off after hy_oas_pct=8.25",
    );
}

#[test]
fn regime_evidence_summary_marks_capitulation_flip() {
    let summary = regime_evidence_summary(
        Some(Regime::RiskOff),
        Regime::RiskOff,
        false,
        true,
        "vix",
        31.0,
    );

    assert_eq!(
        summary,
        "Market regime risk_off capitulation changed false -> true after vix=31.00",
    );
}

#[test]
fn regime_evidence_strength_and_polarity_are_directional() {
    assert_eq!(regime_evidence_strength(true), 0.9);
    assert_eq!(regime_evidence_strength(false), 0.7);
    assert_eq!(regime_evidence_polarity(Regime::RiskOn), 0.5);
    assert_eq!(regime_evidence_polarity(Regime::Neutral), 0.0);
    assert_eq!(regime_evidence_polarity(Regime::RiskOff), -0.5);
}

#[test]
fn eval_ops_table() {
    let cases = [
        (5.0, ">3", true),
        (5.0, ">=5", true),
        (5.0, "<10", true),
        (5.0, "<=5", true),
        (5.0, "==5", true),
        (5.0, "!=5", false),
        (5.0, ">5", false),
    ];
    for (v, e, want) in cases {
        assert_eq!(eval(v, e).unwrap(), want, "eval({v}, {e:?})");
    }
}

#[test]
fn eval_bad_op_errors() {
    assert!(eval(1.0, "~5").is_err());
}
