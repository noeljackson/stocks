//! Macro regime classifier (SPEC §4).
//!
//! [`classify`] is a pure function: given (inputs, config) → (Regime,
//! capitulation, matched-per-state). [`run`] wires it to NATS+DB.

mod expr;
mod service;
#[cfg(test)]
mod tests;

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

pub use service::run;

use crate::platform::domain::Regime;

/// Parsed body of `config(name='regime', active=true)`. Shape mirrors
/// `db/migrations/0002_seed.sql`.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub states: Vec<String>,
    /// regime → indicator → "<op><num>"
    #[serde(default)]
    pub rules: HashMap<String, HashMap<String, String>>,
    #[serde(default)]
    pub capitulation: Capitulation,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Capitulation {
    #[serde(default)]
    pub any_of: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Outcome {
    pub regime: Regime,
    pub capitulation: bool,
    pub indicators: HashMap<String, f64>,
    pub matched: HashMap<String, f64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub reasons: Vec<String>,
}

/// FRED series → indicator-name mapping. Unmapped series keep their raw ID
/// (the classifier just ignores them — useful for telemetry).
pub fn fred_series_to_indicator(series: &str) -> &str {
    match series {
        "VIXCLS" => "vix",
        "BAMLH0A0HYM2" => "hy_oas_pct",
        "DGS10" => "us10y",
        "DGS3MO" => "us3m",
        other => other,
    }
}

/// Pure classifier. For each state in `cfg.rules`, score = (rules satisfied
/// with known inputs) / (rules total). State wins iff score ≥ 0.5 AND > the
/// other state's score. Otherwise neutral — never fabricate conviction.
#[must_use]
pub fn classify(cfg: &Config, inputs: &HashMap<String, f64>) -> Outcome {
    let mut matched = HashMap::new();
    let mut reasons = Vec::new();

    for (state, rules) in &cfg.rules {
        let mut sat = 0u32;
        let mut total = 0u32;
        for (name, e) in rules {
            total += 1;
            let Some(&v) = inputs.get(name) else {
                continue;
            };
            match expr::eval(v, e) {
                Ok(true) => sat += 1,
                Ok(false) => {}
                Err(err) => reasons.push(format!("rule {state}.{name}: {err}")),
            }
        }
        let score = if total > 0 {
            f64::from(sat) / f64::from(total)
        } else {
            0.0
        };
        matched.insert(state.clone(), score);
    }

    let on = matched.get("risk_on").copied().unwrap_or(0.0);
    let off = matched.get("risk_off").copied().unwrap_or(0.0);
    let regime = if on >= 0.5 && on > off {
        Regime::RiskOn
    } else if off >= 0.5 && off > on {
        Regime::RiskOff
    } else {
        Regime::Neutral
    };

    // Capitulation: any "name<op><num>" we can evaluate that fires.
    let mut capitulation = false;
    for e in &cfg.capitulation.any_of {
        let Some((name, expr_part)) = split_name_expr(e) else {
            continue;
        };
        let Some(&v) = inputs.get(name) else {
            continue;
        };
        if let Ok(true) = expr::eval(v, expr_part) {
            capitulation = true;
            reasons.push(format!("capitulation: {e}"));
            break;
        }
    }

    Outcome {
        regime,
        capitulation,
        indicators: inputs.clone(),
        matched,
        reasons,
    }
}

fn split_name_expr(s: &str) -> Option<(&str, &str)> {
    for (i, c) in s.char_indices() {
        if c == '>' || c == '<' || c == '=' {
            if i == 0 {
                return None;
            }
            return Some((s[..i].trim(), s[i..].trim()));
        }
    }
    None
}
