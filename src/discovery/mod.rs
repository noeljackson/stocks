//! Discovery scanner (SPEC §6.1, #22). Cheap-wide signal detectors over
//! the haystack; proposes promotion candidates; user confirms/rejects
//! (feedback loop on domain-fit's circle-of-competence weight).
//!
//! Today's signals (data-gated):
//!   - volume_anomaly       LIVE (price_bar)
//!   - base_breakout        LIVE (price_bar)
//!   - estimate_revision_inflection  STUB (needs #18)
//!   - filing_news_cluster  STUB (needs #19)

pub mod composer;
pub mod ranking;
pub mod service;
pub mod signals;

use serde::{Deserialize, Serialize};

/// One observation from a signal detector. Pure data.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SignalHit {
    pub symbol: String,
    pub signal_name: String,
    pub value: f64,
    pub reasoning: String,
}

/// Discovery config from the `discovery_signals` config row.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub signals: Vec<SignalCfg>,
    pub promote_to_tier2_threshold: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SignalCfg {
    pub name: String,
    pub weight: f64,
    pub enabled: bool,
}

impl Config {
    pub fn enabled(&self, name: &str) -> bool {
        self.signals.iter().any(|s| s.name == name && s.enabled)
    }
}
