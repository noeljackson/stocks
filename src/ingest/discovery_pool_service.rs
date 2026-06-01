//! discovery_pool refresh service (#88).
//!
//! Once per day:
//!   1. Fetch from FMP screener → ScreenerRow list
//!   2. Upsert each row into discovery_pool (re-stamps last_seen_at)
//!   3. Mark rows that didn't show up this pass as dropped_at = now()
//!      (without deleting — preserves history)

use std::time::Duration;

use anyhow::{Context, Result};
use sqlx::{Row, postgres::PgPool};
use tracing::{info, warn};

use super::fmp_screener::FmpScreenerAdapter;
use super::{rate_limit, source_health};

const MIN_MARKET_CAP: i64 = 5_000_000_000; // $5B floor
const MAX_POOL_INSPECTIONS_PER_PASS: i64 = 25;
const POOL_INSPECTION_SIGNAL: &str = "pool_inspection";
const POOL_INSPECTION_CONFIG_VERSION: &str = "pool_inspection:v1";

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PoolInspectionEligibility {
    dropped: bool,
    has_active_ticker: bool,
    has_context: bool,
    has_thesis: bool,
    has_prior_pool_inspection: bool,
}

#[cfg(test)]
#[must_use]
fn eligible_for_pool_inspection(row: &PoolInspectionEligibility) -> bool {
    !row.dropped
        && !row.has_active_ticker
        && !row.has_context
        && !row.has_thesis
        && !row.has_prior_pool_inspection
}

#[must_use]
fn pool_inspection_reason(
    company_name: Option<&str>,
    sector: Option<&str>,
    industry: Option<&str>,
    market_cap: Option<i64>,
    has_price: bool,
    has_news: bool,
    has_estimates: bool,
) -> String {
    let company = company_name.unwrap_or("This symbol");
    let scope = match (sector, industry) {
        (Some(sector), Some(industry)) => format!("{sector}/{industry}"),
        (Some(sector), None) => sector.to_string(),
        (None, Some(industry)) => industry.to_string(),
        (None, None) => "the AI-infrastructure discovery pool".to_string(),
    };
    let cap = market_cap
        .filter(|cap| *cap > 0)
        .map(|cap| format!(", market cap ${:.1}B", cap as f64 / 1_000_000_000.0))
        .unwrap_or_default();
    let mut data = Vec::new();
    if has_price {
        data.push("price");
    }
    if has_news {
        data.push("news");
    }
    if has_estimates {
        data.push("estimates");
    }
    let data_label = if data.is_empty() {
        "no stored price/news/estimates yet".to_string()
    } else {
        data.join(", ")
    };
    format!(
        "Proactive research inspection: {company} is an unreviewed {scope} pool member{cap}. Queued so context/thesis can inspect relevance, not because a trade signal fired. Available data: {data_label}."
    )
}

pub async fn run_once(
    pool: &PgPool,
    adapter: &FmpScreenerAdapter,
) -> Result<(usize, usize, usize, usize)> {
    source_health::mark_started(pool, "fmp_screener", 0).await?;
    let rows = adapter.fetch_pool(MIN_MARKET_CAP).await?;
    if rows.is_empty() {
        source_health::record_success(pool, "fmp_screener", 0, 0, 0, 0).await?;
        return Ok((0, 0, 0, 0));
    }
    let now_seen = chrono::Utc::now();
    let mut inserted = 0usize;
    let mut refreshed = 0usize;

    let mut tx = pool.begin().await.context("begin tx")?;
    for r in &rows {
        let res = sqlx::query(
            r#"INSERT INTO discovery_pool
                 (symbol, company_name, sector, industry, market_cap, last_seen_at, first_seen_at, dropped_at)
               VALUES ($1, $2, $3, $4, $5, $6, $6, NULL)
               ON CONFLICT (symbol) DO UPDATE SET
                 company_name = COALESCE(EXCLUDED.company_name, discovery_pool.company_name),
                 sector       = COALESCE(EXCLUDED.sector,       discovery_pool.sector),
                 industry     = COALESCE(EXCLUDED.industry,     discovery_pool.industry),
                 market_cap   = COALESCE(EXCLUDED.market_cap,   discovery_pool.market_cap),
                 last_seen_at = EXCLUDED.last_seen_at,
                 dropped_at   = NULL"#,
        )
        .bind(&r.symbol)
        .bind(&r.company_name)
        .bind(&r.sector)
        .bind(&r.industry)
        .bind(r.market_cap)
        .bind(now_seen)
        .execute(&mut *tx)
        .await
        .context("upsert discovery_pool")?;
        if res.rows_affected() > 0 {
            refreshed += 1;
            if res.rows_affected() == 1 { /* could be insert or update; count both */ }
        }
        inserted += 1;
    }
    // Mark drops: anything that was active before and didn't show up this pass.
    let dropped = sqlx::query(
        "UPDATE discovery_pool SET dropped_at = now()
          WHERE dropped_at IS NULL AND last_seen_at < $1",
    )
    .bind(now_seen)
    .execute(&mut *tx)
    .await
    .context("mark drops")?
    .rows_affected() as usize;
    tx.commit().await.context("commit tx")?;
    source_health::record_success(
        pool,
        "fmp_screener",
        rows.len() as i64,
        refreshed as i64,
        0,
        0,
    )
    .await?;
    let inspections = match queue_pool_inspections(pool, MAX_POOL_INSPECTIONS_PER_PASS).await {
        Ok(n) => n,
        Err(e) => {
            warn!(error = %e, "pool inspection candidate queue failed");
            0
        }
    };
    Ok((inserted, refreshed, dropped, inspections))
}

