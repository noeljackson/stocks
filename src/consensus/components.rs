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
pub fn price_extension_raw(pct_above_sma200: f64, rsi14: f64, pct_from_52w_high: f64) -> f64 {
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
    let pct_above_sma = if sma > 0.0 {
        (latest - sma) / sma * 100.0
    } else {
        0.0
    };

    // RSI-14 (Wilder, simple form). Iterate oldest→newest over last 15 bars
    // for the seed period; reverse closes since they came DESC.
    let rsi = compute_rsi14(&closes);

    // Distance from 52w high (≈ 252 trading days; whatever we have).
    let high = closes.iter().take(252).cloned().fold(f64::MIN, f64::max);
    let pct_from_high = if high > 0.0 {
        (latest - high) / high * 100.0
    } else {
        0.0
    };

    let raw = price_extension_raw(pct_above_sma, rsi, pct_from_high);
    Ok(ComponentScore {
        name: "price_extension",
        raw,
        weighted: 0.0,
        status: "ok",
    })
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

// ---------- pure scoring functions for the data-driven components ----------

/// Score from estimate revisions over a lookback window.
/// The MORE one-directional revision activity, the HIGHER consensus is being
/// "saturated" (everyone's catching on → we're getting closer to the late-
/// crowd line per SPEC §0). Mixed/coverage_change revisions don't count.
///
/// Saturation curve: 0 net revisions → 0; 1 → 30; 3 → 60; 5+ → 100.
/// Negative `net` (more down than up) gives a negative-signed raw score —
/// the consensus computation treats absolute magnitude as "saturation".
#[must_use]
pub fn estimate_revision_saturation_raw(up_count: u32, down_count: u32) -> f64 {
    let net = (up_count as i32) - (down_count as i32);
    let abs = net.unsigned_abs() as f64;
    let saturation = if abs >= 5.0 { 100.0 } else { abs * 20.0 };
    if net >= 0 { saturation } else { -saturation }
}

/// Score from news coverage breadth over a lookback window.
/// More distinct publishers mentioning a ticker = closer to mainstream
/// attention. Curve: 1 publisher → 20; 3 → 60; 5+ → 100.
#[must_use]
pub fn mainstream_coverage_raw(distinct_publishers: u32) -> f64 {
    let p = distinct_publishers as f64;
    if p >= 5.0 { 100.0 } else { p * 20.0 }
}

/// Score from rate of new publishers picking up the story (delta vs prior
/// window). New_pubs is "publishers in last N days who didn't cover in prior
/// N days." Curve: 0 → 0; 2 → 40; 5+ → 100.
#[must_use]
pub fn coverage_expansion_raw(new_publishers: u32) -> f64 {
    let p = new_publishers as f64;
    if p >= 5.0 { 100.0 } else { p * 20.0 }
}

#[must_use]
pub fn recent_news_fresh_enough(article_count_14d: u32) -> bool {
    article_count_14d >= 3
}

// ---------- async scorers (DB I/O) — read from the new tables ----------

/// Net up-vs-down revisions in the last 14 days → saturation curve.
/// Returns no_data if there are zero revisions for this symbol in the window.
pub async fn estimate_revision_saturation(pool: &PgPool, symbol: &str) -> Result<ComponentScore> {
    let row = sqlx::query(
        r#"SELECT
             count(*) FILTER (WHERE direction = 'up')   AS up_count,
             count(*) FILTER (WHERE direction = 'down') AS down_count,
             count(*) AS total
           FROM estimate_revision
          WHERE symbol = $1
            AND direction IN ('up','down')
            AND detected_at > now() - interval '14 days'"#,
    )
    .bind(symbol)
    .fetch_one(pool)
    .await?;
    let up: i64 = row.try_get("up_count")?;
    let down: i64 = row.try_get("down_count")?;
    let total: i64 = row.try_get("total")?;
    if total == 0 {
        return Ok(ComponentScore {
            name: "estimate_revision_saturation",
            raw: 0.0,
            weighted: 0.0,
            status: "no_data",
        });
    }
    let raw = estimate_revision_saturation_raw(up as u32, down as u32);
    Ok(ComponentScore {
        name: "estimate_revision_saturation",
        raw,
        weighted: 0.0,
        status: "ok",
    })
}

/// Distinct publishers carrying news on this ticker in the last 7 days.
pub async fn mainstream_coverage(pool: &PgPool, symbol: &str) -> Result<ComponentScore> {
    let row = sqlx::query(
        r#"SELECT COUNT(DISTINCT publisher) FILTER (
                     WHERE publisher IS NOT NULL
                       AND published_at > now() - interval '7 days'
                   ) AS pubs,
                  COUNT(*) FILTER (
                     WHERE published_at > now() - interval '14 days'
                   ) AS articles_14d
             FROM news_article
            WHERE symbol = $1"#,
    )
    .bind(symbol)
    .fetch_one(pool)
    .await?;
    let pubs: i64 = row.try_get("pubs")?;
    let articles_14d: i64 = row.try_get("articles_14d")?;
    if !recent_news_fresh_enough(articles_14d.max(0) as u32) || pubs == 0 {
        return Ok(ComponentScore {
            name: "mainstream_coverage",
            raw: 0.0,
            weighted: 0.0,
            status: "no_data",
        });
    }
    let raw = mainstream_coverage_raw(pubs as u32);
    Ok(ComponentScore {
        name: "mainstream_coverage",
        raw,
        weighted: 0.0,
        status: "ok",
    })
}

