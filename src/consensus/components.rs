//! Per-component scorers. Each returns a 0..100 raw value + an attribution
//! status ("ok" if computed from real data, "no_data" otherwise).
//!
//! The four data-gated components (coverage, estimates, news, sentiment)
//! currently degrade to 0/"no_data" — they'll light up as #18, #19, #20
//! land. price_extension is fully functional today using #17's price_bar.

use anyhow::Result;
use sqlx::{Row, postgres::PgPool};

use super::ComponentScore;

// ---------- pure scoring functions (no I/O — testable) ----------

/// Score the price-extension component from technical inputs. SPEC §6.2
/// hints at "distance above MAs, RSI, parabolic volume." We use:
///   - pct above SMA200 (0% → 0, 30%+ → 100)
///   - RSI-14 (50 → 0, 90+ → 100)
///   - distance from 52w high (-30%+ → 0, at high → 100)
/// then average. Each input is clamped 0..100.
#[must_use]
pub fn price_extension_raw(
    pct_above_sma200: f64,
    rsi14: f64,
    pct_from_52w_high: f64,
) -> f64 {
    let a = clamp01((pct_above_sma200 / 30.0).clamp(0.0, 1.0)) * 100.0;
    let b = clamp01(((rsi14 - 50.0) / 40.0).clamp(0.0, 1.0)) * 100.0;
    // pct_from_52w_high is typically <= 0; at high (=0) → 100, at -30% → 0
    let dist = pct_from_52w_high.clamp(-30.0, 0.0);
    let c = ((dist + 30.0) / 30.0) * 100.0;
    (a + b + c) / 3.0
}

fn clamp01(x: f64) -> f64 {
    x.clamp(0.0, 1.0)
}

// ---------- async scorers (DB I/O) ----------

/// Compute price extension for a single symbol from the last ~200 price bars.
/// Returns 0/"no_data" if we don't have enough history.
pub async fn price_extension(pool: &PgPool, symbol: &str) -> Result<ComponentScore> {
    let rows = sqlx::query(
        r#"SELECT close::float8 AS close, ts
             FROM price_bar
            WHERE symbol = $1
         ORDER BY ts DESC
            LIMIT 260"#,
    )
    .bind(symbol)
    .fetch_all(pool)
    .await?;

    // Need at least 15 bars for RSI-14; degrade gracefully to SMA-of-what-we-have
    // + (incomplete-window) 52w high as price-ingest backfill expands over time.
    if rows.len() < 15 {
        return Ok(ComponentScore {
            name: "price_extension",
            raw: 0.0,
            weighted: 0.0,
            status: "no_data",
        });
    }
    let closes: Vec<f64> = rows.iter().map(|r| r.get::<f64, _>("close")).collect();
    let latest = closes[0];

    // pct above SMA200 (degrade to SMA-of-what-we-have if < 200 bars).
    let sma_n = closes.len().min(200);
    let sma = closes.iter().take(sma_n).sum::<f64>() / sma_n as f64;
    let pct_above_sma = if sma > 0.0 { (latest - sma) / sma * 100.0 } else { 0.0 };

    // RSI-14 (Wilder, simple form). Iterate oldest→newest over last 15 bars
    // for the seed period; reverse closes since they came DESC.
    let rsi = compute_rsi14(&closes);

    // Distance from 52w high (≈ 252 trading days; whatever we have).
    let high = closes.iter().take(252).cloned().fold(f64::MIN, f64::max);
    let pct_from_high = if high > 0.0 { (latest - high) / high * 100.0 } else { 0.0 };

    let raw = price_extension_raw(pct_above_sma, rsi, pct_from_high);
    Ok(ComponentScore { name: "price_extension", raw, weighted: 0.0, status: "ok" })
}

