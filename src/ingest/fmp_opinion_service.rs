//! Service loop for FMP analyst opinion (#116).

use std::{collections::BTreeSet, time::Duration};

use anyhow::{Context, Result};
use sqlx::Row;
use sqlx::postgres::PgPool;
use tracing::{info, warn};

use super::fmp_opinion::{
    FmpOpinionAdapter, PriceTargetConsensusRow, decode_consensus, decode_price_target_events,
    decode_recommendations, normalize_price_target_events, normalize_recommendations,
};
use super::{max_symbols_from_env, rate_limit, source_health};
use crate::platform::store::Store;

const OPINION_ACTIONS: [&str; 3] = [
    "fmp_price_target_consensus",
    "fmp_grades_historical",
    "fmp_price_target_news",
];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OpinionScanStats {
    consensus_rows: usize,
    recommendation_rows: usize,
    event_rows: usize,
    inserted_rows: usize,
}

impl OpinionScanStats {
    fn rows_seen(self) -> usize {
        self.consensus_rows + self.recommendation_rows + self.event_rows
    }
}

#[derive(Debug, Default)]
struct OpinionActionCoverage {
    consensus_symbols: BTreeSet<String>,
    recommendation_symbols: BTreeSet<String>,
    event_symbols: BTreeSet<String>,
}

impl OpinionActionCoverage {
    fn record(&mut self, symbol: &str, stats: OpinionScanStats) {
        if stats.consensus_rows > 0 {
            self.consensus_symbols.insert(symbol.to_string());
        }
        if stats.recommendation_rows > 0 {
            self.recommendation_symbols.insert(symbol.to_string());
        }
        if stats.event_rows > 0 {
            self.event_symbols.insert(symbol.to_string());
        }
    }

    fn symbols_for_action(&self, action: &str) -> Vec<String> {
        let symbols = match action {
            "fmp_price_target_consensus" => &self.consensus_symbols,
            "fmp_grades_historical" => &self.recommendation_symbols,
            "fmp_price_target_news" => &self.event_symbols,
            _ => return Vec::new(),
        };
        symbols.iter().cloned().collect()
    }
}

pub async fn run_once(pool: &PgPool, adapter: &FmpOpinionAdapter) -> Result<usize> {
    let store = Store { pool: pool.clone() };
    let max_symbols = max_symbols_from_env("FMP_OPINION_MAX_SYMBOLS_PER_PASS", 75);
    let symbols = store
        .priority_scan_symbols(max_symbols)
        .await
        .unwrap_or_default();
    source_health::mark_started(pool, "fmp_analyst_opinion", symbols.len() as i32).await?;
    store
        .mark_source_tasks_fetching(&OPINION_ACTIONS, &symbols, "ingest.fmp_analyst_opinion")
        .await?;
    let mut rows_seen = 0usize;
    let mut rows_inserted = 0usize;
    let mut symbols_failed = 0i32;
    let mut saw_rate_limit = false;
    let mut coverage = OpinionActionCoverage::default();
    let mut failed_symbols = BTreeSet::new();
    let mut rate_limited_symbols = BTreeSet::new();

    for symbol in &symbols {
        match scan_one(pool, adapter, symbol).await {
            Ok(stats) => {
                rows_seen += stats.rows_seen();
                rows_inserted += stats.inserted_rows;
                coverage.record(symbol, stats);
            }
            Err(e) => {
                symbols_failed += 1;
                failed_symbols.insert(symbol.clone());
                if source_health::failure_kind(&e.to_string()) == "rate_limited" {
                    saw_rate_limit = true;
                    rate_limited_symbols.insert(symbol.clone());
                }
                warn!(
                    symbol = %symbol,
                    error = %format!("{:#}", e),
                    "fmp analyst opinion scan_one failed"
                );
            }
        }
    }

    source_health::record_success(
        pool,
        "fmp_analyst_opinion",
        rows_seen as i64,
        rows_inserted as i64,
        symbols.len() as i32,
        symbols_failed,
    )
    .await?;
    let successful_symbols: Vec<String> = symbols
        .iter()
        .filter(|s| !failed_symbols.contains(*s))
        .cloned()
        .collect();
    for action in OPINION_ACTIONS {
        store
            .complete_source_tasks_for_attempt(
                action,
                &successful_symbols,
                &coverage.symbols_for_action(action),
                "ingest.fmp_analyst_opinion",
                chrono::Duration::minutes(30),
            )
            .await?;
    }
    let non_rate_limited_failures: Vec<String> = failed_symbols
        .difference(&rate_limited_symbols)
        .cloned()
        .collect();
    if !non_rate_limited_failures.is_empty() {
        for action in OPINION_ACTIONS {
            store
                .fail_source_tasks_for_attempt(
                    action,
                    &non_rate_limited_failures,
                    "ingest.fmp_analyst_opinion",
                    "failed",
                    "one or more FMP analyst opinion requests failed",
                    None,
                )
                .await?;
        }
    }
    if saw_rate_limit {
        let retry_after_at = rate_limit::fmp().retry_after_at().await;
        source_health::record_failure(
            pool,
            "fmp_analyst_opinion",
            "rate_limited",
            "one or more FMP analyst opinion requests were rate limited",
            retry_after_at,
        )
        .await?;
        let rate_limited_symbols: Vec<String> = rate_limited_symbols.into_iter().collect();
        for action in OPINION_ACTIONS {
            store
                .fail_source_tasks_for_attempt(
                    action,
                    &rate_limited_symbols,
                    "ingest.fmp_analyst_opinion",
                    "rate_limited",
                    "one or more FMP analyst opinion requests were rate limited",
                    retry_after_at,
                )
                .await?;
        }
    }
    Ok(rows_inserted)
}