/// Publishers in the last 7d that didn't cover the same ticker in the prior 7d.
/// Captures the "story is spreading" velocity.
pub async fn coverage_expansion(pool: &PgPool, symbol: &str) -> Result<ComponentScore> {
    let row = sqlx::query(
        r#"WITH recent AS (
              SELECT DISTINCT publisher FROM news_article
               WHERE symbol = $1 AND publisher IS NOT NULL
                 AND published_at > now() - interval '7 days'
           ), prior AS (
              SELECT DISTINCT publisher FROM news_article
               WHERE symbol = $1 AND publisher IS NOT NULL
                 AND published_at > now() - interval '14 days'
                 AND published_at <= now() - interval '7 days'
           ), coverage AS (
              SELECT count(*) AS articles_14d FROM news_article
               WHERE symbol = $1
                 AND published_at > now() - interval '14 days'
           )
           SELECT (SELECT count(*) FROM recent
                    WHERE publisher NOT IN (SELECT publisher FROM prior)) AS new_pubs,
                  coverage.articles_14d
             FROM coverage"#,
    )
    .bind(symbol)
    .fetch_one(pool)
    .await?;
    let new_pubs: i64 = row.try_get("new_pubs")?;
    let articles_14d: i64 = row.try_get("articles_14d")?;
    if !recent_news_fresh_enough(articles_14d.max(0) as u32) {
        return Ok(ComponentScore {
            name: "coverage_expansion",
            raw: 0.0,
            weighted: 0.0,
            status: "no_data",
        });
    }
    let raw = coverage_expansion_raw(new_pubs as u32);
    Ok(ComponentScore {
        name: "coverage_expansion",
        raw,
        weighted: 0.0,
        status: "ok",
    })
}

// ---------- retail_attention from macro crowd-sentiment markers (#20) ----------

/// Compose a retail-attention raw score from the latest crowd markers.
///
/// Inputs each map to a 0..100 sub-score; we average across what's available.
/// Result is positive (higher = more retail attention/risk-on crowd state).
///
/// - VIX: high VIX (>30) → 0 (fear), low VIX (<15) → 100 (complacency).
///   Curve linear between 15 and 30.
/// - Equity P/C ratio: high (>1.0) → 0 (hedging), low (<0.5) → 100
///   (calls dominate, speculative). Curve linear between 0.5 and 1.0.
#[must_use]
pub fn retail_attention_raw(vix_close: Option<f64>, equity_pcr: Option<f64>) -> Option<f64> {
    let vix_sub = vix_close.map(|v| {
        let clamped = v.clamp(15.0, 30.0);
        100.0 * (30.0 - clamped) / 15.0
    });
    let pcr_sub = equity_pcr.map(|p| {
        let clamped = p.clamp(0.5, 1.0);
        100.0 * (1.0 - clamped) / 0.5
    });
    let mut subs = Vec::new();
    if let Some(s) = vix_sub {
        subs.push(s);
    }
    if let Some(s) = pcr_sub {
        subs.push(s);
    }
    if subs.is_empty() {
        None
    } else {
        Some(subs.iter().sum::<f64>() / subs.len() as f64)
    }
}

