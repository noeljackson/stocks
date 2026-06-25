//! Consensus computation (SPEC §6.2, issue #21).
//!
//! Per the SPEC this is the most-reused event in the system: it fires the
//! fulfillment/exit transition for discovery theses and is the validation
//! anchor for lead-time-to-consensus (consensus_at − alert_at).
//!
//! Design: 5 components, each scored 0..100, weighted, summed → final 0..100.
//! Two thresholds: measurement (default 60 — timestamps "consensus formed"
//! for lead-time accounting) and exit (default 70 — fires fulfillment).
//!
//! Components (per SPEC):
//! 1. coverage_expansion          — analyst initiations/upgrades surge
//! 2. estimate_revision_saturation — early inflection now baseline
//! 3. mainstream_coverage         — specialist→generalist outlet shift
//! 4. retail_attention            — social volume / put-call / call-skew
//! 5. price_extension             — distance above SMAs / RSI / parabolic
//!
//! Per the SPEC's "graceful degradation" rule, components requiring data we
//! don't have yet contribute 0 (with attribution). As #18/#19/#20 land they
//! light up.

pub mod components;
pub mod service;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub weights: Weights,
    pub measurement_threshold: f64,
    pub exit_threshold: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Weights {
    pub coverage_expansion: f64,
    pub estimate_revision_saturation: f64,
    pub mainstream_coverage: f64,
    pub retail_attention: f64,
    pub price_extension: f64,
}

impl Weights {
    /// Sum of all weights — used to normalize so a score always lands in 0..100.
    #[must_use]
    pub fn total(&self) -> f64 {
        self.coverage_expansion
            + self.estimate_revision_saturation
            + self.mainstream_coverage
            + self.retail_attention
            + self.price_extension
    }
}

/// One component's contribution.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ComponentScore {
    pub name: &'static str,
    /// 0..100 (the component's own scale, before weighting).
    pub raw: f64,
    /// 0..weight (the component's contribution after weighting).
    pub weighted: f64,
    /// "ok" if computed from real data, "no_data" if degraded.
    pub status: &'static str,
}

/// Final per-symbol score.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Score {
    pub symbol: String,
    /// Sum of all component.weighted contributions, scaled so a fully-covered
    /// max score is 100. (The weights themselves don't have to sum to 100;
    /// we normalize by the weight total.)
    pub total: f64,
    pub components: Vec<ComponentScore>,
    pub measurement_crossed: bool,
    pub exit_crossed: bool,
}

/// Compose a final score from already-computed components + config.
/// Pure function; called by the service after async component computation.
#[must_use]
pub fn compose(symbol: &str, components: Vec<ComponentScore>, cfg: &Config) -> Score {
    let weight_total = cfg.weights.total();
    let raw_total: f64 = components.iter().map(|c| c.weighted).sum();
    // Normalize so total is always 0..100 regardless of weight choices.
    let total = if weight_total > 0.0 {
        (raw_total / weight_total) * 100.0
    } else {
        0.0
    };
    Score {
        symbol: symbol.to_string(),
        total,
        components,
        measurement_crossed: total >= cfg.measurement_threshold,
        exit_crossed: total >= cfg.exit_threshold,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn cfg() -> Config {
        Config {
            weights: Weights {
                coverage_expansion: 25.0,
                estimate_revision_saturation: 20.0,
                mainstream_coverage: 20.0,
                retail_attention: 15.0,
                price_extension: 20.0,
            },
            measurement_threshold: 60.0,
            exit_threshold: 70.0,
        }
    }

    fn comp(name: &'static str, raw: f64, weight: f64, status: &'static str) -> ComponentScore {
        ComponentScore {
            name,
            raw,
            weighted: raw * weight / 100.0,
            status,
        }
    }

    #[test]
    fn weights_total_sums_correctly() {
        assert_eq!(cfg().weights.total(), 100.0);
    }

    #[test]
    fn all_components_max_yields_100() {
        // Every component at raw=100, weighted = its full weight, total=100.
        let c = cfg();
        let comps = vec![
            comp(
                "coverage_expansion",
                100.0,
                c.weights.coverage_expansion,
                "ok",
            ),
            comp(
                "estimate_revision_saturation",
                100.0,
                c.weights.estimate_revision_saturation,
                "ok",
            ),
            comp(
                "mainstream_coverage",
                100.0,
                c.weights.mainstream_coverage,
                "ok",
            ),
            comp("retail_attention", 100.0, c.weights.retail_attention, "ok"),
            comp("price_extension", 100.0, c.weights.price_extension, "ok"),
        ];
        let s = compose("NVDA", comps, &c);
        assert_eq!(s.total, 100.0);
        assert!(s.measurement_crossed);
        assert!(s.exit_crossed);
    }

    #[test]
    fn all_components_zero_yields_zero() {
        let c = cfg();
        let comps = vec![
            comp(
                "coverage_expansion",
                0.0,
                c.weights.coverage_expansion,
                "no_data",
            ),
            comp(
                "estimate_revision_saturation",
                0.0,
                c.weights.estimate_revision_saturation,
                "no_data",
            ),
            comp(
                "mainstream_coverage",
                0.0,
                c.weights.mainstream_coverage,
                "no_data",
            ),
            comp(
                "retail_attention",
                0.0,
                c.weights.retail_attention,
                "no_data",
            ),
            comp("price_extension", 0.0, c.weights.price_extension, "no_data"),
        ];
        let s = compose("NVDA", comps, &c);
        assert_eq!(s.total, 0.0);
        assert!(!s.measurement_crossed);
        assert!(!s.exit_crossed);
    }

    #[test]
    fn only_price_extension_available_scales_correctly() {
        // SPEC graceful-degradation case: only price_extension lit up.
        // Its weight is 20; if it scores 100, that's 20 weighted out of 100
        // total weight → final 20.
        let c = cfg();
        let comps = vec![
            comp(
                "coverage_expansion",
                0.0,
                c.weights.coverage_expansion,
                "no_data",
            ),
            comp(
                "estimate_revision_saturation",
                0.0,
                c.weights.estimate_revision_saturation,
                "no_data",
            ),
            comp(
                "mainstream_coverage",
                0.0,
                c.weights.mainstream_coverage,
                "no_data",
            ),
            comp(
                "retail_attention",
                0.0,
                c.weights.retail_attention,
                "no_data",
            ),
            comp("price_extension", 100.0, c.weights.price_extension, "ok"),
        ];
        let s = compose("NVDA", comps, &c);
        assert_eq!(s.total, 20.0);
        assert!(!s.measurement_crossed, "20 < 60 threshold");
    }

    #[test]
    fn measurement_crosses_at_60_exit_at_70() {
        let c = cfg();
        // Compose hand-crafted weighted values summing to 60.
        let comps = vec![ComponentScore {
            name: "x",
            raw: 60.0,
            weighted: 60.0,
            status: "ok",
        }];
        // weight_total here is from cfg = 100; raw_total 60 / 100 * 100 = 60.
        let s = compose("X", comps, &c);
        assert_eq!(s.total, 60.0);
        assert!(s.measurement_crossed);
        assert!(!s.exit_crossed);
    }
}