pub async fn scan_one(
    pool: &PgPool,
    adapter: &FmpOpinionAdapter,
    symbol: &str,
) -> Result<OpinionScanStats> {
    let raw = adapter.fetch_one(symbol).await?;
    let consensus_rows = decode_consensus(&raw.consensus)?;
    let recommendation_rows = decode_recommendations(&raw.recommendations)?;
    let event_rows = decode_price_target_events(&raw.price_target_events)?;

    let mut inserted = 0usize;
    for (i, row) in consensus_rows.iter().enumerate() {
        let raw_row = raw
            .consensus
            .get(i)
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        inserted += usize::from(insert_price_target_snapshot(pool, row, &raw_row).await?);
    }

    let recommendations = normalize_recommendations(&recommendation_rows);
    for (i, row) in recommendations.iter().enumerate() {
        let raw_row = raw
            .recommendations
            .get(i)
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        inserted += usize::from(insert_recommendation_snapshot(pool, row, &raw_row).await?);
    }

    let events = normalize_price_target_events(&event_rows);
    for (i, row) in events.iter().enumerate() {
        let raw_row = raw
            .price_target_events
            .get(i)
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        inserted += usize::from(insert_price_target_event(pool, row, &raw_row).await?);
    }

    Ok(OpinionScanStats {
        consensus_rows: consensus_rows.len(),
        recommendation_rows: recommendation_rows.len(),
        event_rows: event_rows.len(),
        inserted_rows: inserted,
    })
}

async fn insert_price_target_snapshot(
    pool: &PgPool,
    row: &PriceTargetConsensusRow,
    raw: &serde_json::Value,
) -> Result<bool> {
    let res = sqlx::query(
        r#"INSERT INTO analyst_price_target_snapshot
             (symbol, target_high, target_low, target_consensus, target_median, raw)
           VALUES ($1, $2, $3, $4, $5, $6::jsonb)
           ON CONFLICT DO NOTHING"#,
    )
    .bind(&row.symbol)
    .bind(row.target_high)
    .bind(row.target_low)
    .bind(row.target_consensus)
    .bind(row.target_median)
    .bind(raw)
    .execute(pool)
    .await
    .context("insert analyst_price_target_snapshot")?;
    Ok(res.rows_affected() > 0)
}

async fn insert_recommendation_snapshot(
    pool: &PgPool,
    row: &super::fmp_opinion::NormalizedRecommendation,
    raw: &serde_json::Value,
) -> Result<bool> {
    let res = sqlx::query(
        r#"INSERT INTO analyst_recommendation_snapshot
             (symbol, as_of_date, strong_buy, buy, hold, sell, strong_sell, raw)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8::jsonb)
           ON CONFLICT DO NOTHING"#,
    )
    .bind(&row.symbol)
    .bind(row.as_of_date)
    .bind(row.strong_buy)
    .bind(row.buy)
    .bind(row.hold)
    .bind(row.sell)
    .bind(row.strong_sell)
    .bind(raw)
    .execute(pool)
    .await
    .context("insert analyst_recommendation_snapshot")?;
    Ok(res.rows_affected() > 0)
}

