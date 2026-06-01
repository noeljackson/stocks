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
    } else if industry.contains("Software") || sector == "Technology" {
        "software and technology platform inflections"
    } else if industry.contains("Electrical")
        || industry.contains("Utility")
        || industry.contains("Utilities")
        || sector == "Utilities"
    {
        "AI power, grid, and cooling bottlenecks"
    } else if industry.contains("Engineering & Construction") {
        "AI power, grid, and data-center construction"
    } else if is_grid_materials_industry(industry) {
        "industrial metals and infrastructure inputs"
    } else if is_agriculture_industry(industry) || sector == "Consumer Defensive" {
        "agriculture, food, and staples pricing power"
    } else if sector == "Financial Services" {
        "credit, liquidity, and rate-sensitive financials"
    } else if sector == "Energy" {
        "energy supply, inflation, and industrial input costs"
    } else if sector == "Healthcare" {
        "healthcare defensives and product/regulatory inflections"
    } else if sector == "Real Estate" {
        "real assets, financing costs, and capacity constraints"
    } else if sector == "Consumer Cyclical" {
        "consumer cycle and discretionary demand inflections"
    } else if sector == "Communication Services" {
        "media, advertising, and connectivity demand inflections"
    } else {
        "cross-sector opportunity radar"
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
    } else if industry.contains("Software") || sector == "Technology" {
        "software and platform shifts can re-rate revenue durability, margins, or adoption"
    } else if industry.contains("Electrical")
        || industry.contains("Utility")
        || industry.contains("Utilities")
        || sector == "Utilities"
    {
        "data-center growth is increasingly gated by power availability and thermal management"
    } else if industry.contains("Engineering & Construction") {
        "AI data centers require grid interconnect, power construction, and thermal infrastructure"
    } else if is_grid_materials_industry(industry) {
        "commodity and infrastructure cycles can create large revisions when supply/demand shifts"
    } else if is_agriculture_industry(industry) || sector == "Consumer Defensive" {
        "food, crop, input-cost, and brand pricing cycles can move earnings and inflation expectations"
    } else if sector == "Financial Services" {
        "credit quality, deposit costs, rates, and capital markets activity can inflect faster than consensus"
    } else if sector == "Energy" {
        "supply discipline, geopolitics, inventories, and demand can move cash flows and inflation inputs"
    } else if sector == "Healthcare" {
        "regulatory, reimbursement, trial, and product-cycle events can create company-specific edges"
    } else if sector == "Real Estate" {
        "cap rates, financing costs, occupancy, and asset scarcity can change faster than stale NAV assumptions"
    } else if sector == "Consumer Cyclical" {
        "consumer demand, inventory, and margin cycles can inflect before broad estimates adjust"
    } else if sector == "Communication Services" {
        "advertising, subscriber, content, and network cycles can create measurable revisions"
    } else {
        "it is a liquid pool member with enough local evidence to deserve a first-pass review"
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
    } else if industry.contains("Software") || sector == "Technology" {
        vec!["Technology", "Software Infrastructure"]
    } else if industry.contains("Electrical")
        || industry.contains("Utility")
        || industry.contains("Utilities")
        || sector == "Utilities"
    {
        vec!["AI Power/Grid"]
    } else if industry.contains("Engineering & Construction") {
        vec!["AI Power/Grid", "Data Centers"]
    } else if is_grid_materials_industry(industry) {
        vec!["Metals/Materials", "Cyclicals"]
    } else if is_agriculture_industry(industry) || sector == "Consumer Defensive" {
        vec!["Staples/Agriculture", "Inflation"]
    } else if sector == "Financial Services" {
        vec!["Financials", "Macro/Rates"]
    } else if sector == "Energy" {
        vec!["Energy", "Inflation"]
    } else if sector == "Healthcare" {
        vec!["Healthcare"]
    } else if sector == "Real Estate" {
        vec!["Real Estate", "Macro/Rates"]
    } else if sector == "Consumer Cyclical" {
        vec!["Consumer Cycle"]
    } else if sector == "Communication Services" {
        vec!["Communication Services"]
    } else {
        vec!["Cross-Sector Opportunities"]
    }
}

