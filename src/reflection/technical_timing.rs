//! Forward-only validation helpers for derived technical timing states.

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::platform::technical::TechnicalBar;

pub const TRACKED_STATES: &[&str] = &[
    "pullback_reversal",
    "pullback_watch",
    "constructive_trend",
    "extended_chase",
    "avoid_breakdown",
];

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct TimingOutcome {
    pub forward_return_pct: f64,
    pub max_drawdown_pct: f64,
    pub benchmark_return_pct: Option<f64>,
    pub benchmark_max_drawdown_pct: Option<f64>,
    pub excess_return_pct: Option<f64>,
}

#[must_use]
pub fn tracks_state(state: &str, setup_kind: &str) -> bool {
    TRACKED_STATES
        .iter()
        .any(|tracked| state == *tracked || setup_kind == *tracked)
}

#[must_use]
pub fn evaluate_outcome(
    entry_close: f64,
    future_bars: &[TechnicalBar],
    horizon_bars: usize,
    benchmark_entry_close: Option<f64>,
    benchmark_future_bars: &[TechnicalBar],
) -> Option<TimingOutcome> {
    if entry_close <= 0.0 || horizon_bars == 0 || future_bars.len() < horizon_bars {
        return None;
    }
    let horizon = &future_bars[..horizon_bars];
    let forward_return_pct = pct_return(entry_close, horizon.last()?.close)?;
    let max_drawdown_pct = max_drawdown(entry_close, horizon);
    let (benchmark_return_pct, benchmark_max_drawdown_pct) =
        if let Some(benchmark_entry) = benchmark_entry_close.filter(|v| *v > 0.0) {
            if benchmark_future_bars.len() >= horizon_bars {
                let benchmark_horizon = &benchmark_future_bars[..horizon_bars];
                (
                    pct_return(benchmark_entry, benchmark_horizon.last()?.close),
                    Some(max_drawdown(benchmark_entry, benchmark_horizon)),
                )
            } else {
                (None, None)
            }
        } else {
            (None, None)
        };
    Some(TimingOutcome {
        forward_return_pct,
        max_drawdown_pct,
        benchmark_return_pct,
        benchmark_max_drawdown_pct,
        excess_return_pct: benchmark_return_pct.map(|benchmark| forward_return_pct - benchmark),
    })
}

fn pct_return(entry: f64, exit: f64) -> Option<f64> {
    (entry > 0.0).then(|| (exit - entry) / entry * 100.0)
}

fn max_drawdown(entry: f64, future_bars: &[TechnicalBar]) -> f64 {
    future_bars
        .iter()
        .map(|bar| (bar.low - entry) / entry * 100.0)
        .fold(0.0, f64::min)
}

#[must_use]
pub fn due_at(observed_at: DateTime<Utc>, horizon_bars: i64) -> DateTime<Utc> {
    observed_at + chrono::Duration::days(horizon_bars.max(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn bar(day: u32, close: f64, low: f64) -> TechnicalBar {
        TechnicalBar {
            ts: Utc.with_ymd_and_hms(2026, 1, day, 0, 0, 0).unwrap(),
            close,
            high: close.max(low),
            low,
            volume: 1_000_000.0,
        }
    }

    #[test]
    fn tracked_states_match_state_or_setup_kind() {
        assert!(tracks_state("pullback_reversal", "pullback_reversal"));
        assert!(tracks_state("constructive", "constructive_trend"));
        assert!(tracks_state("deteriorating", "avoid_breakdown"));
        assert!(!tracks_state("base_building", "wait_breakout"));
    }

    #[test]
    fn outcome_scores_forward_return_drawdown_and_excess() {
        let outcome = evaluate_outcome(
            100.0,
            &[bar(2, 103.0, 98.0), bar(3, 110.0, 101.0)],
            2,
            Some(200.0),
            &[bar(2, 202.0, 199.0), bar(3, 204.0, 201.0)],
        )
        .unwrap();

        assert_eq!(outcome.forward_return_pct, 10.0);
        assert_eq!(outcome.max_drawdown_pct, -2.0);
        assert_eq!(outcome.benchmark_return_pct, Some(2.0));
        assert_eq!(outcome.benchmark_max_drawdown_pct, Some(-0.5));
        assert_eq!(outcome.excess_return_pct, Some(8.0));
    }

    #[test]
    fn missing_future_bars_defers_scoring() {
        assert_eq!(
            evaluate_outcome(100.0, &[bar(2, 103.0, 98.0)], 2, None, &[]),
            None
        );
    }
}