/// Wilder's RSI-14 (standard, simple). Returns 50 when there isn't enough history.
fn compute_rsi14(closes_desc: &[f64]) -> f64 {
    if closes_desc.len() < 15 {
        return 50.0;
    }
    // Use the most-recent 15 bars (need 14 gains/losses); they came descending so reverse.
    let recent: Vec<f64> = closes_desc.iter().take(15).rev().cloned().collect();
    let mut gains = 0.0;
    let mut losses = 0.0;
    for w in recent.windows(2) {
        let change = w[1] - w[0];
        if change > 0.0 {
            gains += change;
        } else {
            losses -= change;
        }
    }
    if losses == 0.0 {
        return 100.0;
    }
    let rs = (gains / 14.0) / (losses / 14.0);
    100.0 - (100.0 / (1.0 + rs))
}

// ---------- degraded stubs (return zero with attribution) ----------

#[must_use]
pub fn coverage_expansion_stub() -> ComponentScore {
    ComponentScore { name: "coverage_expansion", raw: 0.0, weighted: 0.0, status: "no_data" }
}

#[must_use]
pub fn estimate_revision_saturation_stub() -> ComponentScore {
    ComponentScore { name: "estimate_revision_saturation", raw: 0.0, weighted: 0.0, status: "no_data" }
}

#[must_use]
pub fn mainstream_coverage_stub() -> ComponentScore {
    ComponentScore { name: "mainstream_coverage", raw: 0.0, weighted: 0.0, status: "no_data" }
}

#[must_use]
pub fn retail_attention_stub() -> ComponentScore {
    ComponentScore { name: "retail_attention", raw: 0.0, weighted: 0.0, status: "no_data" }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn price_extension_raw_extremes() {
        // All maxed → 100.
        assert_eq!(price_extension_raw(30.0, 90.0, 0.0), 100.0);
        // All bottomed → 0.
        assert_eq!(price_extension_raw(0.0, 50.0, -30.0), 0.0);
        // Mid: 15% above SMA (=50), RSI 70 (=50), -15% from high (=50) → 50.
        let r = price_extension_raw(15.0, 70.0, -15.0);
        assert!((r - 50.0).abs() < 1.0, "expected ~50, got {r}");
    }

    #[test]
    fn price_extension_raw_clamps_oob() {
        // Way above SMA + RSI 110 + above 52w (positive) — still 100, no overflow.
        assert_eq!(price_extension_raw(99.0, 110.0, 5.0), 100.0);
        // Way below SMA + RSI 10 + -50% from high — still 0, no underflow.
        assert_eq!(price_extension_raw(-50.0, 10.0, -50.0), 0.0);
    }

    #[test]
    fn rsi_max_when_no_losses() {
        // Strictly ascending → no losses → RSI = 100.
        let closes_desc: Vec<f64> = (1..=20).rev().map(|i| i as f64).collect();
        assert_eq!(compute_rsi14(&closes_desc), 100.0);
    }

    #[test]
    fn rsi_falls_back_to_50_with_short_history() {
        let closes_desc = vec![100.0, 99.0, 98.0]; // only 3 bars
        assert_eq!(compute_rsi14(&closes_desc), 50.0);
    }

    #[test]
    fn rsi_mid_for_flat_oscillation() {
        // Equal gains and losses → RSI = 50.
        let closes_desc: Vec<f64> = vec![
            100.0, 99.0, 100.0, 99.0, 100.0, 99.0, 100.0, 99.0,
            100.0, 99.0, 100.0, 99.0, 100.0, 99.0, 100.0,
        ];
        let r = compute_rsi14(&closes_desc);
        assert!((r - 50.0).abs() < 1.0, "expected ~50, got {r}");
    }

    #[test]
    fn degraded_stubs_attribute_no_data() {
        for s in [
            coverage_expansion_stub(),
            estimate_revision_saturation_stub(),
            mainstream_coverage_stub(),
            retail_attention_stub(),
        ] {
            assert_eq!(s.raw, 0.0);
            assert_eq!(s.status, "no_data");
        }
    }
}
