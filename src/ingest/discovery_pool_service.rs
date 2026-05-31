//! discovery_pool refresh service (#88).
//!
//! Once per day:
//!   1. Fetch from FMP screener → ScreenerRow list
//!   2. Upsert each row into discovery_pool (re-stamps last_seen_at)
//!   3. Mark rows that didn't show up this pass as dropped_at = now()
//!      (without deleting — preserves history)

use std::time::Duration;

use anyhow::{Context, Result};
use sqlx::postgres::PgPool;
use tracing::{info, warn};

use super::fmp_screener::FmpScreenerAdapter;

const MIN_MARKET_CAP: i64 = 5_000_000_000; // $5B floor

pub async fn run_once(pool: &PgPool, adapter: &FmpScreenerAdapter) -> Result<(usize, usize, usize)> {
    let rows = adapter.fetch_pool(MIN_MARKET_CAP).await?;
    if rows.is_empty() {
        return Ok((0, 0, 0));
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
    Ok((inserted, refreshed, dropped))
}

pub async fn run(pool: PgPool, adapter: FmpScreenerAdapter, interval: Duration) -> Result<()> {
    info!(interval_secs = interval.as_secs(), "discovery_pool service started");
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        match run_once(&pool, &adapter).await {
            Ok((seen, refreshed, dropped)) if seen > 0 => {
                info!(seen, refreshed, dropped, "discovery_pool pass complete");
            }
            Ok(_) => {}
            Err(e) => warn!(error = %e, "discovery_pool pass failed"),
        }
    }
}
