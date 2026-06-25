//! Goalpost detector: integrity guard for thesis invalidation conditions
//! (SPEC §5.3). Pure function diff; service wires it to thesis.updated.

mod service;
#[cfg(test)]
mod tests;

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

pub use service::run;

/// Mirrors the JSON shape stored in `thesis.invalidation_conditions`.
/// Internally a sum type — `quantitative` carries `expr`, `narrative` carries
/// `assertion`. Either form requires a stable `name` for diffing.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Condition {
    Quantitative { name: String, expr: String },
    Narrative { name: String, assertion: String },
}

impl Condition {
    fn name(&self) -> &str {
        match self {
            Self::Quantitative { name, .. } | Self::Narrative { name, .. } => name,
        }
    }
}

/// Verdict on a thesis version transition.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Report {
    pub weakened: bool,
    pub needs_review: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub dropped: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub loosened: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub added: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub reasons: Vec<String>,
}

/// Compares two condition sets and produces an integrity verdict.
#[must_use]
pub fn analyze(original: &[Condition], updated: &[Condition]) -> Report {
    let mut r = Report::default();

    let by_name = |cs: &[Condition]| -> HashMap<String, Condition> {
        cs.iter()
            .filter(|c| !c.name().is_empty())
            .map(|c| (c.name().to_string(), c.clone()))
            .collect()
    };
    let orig = by_name(original);
    let upd = by_name(updated);

    for name in orig.keys() {
        if !upd.contains_key(name) {
            r.dropped.push(name.clone());
        }
    }
    for name in upd.keys() {
        if !orig.contains_key(name) {
            r.added.push(name.clone());
        }
    }
    for (name, oc) in &orig {
        let Some(uc) = upd.get(name) else { continue };
        match (oc, uc) {
            (
                Condition::Quantitative { expr: oe, .. },
                Condition::Quantitative { expr: ue, .. },
            ) => {
                if expr_is_looser_than(ue, oe) {
                    r.loosened.push(name.clone());
                    r.reasons.push(format!("loosened {name}: {oe} → {ue}"));
                }
            }
            (
                Condition::Narrative { assertion: oa, .. },
                Condition::Narrative { assertion: ua, .. },
            ) => {
                if oa != ua {
                    r.needs_review = true;
                    r.reasons
                        .push(format!("narrative changed for {name} — human review"));
                }
            }
            // Type swap (quantitative ⇄ narrative): conservatively needs review.
            _ => {
                r.needs_review = true;
                r.reasons
                    .push(format!("type changed for {name} — human review"));
            }
        }
    }

    if !r.dropped.is_empty() || !r.loosened.is_empty() {
        r.weakened = true;
    }
    for n in &r.dropped {
        r.reasons
            .push(format!("dropped invalidation condition: {n}"));
    }
    // Pure rewrite: all originals dropped, all updated added.
    if !original.is_empty() && r.dropped.len() == original.len() && !r.added.is_empty() {
        r.needs_review = true;
    }

    // Stable order for deterministic test assertions.
    r.dropped.sort();
    r.added.sort();
    r.loosened.sort();
    r
}

/// `field op num` parsed with whitespace tolerance.
///
/// Supported ops: `<`, `<=`, `>`, `>=`, `==`, `!=`. `==`/`!=` are never
/// considered looser/stricter (binary equality has no notion of "looser").
fn parse_simple_expr(s: &str) -> Option<(String, &'static str, f64)> {
    let s = s.trim();
    // Longest ops first.
    for cand in &["<=", ">=", "==", "!=", "<", ">"] {
        if let Some(idx) = s.find(cand) {
            if idx == 0 {
                continue;
            }
            let lhs = s[..idx].trim();
            let rhs = s[idx + cand.len()..].trim();
            if lhs.is_empty() || rhs.is_empty() || lhs.chars().any(char::is_whitespace) {
                continue;
            }
            let n: f64 = rhs.parse().ok()?;
            return Some((lhs.to_string(), cand, n));
        }
    }
    None
}

fn op_family(op: &str) -> &str {
    match op {
        "<" | "<=" => "<",
        ">" | ">=" => ">",
        _ => op,
    }
}

fn expr_is_looser_than(new_expr: &str, old_expr: &str) -> bool {
    let (nf, nop, nn) = match parse_simple_expr(new_expr) {
        Some(t) => t,
        None => return false,
    };
    let (of, oop, on) = match parse_simple_expr(old_expr) {
        Some(t) => t,
        None => return false,
    };
    if nf != of {
        return false;
    }
    if op_family(nop) != op_family(oop) {
        return false;
    }
    match op_family(nop) {
        // "field < N" invalidates if field drops below N. Lower N → harder to trip → LOOSER.
        "<" => nn < on,
        // "field > N" invalidates if field exceeds N. Higher N → harder to trip → LOOSER.
        ">" => nn > on,
        _ => false,
    }
}
