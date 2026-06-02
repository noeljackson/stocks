//! Pure scoring functions for forecast calibration (Brier) and
//! lead-time-to-consensus. No I/O; consumed by `service.rs`.

use chrono::{DateTime, Utc};
use serde::Serialize;

/// One Brier-score contribution for a binary forecast.
/// Brier = (predicted_prob - realised_outcome)² where outcome ∈ {0, 1}.
/// 0 is perfect; 1 is maximally wrong. Used per-thesis; aggregated over time.
#[must_use]
pub fn brier(predicted_prob: f64, realised: bool) -> f64 {
    let outcome = if realised { 1.0 } else { 0.0 };
    (predicted_prob - outcome).powi(2)
}

/// Mean Brier across N predictions. Returns None for empty input.
#[must_use]
pub fn mean_brier(scores: &[f64]) -> Option<f64> {
    if scores.is_empty() {
        return None;
    }
    Some(scores.iter().sum::<f64>() / scores.len() as f64)
}

/// Lead-time in (positive) days between an alert and the consensus crossing.
/// Negative means consensus *preceded* the alert — system was late, not early.
#[must_use]
pub fn lead_time_days(alert_at: DateTime<Utc>, consensus_at: DateTime<Utc>) -> f64 {
    let delta = consensus_at.signed_duration_since(alert_at);
    delta.num_seconds() as f64 / 86_400.0
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct CalibrationSummary {
    pub predictions_total: i64,
    pub outcomes_scored: i64,
    pub mean_brier: Option<f64>,
    pub mean_lead_time_days: Option<f64>,
    pub median_lead_time_days: Option<f64>,
    pub parent_themes: Vec<ParentThemeCalibration>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ParentThemeCalibration {
    pub key: String,
    pub name: String,
    pub scope: String,
    pub role: String,
    pub predictions_total: i64,
    pub outcomes_scored: i64,
    pub mean_brier: Option<f64>,
    pub mean_lead_time_days: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn brier_perfect_zero() {
        assert_eq!(brier(1.0, true), 0.0);
        assert_eq!(brier(0.0, false), 0.0);
    }

    #[test]
    fn brier_max_one() {
        assert_eq!(brier(0.0, true), 1.0);
        assert_eq!(brier(1.0, false), 1.0);
    }

    #[test]
    fn brier_50_50_is_quarter() {
        assert_eq!(brier(0.5, true), 0.25);
        assert_eq!(brier(0.5, false), 0.25);
    }

    #[test]
    fn mean_brier_handles_empty() {
        assert_eq!(mean_brier(&[]), None);
    }

    #[test]
    fn mean_brier_avg() {
        assert_eq!(mean_brier(&[0.0, 0.5, 0.25]), Some(0.25));
    }

    #[test]
    fn lead_time_positive_when_alert_before_consensus() {
        let alert = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let consensus = Utc.with_ymd_and_hms(2026, 1, 31, 0, 0, 0).unwrap();
        assert_eq!(lead_time_days(alert, consensus), 30.0);
    }

    #[test]
    fn lead_time_negative_when_alert_late() {
        let consensus = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let alert = Utc.with_ymd_and_hms(2026, 1, 11, 0, 0, 0).unwrap();
        assert_eq!(lead_time_days(alert, consensus), -10.0);
    }
}
