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
const MAX_RESEARCH_NOMINATIONS_PER_PASS: i64 = 25;
const RESEARCH_NOMINATION_SIGNAL: &str = "research_nomination";
const RESEARCH_NOMINATION_CONFIG_VERSION: &str = "research_nomination:v1";
const MIN_NOMINATION_EVIDENCE_SCORE: i32 = 2;

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ResearchNominationEligibility {
    dropped: bool,
    has_active_ticker: bool,
    has_context: bool,
    has_thesis: bool,
    has_prior_research_nomination: bool,
}

#[cfg(test)]
#[must_use]
fn eligible_for_research_nomination(row: &ResearchNominationEligibility) -> bool {
    !row.dropped
        && !row.has_active_ticker
        && !row.has_context
        && !row.has_thesis
        && !row.has_prior_research_nomination
}

#[must_use]
fn research_nomination_reason(
    company_name: Option<&str>,
    sector: Option<&str>,
    industry: Option<&str>,
    market_cap: Option<i64>,
    has_price: bool,
    has_news: bool,
    has_estimates: bool,
    has_fundamentals: bool,
) -> String {
    let company = company_name.unwrap_or("This symbol");
    let theme = nomination_theme(sector, industry);
    let business_fit = nomination_business_fit(sector, industry);
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
    if has_fundamentals {
        data.push("fundamentals");
    }
    let data_label = if data.is_empty() {
        "no stored evidence yet".to_string()
    } else {
        data.join(", ")
    };
    format!(
        "Research nomination: {company} fits {theme} because {business_fit}{cap}. Available evidence: {data_label}. Confirming adds it to the monitored universe/watchlists and runs context/thesis; this is not a trade signal."
    )
}

fn nomination_theme(sector: Option<&str>, industry: Option<&str>) -> &'static str {
    let sector = sector.unwrap_or_default();
    let industry = industry.unwrap_or_default();
    if industry.contains("Semiconductor") {
        "AI compute and semicap supply"
    } else if industry.contains("Communication Equipment") || industry.contains("Computer Hardware")
    {
        "AI networking, optics, and hardware infrastructure"
    } else if industry.contains("Electrical")
        || industry.contains("Utility")
        || industry.contains("Utilities")
        || sector == "Utilities"
    {
        "AI power, grid, and cooling bottlenecks"
    } else if industry.contains("Copper") || sector == "Basic Materials" {
        "AI grid materials and copper bottlenecks"
    } else if industry.contains("REIT") {
        "data-center real estate and capacity"
    } else if industry.contains("Software") {
        "software infrastructure for AI/cloud operations"
    } else {
        "the tech-infrastructure watch universe"
    }
}

fn nomination_business_fit(sector: Option<&str>, industry: Option<&str>) -> &'static str {
    let sector = sector.unwrap_or_default();
    let industry = industry.unwrap_or_default();
    if industry.contains("Semiconductor") {
        "it sits in the compute, memory, or equipment stack that constrains AI capacity"
    } else if industry.contains("Communication Equipment") || industry.contains("Computer Hardware")
    {
        "AI clusters need faster interconnect, optics, storage, and server hardware"
    } else if industry.contains("Electrical")
        || industry.contains("Utility")
        || industry.contains("Utilities")
        || sector == "Utilities"
    {
        "data-center growth is increasingly gated by power availability and thermal management"
    } else if industry.contains("Copper") || sector == "Basic Materials" {
        "grid buildout and electrical equipment demand can transmit AI capex into materials"
    } else if industry.contains("REIT") {
        "leased data-center capacity can become a bottleneck when AI demand accelerates"
    } else if industry.contains("Software") {
        "AI infrastructure needs secure, observable, automated cloud/software operations"
    } else {
        "its sector/industry screen matched the infrastructure-adjacent discovery universe"
    }
}

fn suggested_watchlists(sector: Option<&str>, industry: Option<&str>) -> Vec<&'static str> {
    let sector = sector.unwrap_or_default();
    let industry = industry.unwrap_or_default();
    if industry.contains("Semiconductor") {
        vec!["AI Infrastructure", "Semiconductors"]
    } else if industry.contains("Communication Equipment") || industry.contains("Computer Hardware")
    {
        vec!["AI Infrastructure", "Networking/Optics"]
    } else if industry.contains("Electrical")
        || industry.contains("Utility")
        || industry.contains("Utilities")
        || sector == "Utilities"
    {
        vec!["AI Power/Grid"]
    } else if industry.contains("Copper") || sector == "Basic Materials" {
        vec!["Copper/Materials"]
    } else if industry.contains("REIT") {
        vec!["Data Centers"]
    } else if industry.contains("Software") {
        vec!["Software Infrastructure"]
    } else {
        vec!["AI Infrastructure"]
    }
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
    let nominations =
        match queue_research_nominations(pool, MAX_RESEARCH_NOMINATIONS_PER_PASS).await {
            Ok(n) => n,
            Err(e) => {
                warn!(error = %e, "research nomination candidate queue failed");
                0
            }
        };
    Ok((inserted, refreshed, dropped, nominations))
}