async fn queue_pool_inspections(pool: &PgPool, limit: i64) -> Result<usize> {
    use crate::attention::{kind, severity, source, title_for_candidate};

    let rows = sqlx::query(
        r#"WITH eligible AS (
             SELECT dp.symbol,
                    dp.company_name,
                    dp.sector,
                    dp.industry,
                    dp.market_cap,
                    dp.first_seen_at,
                    EXISTS (SELECT 1 FROM price_bar pb WHERE pb.symbol = dp.symbol) AS has_price,
                    EXISTS (SELECT 1 FROM news_article na WHERE na.symbol = dp.symbol) AS has_news,
                    EXISTS (SELECT 1 FROM estimate_snapshot es WHERE es.symbol = dp.symbol) AS has_estimates
               FROM discovery_pool dp
              WHERE dp.dropped_at IS NULL
                AND NOT EXISTS (
                      SELECT 1 FROM ticker t
                       WHERE t.symbol = dp.symbol
                         AND t.status = 'active'
                    )
                AND NOT EXISTS (
                      SELECT 1 FROM ticker_context tc
                       WHERE tc.symbol = dp.symbol
                    )
                AND NOT EXISTS (
                      SELECT 1 FROM thesis th
                       WHERE th.symbol = dp.symbol
                    )
                AND NOT EXISTS (
                      SELECT 1 FROM discovery_candidate dc
                       WHERE dc.symbol = dp.symbol
                         AND dc.signal_name = $1
                    )
           )
           SELECT *,
                  ((CASE WHEN has_price THEN 1 ELSE 0 END) +
                   (CASE WHEN has_news THEN 1 ELSE 0 END) +
                   (CASE WHEN has_estimates THEN 1 ELSE 0 END)) AS data_score
             FROM eligible
            ORDER BY data_score DESC,
                     COALESCE(market_cap, 0) DESC,
                     first_seen_at DESC,
                     symbol ASC
            LIMIT $2"#,
    )
    .bind(POOL_INSPECTION_SIGNAL)
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("load pool inspection candidates")?;

    let mut created = 0usize;
    for row in rows {
        let symbol: String = row.try_get("symbol")?;
        let company_name: Option<String> = row.try_get("company_name")?;
        let sector: Option<String> = row.try_get("sector")?;
        let industry: Option<String> = row.try_get("industry")?;
        let market_cap: Option<i64> = row.try_get("market_cap")?;
        let has_price: bool = row.try_get("has_price")?;
        let has_news: bool = row.try_get("has_news")?;
        let has_estimates: bool = row.try_get("has_estimates")?;
        let data_score: i32 = row.try_get("data_score")?;
        let reasoning = pool_inspection_reason(
            company_name.as_deref(),
            sector.as_deref(),
            industry.as_deref(),
            market_cap,
            has_price,
            has_news,
            has_estimates,
        );
        let inserted = sqlx::query(
            r#"INSERT INTO discovery_candidate
                 (symbol, signal_name, signal_value, reasoning, config_version)
               SELECT $1, $2, $3, $4, $5
                WHERE NOT EXISTS (
                      SELECT 1 FROM discovery_candidate
                       WHERE symbol = $1
                         AND signal_name = $2
                    )
               RETURNING id"#,
        )
        .bind(&symbol)
        .bind(POOL_INSPECTION_SIGNAL)
        .bind(data_score as f64)
        .bind(&reasoning)
        .bind(POOL_INSPECTION_CONFIG_VERSION)
        .fetch_optional(pool)
        .await
        .with_context(|| format!("insert pool inspection candidate for {symbol}"))?;

        let Some(inserted) = inserted else {
            continue;
        };
        let candidate_id: i64 = inserted.try_get("id")?;
        let source_ref = serde_json::json!({
            "candidate_id": candidate_id,
            "signal_value": data_score,
            "interpretation_kind": POOL_INSPECTION_SIGNAL,
            "raw_signals": [],
            "config_version": POOL_INSPECTION_CONFIG_VERSION,
            "company_name": company_name,
            "sector": sector,
            "industry": industry,
            "market_cap": market_cap,
            "available_data": {
                "price": has_price,
                "news": has_news,
                "estimates": has_estimates
            }
        });
        sqlx::query(
            r#"INSERT INTO attention_item
                 (kind, symbol, candidate_id, severity, title, reason, source, source_ref)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8::jsonb)
               ON CONFLICT DO NOTHING"#,
        )
        .bind(kind::CANDIDATE_REVIEW)
        .bind(&symbol)
        .bind(candidate_id)
        .bind(severity::REVIEW)
        .bind(title_for_candidate(&symbol, POOL_INSPECTION_SIGNAL))
        .bind(&reasoning)
        .bind(source::DISCOVERY)
        .bind(source_ref)
        .execute(pool)
        .await
        .with_context(|| format!("insert pool inspection attention for {symbol}"))?;
        created += 1;
    }
    Ok(created)
}

