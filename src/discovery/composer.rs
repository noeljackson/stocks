//! Deterministic discovery interpretation (#107).
//!
//! Raw detectors answer "what fired?". The composer answers the operator-facing
//! question: "what does that combination likely mean?" Keep this pure and
//! conservative; LLM cognition happens later only after a symbol earns depth.

use serde::Serialize;

use super::SignalHit;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryInterpretationKind {
    EarlyAccumulation,
    BreakoutConfirmation,
    ExtendedMomentum,
    ConsensusArrival,
    PossibleExhaustion,
    ExistingThesisTrigger,
    LowQualityNoise,
}

impl DiscoveryInterpretationKind {
    #[must_use]
    pub fn signal_name(self) -> &'static str {
        match self {
            Self::EarlyAccumulation => "early_accumulation",
            Self::BreakoutConfirmation => "breakout_confirmation",
            Self::ExtendedMomentum => "extended_momentum",
            Self::ConsensusArrival => "consensus_arrival",
            Self::PossibleExhaustion => "possible_exhaustion",
            Self::ExistingThesisTrigger => "existing_thesis_trigger",
            Self::LowQualityNoise => "low_quality_noise",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct PriceExtension {
    pub pct_above_sma: f64,
    pub sma_days: usize,
    pub rsi14: f64,
    pub pct_from_high: f64,
    pub raw: f64,
}

impl PriceExtension {
    #[must_use]
    pub fn from_closes_desc(closes_desc: &[f64]) -> Option<Self> {
        if closes_desc.len() < 15 {
            return None;
        }
        let latest = closes_desc[0];
        if latest <= 0.0 {
            return None;
        }
        let sma_n = closes_desc.len().min(200);
        let sma = closes_desc.iter().take(sma_n).sum::<f64>() / sma_n as f64;
        let pct_above_sma = if sma > 0.0 {
            (latest - sma) / sma * 100.0
        } else {
            0.0
        };
        let rsi14 = compute_rsi14(closes_desc);
        let high = closes_desc
            .iter()
            .take(252)
            .cloned()
            .fold(f64::MIN, f64::max);
        let pct_from_high = if high > 0.0 {
            (latest - high) / high * 100.0
        } else {
            0.0
        };
        let raw = price_extension_raw(pct_above_sma, rsi14, pct_from_high);
        Some(Self {
            pct_above_sma,
            sma_days: sma_n,
            rsi14,
            pct_from_high,
            raw,
        })
    }

    #[must_use]
    pub fn is_high(self) -> bool {
        self.raw >= 75.0
            || (self.pct_from_high >= -2.0 && (self.rsi14 >= 70.0 || self.pct_above_sma >= 20.0))
    }

    #[must_use]
    pub fn is_low_or_mid(self) -> bool {
        self.raw < 65.0
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ComposedSignal {
    pub symbol: String,
    pub kind: DiscoveryInterpretationKind,
    pub signal_name: String,
    pub value: f64,
    pub reasoning: String,
    pub raw_signals: Vec<String>,
    pub price_extension: Option<PriceExtension>,
    pub thesis_id: Option<uuid::Uuid>,
}

#[derive(Debug, Clone, Default)]
pub struct SignalContext {
    pub has_open_thesis: bool,
    pub has_actionable_thesis: bool,
    pub actionable_thesis_id: Option<uuid::Uuid>,
    pub is_active_ticker: bool,
    pub is_watchlisted: bool,
}

#[must_use]
pub fn compose(
    symbol: &str,
    raw_hits: &[SignalHit],
    extension: Option<PriceExtension>,
    ctx: &SignalContext,
) -> Option<ComposedSignal> {
    if raw_hits.is_empty() {
        return None;
    }
    if ctx.is_active_ticker && !ctx.has_actionable_thesis {
        return None;
    }
    let has = |name: &str| raw_hits.iter().any(|h| h.signal_name == name);
    let volume = raw_hits
        .iter()
        .find(|h| h.signal_name == "volume_anomaly")
        .map(|h| h.value);
    let estimate = raw_hits
        .iter()
        .find(|h| h.signal_name == "estimate_revision_velocity")
        .map(|h| h.value);
    let sentiment = raw_hits
        .iter()
        .find(|h| h.signal_name == "news_sentiment_shift")
        .map(|h| h.value);
    let raw_names = raw_hits
        .iter()
        .map(|h| h.signal_name.clone())
        .collect::<Vec<_>>();

    let negative_evidence = estimate.is_some_and(|v| v < 0.0) || sentiment.is_some_and(|v| v < 0.0);

    let kind = if ctx.has_actionable_thesis {
        DiscoveryInterpretationKind::ExistingThesisTrigger
    } else if negative_evidence && extension.is_some_and(|e| e.raw >= 60.0) {
        DiscoveryInterpretationKind::PossibleExhaustion
    } else if has("base_breakout") && extension.is_none_or(PriceExtension::is_low_or_mid) {
        DiscoveryInterpretationKind::BreakoutConfirmation
    } else if volume.is_some() && extension.is_some_and(PriceExtension::is_high) {
        if estimate.is_some_and(|v| v > 0.0)
            || sentiment.is_some_and(|v| v > 0.0)
            || ctx.has_open_thesis
        {
            DiscoveryInterpretationKind::ConsensusArrival
        } else {
            DiscoveryInterpretationKind::ExtendedMomentum
        }
    } else if volume.is_some()
        && extension.is_some_and(PriceExtension::is_low_or_mid)
        && (has("base_breakout")
            || estimate.is_some_and(|v| v > 0.0)
            || sentiment.is_some_and(|v| v > 0.0))
    {
        DiscoveryInterpretationKind::EarlyAccumulation
    } else if raw_hits.len() == 1 && volume.is_some() {
        DiscoveryInterpretationKind::LowQualityNoise
    } else {
        DiscoveryInterpretationKind::EarlyAccumulation
    };

    if kind == DiscoveryInterpretationKind::LowQualityNoise {
        return None;
    }

    let mut parts = Vec::new();
    if let Some(v) = volume {
        parts.push(format!("volume {v:.1}x 20-day avg"));
    }
    if has("base_breakout") {
        parts.push("base breakout".to_string());
    }
    if let Some(v) = estimate {
        parts.push(format!("{:+} net estimate revisions", v as i32));
    }
    if let Some(v) = sentiment {
        parts.push(format!("news sentiment shift {v:+.2}"));
    }
    if let Some(e) = extension {
        parts.push(format!(
            "extension {:.0}/100, RSI {:.0}, {:+.1}% vs available-window high, {:+.1}% vs {}-day SMA",
            e.raw, e.rsi14, e.pct_from_high, e.pct_above_sma, e.sma_days
        ));
    }
    if ctx.has_actionable_thesis {
        parts.push("existing actionable thesis".to_string());
    } else if ctx.has_open_thesis {
        parts.push("existing open thesis".to_string());
    } else if ctx.is_watchlisted {
        parts.push("already watchlisted".to_string());
    }

    let label = match kind {
        DiscoveryInterpretationKind::EarlyAccumulation => "possible early accumulation",
        DiscoveryInterpretationKind::BreakoutConfirmation => "breakout confirmation",
        DiscoveryInterpretationKind::ExtendedMomentum => "extended momentum, not early discovery",
        DiscoveryInterpretationKind::ConsensusArrival => "consensus arrival / crowd chase",
        DiscoveryInterpretationKind::PossibleExhaustion => "possible exhaustion / risk event",
        DiscoveryInterpretationKind::ExistingThesisTrigger => "existing thesis trigger",
        DiscoveryInterpretationKind::LowQualityNoise => unreachable!(),
    };
    Some(ComposedSignal {
        symbol: symbol.to_string(),
        kind,
        signal_name: kind.signal_name().to_string(),
        value: extension.map_or_else(
            || raw_hits.iter().map(|h| h.value.abs()).fold(0.0, f64::max),
            |e| e.raw,
        ),
        reasoning: format!("{symbol}: {label} — {}", parts.join("; ")),
        raw_signals: raw_names,
        price_extension: extension,
        thesis_id: ctx.actionable_thesis_id,
    })
}

fn price_extension_raw(pct_above_sma: f64, rsi14: f64, pct_from_high: f64) -> f64 {
    let a = (pct_above_sma / 30.0).clamp(0.0, 1.0) * 100.0;
    let b = ((rsi14 - 50.0) / 40.0).clamp(0.0, 1.0) * 100.0;
    let dist = pct_from_high.clamp(-30.0, 0.0);
    let c = ((dist + 30.0) / 30.0) * 100.0;
    (a + b + c) / 3.0
}

fn compute_rsi14(closes_desc: &[f64]) -> f64 {
    if closes_desc.len() < 15 {
        return 50.0;
    }
    let mut gains = 0.0;
    let mut losses = 0.0;
    for win in closes_desc[..15].windows(2) {
        let newer = win[0];
        let older = win[1];
        let diff = newer - older;
        if diff >= 0.0 {
            gains += diff;
        } else {
            losses += -diff;
        }
    }
    if losses == 0.0 {
        return 100.0;
    }
    let rs = gains / losses;
    100.0 - (100.0 / (1.0 + rs))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(name: &'static str, value: f64) -> SignalHit {
        SignalHit {
            symbol: "TST".to_string(),
            signal_name: name.to_string(),
            value,
            reasoning: name.to_string(),
        }
    }

    #[test]
    fn volume_spike_at_high_extension_is_extended_momentum() {
        let ext = PriceExtension {
            pct_above_sma: 28.0,
            sma_days: 200,
            rsi14: 77.0,
            pct_from_high: 0.0,
            raw: 90.0,
        };
        let got = compose(
            "TST",
            &[hit("volume_anomaly", 4.5)],
            Some(ext),
            &SignalContext::default(),
        )
        .unwrap();
        assert_eq!(got.kind, DiscoveryInterpretationKind::ExtendedMomentum);
        assert_eq!(got.signal_name, "extended_momentum");
        assert!(got.reasoning.contains("not early discovery"));
    }

    #[test]
    fn volume_plus_tight_breakout_is_breakout_confirmation() {
        let ext = PriceExtension {
            pct_above_sma: 6.0,
            sma_days: 200,
            rsi14: 58.0,
            pct_from_high: -8.0,
            raw: 45.0,
        };
        let got = compose(
            "TST",
            &[hit("volume_anomaly", 3.7), hit("base_breakout", 4.2)],
            Some(ext),
            &SignalContext::default(),
        )
        .unwrap();
        assert_eq!(got.kind, DiscoveryInterpretationKind::BreakoutConfirmation);
        assert_eq!(got.raw_signals, vec!["volume_anomaly", "base_breakout"]);
    }

    #[test]
    fn high_extension_plus_positive_news_is_consensus_arrival() {
        let ext = PriceExtension {
            pct_above_sma: 31.0,
            sma_days: 200,
            rsi14: 81.0,
            pct_from_high: -0.5,
            raw: 95.0,
        };
        let got = compose(
            "TST",
            &[hit("volume_anomaly", 5.0), hit("news_sentiment_shift", 0.4)],
            Some(ext),
            &SignalContext::default(),
        )
        .unwrap();
        assert_eq!(got.kind, DiscoveryInterpretationKind::ConsensusArrival);
        assert!(got.reasoning.contains("crowd chase"));
    }

    #[test]
    fn negative_news_at_high_extension_is_possible_exhaustion() {
        let ext = PriceExtension {
            pct_above_sma: 40.0,
            sma_days: 200,
            rsi14: 72.0,
            pct_from_high: -3.0,
            raw: 78.0,
        };
        let got = compose(
            "TST",
            &[hit("news_sentiment_shift", -0.4)],
            Some(ext),
            &SignalContext::default(),
        )
        .unwrap();
        assert_eq!(got.kind, DiscoveryInterpretationKind::PossibleExhaustion);
        assert!(got.reasoning.contains("risk event"));
    }

    #[test]
    fn actionable_thesis_routes_to_existing_thesis_trigger() {
        let ext = PriceExtension {
            pct_above_sma: 10.0,
            sma_days: 200,
            rsi14: 62.0,
            pct_from_high: -5.0,
            raw: 55.0,
        };
        let got = compose(
            "TST",
            &[hit("volume_anomaly", 3.2)],
            Some(ext),
            &SignalContext {
                has_actionable_thesis: true,
                ..SignalContext::default()
            },
        )
        .unwrap();
        assert_eq!(got.kind, DiscoveryInterpretationKind::ExistingThesisTrigger);
    }

    #[test]
    fn active_ticker_without_actionable_thesis_does_not_create_candidate_review() {
        let ext = PriceExtension {
            pct_above_sma: 12.0,
            sma_days: 200,
            rsi14: 64.0,
            pct_from_high: -8.0,
            raw: 58.0,
        };
        let got = compose(
            "TST",
            &[hit("volume_anomaly", 3.8), hit("news_sentiment_shift", 0.5)],
            Some(ext),
            &SignalContext {
                is_active_ticker: true,
                ..SignalContext::default()
            },
        );
        assert!(got.is_none());
    }

    #[test]
    fn weak_volume_only_signal_is_suppressed() {
        let ext = PriceExtension {
            pct_above_sma: 4.0,
            sma_days: 200,
            rsi14: 54.0,
            pct_from_high: -20.0,
            raw: 22.0,
        };
        let got = compose(
            "TST",
            &[hit("volume_anomaly", 3.1)],
            Some(ext),
            &SignalContext::default(),
        );
        assert!(got.is_none());
    }
}