/// Read the latest crowd_sentiment markers and compose retail_attention.
pub async fn retail_attention(pool: &PgPool) -> Result<ComponentScore> {
    let vix: Option<f64> = sqlx::query_scalar(
        "SELECT value::float8 FROM crowd_sentiment
          WHERE source='cboe_vix' AND metric='vix_close'
       ORDER BY observed_at DESC LIMIT 1",
    )
    .fetch_optional(pool)
    .await?;
    let pcr: Option<f64> = sqlx::query_scalar(
        "SELECT value::float8 FROM crowd_sentiment
          WHERE source='cboe_pcr' AND metric='equity_pcr'
       ORDER BY observed_at DESC LIMIT 1",
    )
    .fetch_optional(pool)
    .await?;
    match retail_attention_raw(vix, pcr) {
        Some(raw) => Ok(ComponentScore {
            name: "retail_attention",
            raw,
            weighted: 0.0,
            status: "ok",
        }),
        None => Ok(ComponentScore {
            name: "retail_attention",
            raw: 0.0,
            weighted: 0.0,
            status: "no_data",
        }),
    }
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
            100.0, 99.0, 100.0, 99.0, 100.0, 99.0, 100.0, 99.0, 100.0, 99.0, 100.0, 99.0, 100.0,
            99.0, 100.0,
        ];
        let r = compute_rsi14(&closes_desc);
        assert!((r - 50.0).abs() < 1.0, "expected ~50, got {r}");
    }

    #[test]
    fn retail_attention_raw_blends_vix_and_pcr() {
        // VIX 15 (low, complacency) = 100; PCR 0.5 (calls dominate) = 100 → avg 100
        assert_eq!(retail_attention_raw(Some(15.0), Some(0.5)), Some(100.0));
        // VIX 30 (fear) = 0; PCR 1.0 (hedging) = 0 → avg 0
        assert_eq!(retail_attention_raw(Some(30.0), Some(1.0)), Some(0.0));
        // Single input alone: VIX 22.5 → 50, no PCR → just 50
        assert_eq!(retail_attention_raw(Some(22.5), None), Some(50.0));
    }

    #[test]
    fn retail_attention_raw_clamps_extremes() {
        // VIX 50 (way past 30 cap) → 0
        assert_eq!(retail_attention_raw(Some(50.0), None), Some(0.0));
        // VIX 10 (way below 15) → 100
        assert_eq!(retail_attention_raw(Some(10.0), None), Some(100.0));
        // PCR 0.2 (way below 0.5) → 100
        assert_eq!(retail_attention_raw(None, Some(0.2)), Some(100.0));
    }

    #[test]
    fn retail_attention_raw_none_when_no_inputs() {
        assert_eq!(retail_attention_raw(None, None), None);
    }

    #[test]
    fn estimate_saturation_curve_increases_with_net() {
        assert_eq!(estimate_revision_saturation_raw(0, 0), 0.0);
        assert_eq!(estimate_revision_saturation_raw(1, 0), 20.0);
        assert_eq!(estimate_revision_saturation_raw(3, 0), 60.0);
        assert_eq!(estimate_revision_saturation_raw(5, 0), 100.0);
        assert_eq!(
            estimate_revision_saturation_raw(10, 0),
            100.0,
            "clamps at 100"
        );
    }

    #[test]
    fn estimate_saturation_signs_negative_when_down_dominates() {
        assert_eq!(estimate_revision_saturation_raw(0, 3), -60.0);
        // Mixed cancels: 3 up + 3 down = net 0 → 0
        assert_eq!(estimate_revision_saturation_raw(3, 3), 0.0);
        // 1 up + 4 down → net -3 → -60
        assert_eq!(estimate_revision_saturation_raw(1, 4), -60.0);
    }

    #[test]
    fn mainstream_coverage_curve_scales_with_publishers() {
        assert_eq!(mainstream_coverage_raw(0), 0.0);
        assert_eq!(mainstream_coverage_raw(1), 20.0);
        assert_eq!(mainstream_coverage_raw(3), 60.0);
        assert_eq!(mainstream_coverage_raw(5), 100.0);
        assert_eq!(mainstream_coverage_raw(100), 100.0, "clamps at 100");
    }

    #[test]
    fn coverage_expansion_curve_scales_with_new_publishers() {
        assert_eq!(coverage_expansion_raw(0), 0.0);
        assert_eq!(coverage_expansion_raw(2), 40.0);
        assert_eq!(coverage_expansion_raw(5), 100.0);
    }

    #[test]
    fn recent_news_freshness_requires_three_articles_in_two_weeks() {
        assert!(!recent_news_fresh_enough(0));
        assert!(!recent_news_fresh_enough(2));
        assert!(recent_news_fresh_enough(3));
    }
}