fn is_grid_materials_industry(industry: &str) -> bool {
    industry.contains("Copper")
        || industry.contains("Steel")
        || industry.contains("Aluminum")
        || industry.contains("Industrial Metals")
        || industry.contains("Other Industrial Metals")
        || industry.contains("Silver")
        || industry.contains("Gold")
        || industry.contains("Metals")
        || industry.contains("Mining")
        || industry.contains("Construction Materials")
        || industry.contains("Industrial Materials")
        || industry.contains("Chemicals")
}

fn is_agriculture_industry(industry: &str) -> bool {
    industry.contains("Agricultural")
        || industry.contains("Farm")
        || industry.contains("Food")
        || industry.contains("Packaged Foods")
}

fn nomination_domain_fit(sector: Option<&str>, industry: Option<&str>) -> f64 {
    let sector = sector.unwrap_or_default();
    let industry = industry.unwrap_or_default();
    if industry.contains("Semiconductor") {
        88.0
    } else if industry.contains("Communication Equipment") || industry.contains("Computer Hardware")
    {
        82.0
    } else if industry.contains("Software") || sector == "Technology" {
        70.0
    } else if industry.contains("Electrical")
        || industry.contains("Utility")
        || industry.contains("Utilities")
        || sector == "Utilities"
    {
        76.0
    } else if industry.contains("Engineering & Construction") {
        68.0
    } else if is_grid_materials_industry(industry) {
        72.0
    } else if is_agriculture_industry(industry) || sector == "Consumer Defensive" {
        70.0
    } else if sector == "Energy" {
        68.0
    } else if sector == "Basic Materials" {
        62.0
    } else if sector == "Financial Services" {
        62.0
    } else if sector == "Real Estate" {
        58.0
    } else if sector == "Healthcare"
        || sector == "Consumer Cyclical"
        || sector == "Communication Services"
    {
        55.0
    } else {
        45.0
    }
}

fn nomination_tier(domain_fit: f64) -> i32 {
    if domain_fit >= 80.0 {
        1
    } else if domain_fit >= 60.0 {
        2
    } else {
        3
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
    use crate::attention::{initial_assignment, kind, severity, source, title_for_candidate};

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
        let domain_fit = nomination_domain_fit(sector.as_deref(), industry.as_deref());
        let proposed_tier = nomination_tier(domain_fit);
        let inserted = sqlx::query(
            r#"INSERT INTO discovery_candidate
                 (symbol, signal_name, signal_value, domain_fit, proposed_tier, reasoning, config_version)
               SELECT $1, $2, $3, $4, $5, $6, $7
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
        .bind(domain_fit)
        .bind(proposed_tier)
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
            "domain_fit": domain_fit,
            "proposed_tier": proposed_tier,
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
        let (fsm_state, owner) =
            initial_assignment(kind::CANDIDATE_REVIEW, severity::REVIEW, source::DISCOVERY);
        sqlx::query(
            r#"INSERT INTO attention_item
                 (kind, symbol, candidate_id, severity, title, reason, source, source_ref,
                  fsm_state, owner, state_reason)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8::jsonb, $9, $10, $11)
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
        .bind(fsm_state)
        .bind(owner)
        .bind(RESEARCH_NOMINATION_SIGNAL)
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
        assert!(reason.contains("software and technology platform inflections"));
        assert!(reason.contains("price, news, estimates, fundamentals"));
    }

    #[test]
    fn nomination_scope_handles_commodities_and_financials() {
        assert_eq!(
            nomination_theme(Some("Basic Materials"), Some("Copper")),
            "industrial metals and infrastructure inputs"
        );
        assert_eq!(
            suggested_watchlists(Some("Consumer Defensive"), Some("Agricultural Inputs")),
            vec!["Staples/Agriculture", "Inflation"]
        );
        assert_eq!(
            nomination_theme(Some("Financial Services"), Some("Banks - Regional")),
            "credit, liquidity, and rate-sensitive financials"
        );
        assert!(nomination_domain_fit(Some("Consumer Defensive"), Some("Farm Products")) >= 70.0);
    }
}
