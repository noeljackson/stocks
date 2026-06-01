//! Service loop for FMP analyst opinion (#116).

use std::time::Duration;

use anyhow::{Context, Result};
use sqlx::postgres::PgPool;
use tracing::{info, warn};

use super::fmp_opinion::{
    FmpOpinionAdapter, PriceTargetConsensusRow, decode_consensus, decode_price_target_events,
    decode_recommendations, normalize_price_target_events, normalize_recommendations,
};
use super::{max_symbols_from_env, rate_limit, source_health};
use crate::platform::store::Store;

pub async fn run_once(pool: &PgPool, adapter: &FmpOpinionAdapter) -> Result<usize> {
    let store = Store { pool: pool.clone() };
    let max_symbols = max_symbols_from_env("FMP_OPINION_MAX_SYMBOLS_PER_PASS", 75);
    let symbols = store
        .priority_scan_symbols(max_symbols)
        .await
        .unwrap_or_default();
    source_health::mark_started(pool, "fmp_analyst_opinion", symbols.len() as i32).await?;
    let mut rows_seen = 0usize;
    let mut rows_inserted = 0usize;
    let mut symbols_failed = 0i32;
    let mut saw_rate_limit = false;

    for symbol in &symbols {
        match scan_one(pool, adapter, symbol).await {
            Ok((seen, inserted)) => {
                rows_seen += seen;
                rows_inserted += inserted;
            }
            Err(e) => {
                symbols_failed += 1;
                if source_health::failure_kind(&e.to_string()) == "rate_limited" {
                    saw_rate_limit = true;
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
    if saw_rate_limit {
        source_health::record_failure(
            pool,
            "fmp_analyst_opinion",
            "rate_limited",
            "one or more FMP analyst opinion requests were rate limited",
            rate_limit::fmp().retry_after_at().await,
        )
        .await?;
    }
    Ok(rows_inserted)
}

pub async fn scan_one(
    pool: &PgPool,
    adapter: &FmpOpinionAdapter,
    symbol: &str,
) -> Result<(usize, usize)> {
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

    Ok((
        consensus_rows.len() + recommendation_rows.len() + event_rows.len(),
        inserted,
    ))
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
    let res = sqlx::query(
        r#"INSERT INTO analyst_price_target_event
             (symbol, published_at, news_url, news_title, analyst_name,
              analyst_company, price_target, adj_price_target, price_when_posted,
              news_publisher, news_base_url, raw)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12::jsonb)
           ON CONFLICT DO NOTHING"#,
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
    .execute(pool)
    .await
    .context("insert analyst_price_target_event")?;
    Ok(res.rows_affected() > 0)
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