async fn insert_price_target_event(
    pool: &PgPool,
    row: &super::fmp_opinion::NormalizedPriceTargetEvent,
    raw: &serde_json::Value,
) -> Result<bool> {
    let inserted = sqlx::query(
        r#"INSERT INTO analyst_price_target_event
             (symbol, published_at, news_url, news_title, analyst_name,
              analyst_company, price_target, adj_price_target, price_when_posted,
              news_publisher, news_base_url, raw)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12::jsonb)
           ON CONFLICT DO NOTHING
           RETURNING id"#,
    )
    .bind(&row.symbol)
    .bind(row.published_at)
    .bind(&row.news_url)
    .bind(&row.news_title)
    .bind(&row.analyst_name)
    .bind(&row.analyst_company)
    .bind(row.price_target)
    .bind(row.adj_price_target)
    .bind(row.price_when_posted)
    .bind(&row.news_publisher)
    .bind(&row.news_base_url)
    .bind(raw)
    .fetch_optional(pool)
    .await
    .context("insert analyst_price_target_event")?;
    if let Some(inserted) = inserted {
        let event_id: i64 = inserted.try_get("id")?;
        upsert_price_target_event_evidence_item(pool, event_id, row).await?;
        return Ok(true);
    }
    Ok(false)
}

async fn upsert_price_target_event_evidence_item(
    pool: &PgPool,
    event_id: i64,
    row: &super::fmp_opinion::NormalizedPriceTargetEvent,
) -> Result<()> {
    let polarity = match (row.adj_price_target, row.price_when_posted) {
        (Some(target), Some(price)) if price > 0.0 && target > price * 1.05 => Some(0.5),
        (Some(target), Some(price)) if price > 0.0 && target < price * 0.95 => Some(-0.5),
        (Some(_), Some(price)) if price > 0.0 => Some(0.0),
        _ => None,
    };
    sqlx::query(
        r#"INSERT INTO evidence_item
             (symbol, kind, observed_at, source, source_id, source_ref,
              summary, strength, polarity, url)
           VALUES (
             $1, 'rating_change', $2, 'fmp_opinion', $3,
             jsonb_build_object(
                'table', 'analyst_price_target_event',
                'id', $4::bigint,
                'analyst_name', $5::text,
                'analyst_company', $6::text,
                'price_target', $7::double precision,
                'adj_price_target', $8::double precision,
                'price_when_posted', $9::double precision
             ),
             left($10, 500), 0.6, $11, $12
           )
           ON CONFLICT (source, source_id) DO NOTHING"#,
    )
    .bind(&row.symbol)
    .bind(row.published_at)
    .bind(format!("analyst_price_target_event:{event_id}"))
    .bind(event_id)
    .bind(&row.analyst_name)
    .bind(&row.analyst_company)
    .bind(row.price_target)
    .bind(row.adj_price_target)
    .bind(row.price_when_posted)
    .bind(&row.news_title)
    .bind(polarity)
    .bind(&row.news_url)
    .execute(pool)
    .await
    .context("insert analyst_price_target_event evidence_item")?;
    Ok(())
}

pub async fn run(pool: PgPool, adapter: FmpOpinionAdapter, interval: Duration) -> Result<()> {
    info!(
        interval_secs = interval.as_secs(),
        "fmp analyst opinion service started"
    );
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        match run_once(&pool, &adapter).await {
            Ok(n) if n > 0 => info!(inserted = n, "fmp analyst opinion pass complete"),
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
                    "fmp_analyst_opinion",
                    source_health::failure_kind(&message),
                    &message,
                    retry_after_at,
                )
                .await
                {
                    warn!(error = %record_err, "fmp analyst opinion source health failed");
                }
                warn!(error = %e, "fmp analyst opinion pass failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opinion_scan_stats_sums_all_endpoint_rows() {
        let stats = OpinionScanStats {
            consensus_rows: 1,
            recommendation_rows: 2,
            event_rows: 3,
            inserted_rows: 4,
        };

        assert_eq!(stats.rows_seen(), 6);
    }

    #[test]
    fn opinion_action_coverage_tracks_each_action_independently() {
        let mut coverage = OpinionActionCoverage::default();

        coverage.record(
            "NVDA",
            OpinionScanStats {
                consensus_rows: 1,
                recommendation_rows: 0,
                event_rows: 2,
                inserted_rows: 3,
            },
        );
        coverage.record(
            "AMD",
            OpinionScanStats {
                consensus_rows: 0,
                recommendation_rows: 1,
                event_rows: 0,
                inserted_rows: 1,
            },
        );

        assert_eq!(
            coverage.symbols_for_action("fmp_price_target_consensus"),
            vec!["NVDA".to_string()]
        );
        assert_eq!(
            coverage.symbols_for_action("fmp_grades_historical"),
            vec!["AMD".to_string()]
        );
        assert_eq!(
            coverage.symbols_for_action("fmp_price_target_news"),
            vec!["NVDA".to_string()]
        );
    }
}
