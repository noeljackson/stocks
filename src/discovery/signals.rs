//! Pure signal-detector functions.
//!
//! Price-based signals take vectors of recent closes/volumes and return
//! `Option<f64>` — Some(strength) if the signal fired, None if not.
//!
//! Data-driven signals (#18, #19) take counts/scores already aggregated by
//! the service layer and return the same Option<f64> shape so the persist
//! pipeline doesn't care where the signal came from.

/// Estimate revision velocity: net up-vs-down analyst revisions in a window.
/// Fires when |net| ≥ `min_net` AND there's clear directional dominance
/// (one side > 2× the other). Returns Some(net) — positive for up-velocity,
/// negative for down-velocity.
#[must_use]
pub fn estimate_revision_velocity(up: u32, down: u32, min_net: i32) -> Option<f64> {
    let net = (up as i32) - (down as i32);
    if net.abs() < min_net {
        return None;
    }
    let bigger = up.max(down) as f64;
    let smaller = up.min(down) as f64;
    if bigger > 2.0 * smaller.max(1.0) - 1.0 {
        Some(net as f64)
    } else {
        None
    }
}

/// News sentiment shift: average polarity in the recent window minus the
/// average in the prior window. Fires when the absolute shift exceeds
/// `min_shift` AND we have at least `min_articles` in BOTH windows (we
/// can't trust a single-article "shift").
#[must_use]
pub fn news_sentiment_shift(
    recent_avg: f64,
    recent_n: u32,
    prior_avg: f64,
    prior_n: u32,
    min_shift: f64,
    min_articles: u32,
) -> Option<f64> {
    if recent_n < min_articles || prior_n < min_articles {
        return None;
    }
    let shift = recent_avg - prior_avg;
    if shift.abs() >= min_shift { Some(shift) } else { None }
}

/// Volume anomaly: today's volume relative to the 20-day average.
///   3x avg → returns Some(3.0)
///   5x avg → Some(5.0)
/// Fires when latest > threshold × avg(prior 20).
#[must_use]
pub fn volume_anomaly(volumes_desc: &[f64], threshold_x: f64) -> Option<f64> {
    if volumes_desc.len() < 21 || threshold_x <= 0.0 {
        return None;
    }
    let latest = volumes_desc[0];
    if latest <= 0.0 {
        return None;
    }
    let prior20: f64 = volumes_desc[1..21].iter().sum::<f64>() / 20.0;
    if prior20 <= 0.0 {
        return None;
    }
    let multiple = latest / prior20;
    if multiple > threshold_x {
        Some(multiple)
    } else {
        None
    }
}