async fn queue_research_nominations(pool: &PgPool, limit: i64) -> Result<usize> {
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
                    EXISTS (SELECT 1 FROM estimate_snapshot es WHERE es.symbol = dp.symbol) AS has_estimates,
                    EXISTS (SELECT 1 FROM company_fact cf WHERE cf.symbol = dp.symbol) AS has_fundamentals
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
                   (CASE WHEN has_estimates THEN 1 ELSE 0 END) +
                   (CASE WHEN has_fundamentals THEN 1 ELSE 0 END)) AS data_score
             FROM eligible
            WHERE ((CASE WHEN has_price THEN 1 ELSE 0 END) +
                   (CASE WHEN has_news THEN 1 ELSE 0 END) +
                   (CASE WHEN has_estimates THEN 1 ELSE 0 END) +
                   (CASE WHEN has_fundamentals THEN 1 ELSE 0 END)) >= $2
            ORDER BY data_score DESC,
                     COALESCE(market_cap, 0) DESC,
                     first_seen_at DESC,
                     symbol ASC
            LIMIT $3"#,
    )
    .bind(RESEARCH_NOMINATION_SIGNAL)
    .bind(MIN_NOMINATION_EVIDENCE_SCORE)
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("load research nomination candidates")?;

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
        let has_fundamentals: bool = row.try_get("has_fundamentals")?;
        let data_score: i32 = row.try_get("data_score")?;
        let reasoning = research_nomination_reason(
            company_name.as_deref(),
            sector.as_deref(),
            industry.as_deref(),
            market_cap,
            has_price,
            has_news,
            has_estimates,
            has_fundamentals,
        );
        let theme = nomination_theme(sector.as_deref(), industry.as_deref());
        let business_fit = nomination_business_fit(sector.as_deref(), industry.as_deref());
        let suggested_watchlists = suggested_watchlists(sector.as_deref(), industry.as_deref());
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
        .bind(RESEARCH_NOMINATION_SIGNAL)
        .bind(data_score as f64)
        .bind(&reasoning)
        .bind(RESEARCH_NOMINATION_CONFIG_VERSION)
        .fetch_optional(pool)
        .await
        .with_context(|| format!("insert research nomination candidate for {symbol}"))?;

        let Some(inserted) = inserted else {
            continue;
        };
        let candidate_id: i64 = inserted.try_get("id")?;
        let source_ref = serde_json::json!({
            "candidate_id": candidate_id,
            "signal_value": data_score,
            "interpretation_kind": RESEARCH_NOMINATION_SIGNAL,
            "raw_signals": [],
            "config_version": RESEARCH_NOMINATION_CONFIG_VERSION,
            "company_name": company_name,
            "sector": sector,
            "industry": industry,
            "market_cap": market_cap,
            "nomination_reasons": {
                "theme": theme,
                "business_fit": business_fit,
                "suggested_watchlists": suggested_watchlists,
                "acceptance_effect": "add to monitored universe/watchlists and run context/thesis"
            },
            "available_data": {
                "price": has_price,
                "news": has_news,
                "estimates": has_estimates,
                "fundamentals": has_fundamentals
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
        .bind(title_for_candidate(&symbol, RESEARCH_NOMINATION_SIGNAL))
        .bind(&reasoning)
        .bind(source::DISCOVERY)
        .bind(source_ref)
        .execute(pool)
        .await
        .with_context(|| format!("insert research nomination attention for {symbol}"))?;
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
            Ok((seen, refreshed, dropped, nominations)) if seen > 0 => {
                info!(
                    seen,
                    refreshed, dropped, nominations, "discovery_pool pass complete"
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

    fn eligibility() -> ResearchNominationEligibility {
        ResearchNominationEligibility {
            dropped: false,
            has_active_ticker: false,
            has_context: false,
            has_thesis: false,
            has_prior_research_nomination: false,
        }
    }

    #[test]
    fn research_nomination_requires_unpromoted_unnominated_pool_member() {
        assert!(eligible_for_research_nomination(&eligibility()));

        assert!(!eligible_for_research_nomination(
            &ResearchNominationEligibility {
                has_active_ticker: true,
                ..eligibility()
            }
        ));
        assert!(!eligible_for_research_nomination(
            &ResearchNominationEligibility {
                has_context: true,
                ..eligibility()
            }
        ));
        assert!(!eligible_for_research_nomination(
            &ResearchNominationEligibility {
                has_thesis: true,
                ..eligibility()
            }
        ));
        assert!(!eligible_for_research_nomination(
            &ResearchNominationEligibility {
                has_prior_research_nomination: true,
                ..eligibility()
            }
        ));
        assert!(!eligible_for_research_nomination(
            &ResearchNominationEligibility {
                dropped: true,
                ..eligibility()
            }
        ));
    }

    #[test]
    fn research_nomination_reason_distinguishes_research_from_signal() {
        let reason = research_nomination_reason(
            Some("CoreWeave, Inc."),
            Some("Technology"),
            Some("Software - Infrastructure"),
            Some(59_800_000_000),
            true,
            true,
            true,
            true,
        );
        assert!(reason.contains("Research nomination"));
        assert!(reason.contains("not a trade signal"));
        assert!(reason.contains("software infrastructure"));
        assert!(reason.contains("price, news, estimates, fundamentals"));
    }
}