pub async fn run(pool: PgPool, adapter: FmpScreenerAdapter, interval: Duration) -> Result<()> {
    info!(
        interval_secs = interval.as_secs(),
        "discovery_pool service started"
    );
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        match run_once(&pool, &adapter).await {
            Ok((seen, refreshed, dropped, inspections)) if seen > 0 => {
                info!(
                    seen,
                    refreshed, dropped, inspections, "discovery_pool pass complete"
                );
            }
            Ok(_) => {}
            Err(e) => {
                let message = e.to_string();
                let retry_after_at = if source_health::failure_kind(&message) == "rate_limited" {
                    rate_limit::fmp().retry_after_at().await
                } else {
                    None
                };
                if let Err(record_err) = source_health::record_failure(
                    &pool,
                    "fmp_screener",
                    source_health::failure_kind(&message),
                    &message,
                    retry_after_at,
                )
                .await
                {
                    warn!(error = %record_err, "fmp_screener source health failure record failed");
                }
                warn!(error = %e, "discovery_pool pass failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eligibility() -> PoolInspectionEligibility {
        PoolInspectionEligibility {
            dropped: false,
            has_active_ticker: false,
            has_context: false,
            has_thesis: false,
            has_prior_pool_inspection: false,
        }
    }

    #[test]
    fn pool_inspection_requires_unpromoted_uninspected_pool_member() {
        assert!(eligible_for_pool_inspection(&eligibility()));

        assert!(!eligible_for_pool_inspection(&PoolInspectionEligibility {
            has_active_ticker: true,
            ..eligibility()
        }));
        assert!(!eligible_for_pool_inspection(&PoolInspectionEligibility {
            has_context: true,
            ..eligibility()
        }));
        assert!(!eligible_for_pool_inspection(&PoolInspectionEligibility {
            has_thesis: true,
            ..eligibility()
        }));
        assert!(!eligible_for_pool_inspection(&PoolInspectionEligibility {
            has_prior_pool_inspection: true,
            ..eligibility()
        }));
        assert!(!eligible_for_pool_inspection(&PoolInspectionEligibility {
            dropped: true,
            ..eligibility()
        }));
    }

    #[test]
    fn pool_inspection_reason_distinguishes_research_from_signal() {
        let reason = pool_inspection_reason(
            Some("CoreWeave, Inc."),
            Some("Technology"),
            Some("Software - Infrastructure"),
            Some(59_800_000_000),
            true,
            true,
            true,
        );
        assert!(reason.contains("Proactive research inspection"));
        assert!(reason.contains("not because a trade signal fired"));
        assert!(reason.contains("price, news, estimates"));
    }
}