/// Base breakout: latest close above the highest-high of the prior N days
/// (default 55, "Donchian-channel-style"), with consolidation tightness
/// (range of last 20 < 8% of midpoint). Returns Some(breakout_pct) if
/// fired — the % above the breakout level.
#[must_use]
pub fn base_breakout(closes_desc: &[f64], breakout_window: usize, tightness_pct: f64) -> Option<f64> {
    if closes_desc.len() < breakout_window + 1 {
        return None;
    }
    let latest = closes_desc[0];
    // Range over the last 20 bars must be tight.
    let last20 = &closes_desc[..closes_desc.len().min(20)];
    let high = last20.iter().cloned().fold(f64::MIN, f64::max);
    let low = last20.iter().cloned().fold(f64::MAX, f64::min);
    if high <= 0.0 || low <= 0.0 {
        return None;
    }
    let mid = (high + low) / 2.0;
    let range_pct = if mid > 0.0 { (high - low) / mid * 100.0 } else { 100.0 };
    if range_pct > tightness_pct {
        return None;
    }
    // Breakout: latest > max of prior N (excluding today).
    let prior = &closes_desc[1..=breakout_window];
    let prior_high = prior.iter().cloned().fold(f64::MIN, f64::max);
    if prior_high <= 0.0 || latest <= prior_high {
        return None;
    }
    Some((latest - prior_high) / prior_high * 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_anomaly_fires_on_3x_spike() {
        let mut v = vec![1.0e6; 21];
        v[0] = 5.0e6; // latest 5x average
        assert_eq!(volume_anomaly(&v, 3.0), Some(5.0));
    }

    #[test]
    fn volume_anomaly_silent_below_threshold() {
        let mut v = vec![1.0e6; 21];
        v[0] = 2.0e6;
        assert_eq!(volume_anomaly(&v, 3.0), None, "2x < 3x threshold");
    }

    #[test]
    fn volume_anomaly_handles_short_history() {
        let v = vec![1e6; 10]; // < 21 bars
        assert_eq!(volume_anomaly(&v, 3.0), None);
    }

    #[test]
    fn volume_anomaly_handles_zero_prior() {
        let mut v = vec![0.0; 21];
        v[0] = 1e6;
        assert_eq!(volume_anomaly(&v, 3.0), None);
    }

    #[test]
    fn base_breakout_fires_after_tight_base() {
        let mut c = vec![100.0; 60];
        c[0] = 105.0; // latest 5% above prior
        // Force last20 to be tight around 100 (range 100-100 = 0% < 8%).
        for i in 1..20 {
            c[i] = 100.0;
        }
        let hit = base_breakout(&c, 55, 8.0);
        assert!(hit.is_some());
        assert!((hit.unwrap() - 5.0).abs() < 0.01);
    }

    #[test]
    fn base_breakout_silent_in_wide_range() {
        let mut c = vec![100.0; 60];
        c[0] = 105.0;
        // Wide range: last20 spans 80..120 → 40% range > 8% tightness.
        for i in 1..20 {
            c[i] = if i % 2 == 0 { 120.0 } else { 80.0 };
        }
        assert_eq!(base_breakout(&c, 55, 8.0), None);
    }

    #[test]
    fn base_breakout_silent_when_not_above_prior_high() {
        let mut c = vec![100.0; 60];
        c[0] = 99.0; // below prior
        for i in 1..20 {
            c[i] = 100.0;
        }
        assert_eq!(base_breakout(&c, 55, 8.0), None);
    }

    #[test]
    fn base_breakout_short_history_returns_none() {
        let c = vec![100.0; 10];
        assert_eq!(base_breakout(&c, 55, 8.0), None);
    }

    // ---- estimate_revision_velocity ----

    #[test]
    fn revision_velocity_fires_when_net_dominates() {
        assert_eq!(estimate_revision_velocity(5, 1, 3), Some(4.0));
        assert_eq!(estimate_revision_velocity(0, 4, 3), Some(-4.0));
    }

    #[test]
    fn revision_velocity_silent_below_min_net() {
        assert_eq!(estimate_revision_velocity(2, 0, 3), None);
    }

    #[test]
    fn revision_velocity_silent_when_mixed() {
        // 3 up, 2 down: net=1, way below min, OR even if min=1:
        assert_eq!(estimate_revision_velocity(3, 2, 1), None,
            "3 vs 2 is too close; bigger must be >2x smaller");
        // 4 up, 1 down: net=3, bigger(4) > 2*1-1=1, fires
        assert_eq!(estimate_revision_velocity(4, 1, 3), Some(3.0));
    }

    // ---- news_sentiment_shift ----

    #[test]
    fn sentiment_shift_fires_on_clear_drift_with_enough_articles() {
        assert_eq!(
            news_sentiment_shift(0.6, 5, 0.0, 5, 0.3, 3),
            Some(0.6),
            "+0.6 shift, 5+5 articles, threshold 0.3 → fires"
        );
    }

    #[test]
    fn sentiment_shift_negative_drift_fires() {
        let r = news_sentiment_shift(-0.4, 4, 0.3, 4, 0.3, 3).unwrap();
        assert!((r - -0.7).abs() < 1e-9);
    }

    #[test]
    fn sentiment_shift_silent_below_threshold() {
        assert_eq!(news_sentiment_shift(0.2, 5, 0.0, 5, 0.3, 3), None);
    }

    #[test]
    fn sentiment_shift_silent_with_too_few_articles() {
        assert_eq!(
            news_sentiment_shift(0.9, 2, 0.0, 5, 0.3, 3),
            None,
            "recent_n=2 < min_articles=3"
        );
    }
}
