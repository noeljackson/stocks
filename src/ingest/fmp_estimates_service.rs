//! Service loop: fetch FMP estimates → snapshot → diff against prior → emit
//! `estimate_revision` rows (#18). Runs as a tokio task spawned from
//! `src/bin/ingest.rs`.
//!
//! The per-pass flow for each universe ticker is:
//!   1. Fetch /stable/analyst-estimates (returns snapshot per fiscal period)
//!   2. For each period in the response, INSERT into `estimate_snapshot`
//!   3. Look up the MOST RECENT prior snapshot for (symbol, fiscal_period_end)
//!   4. `diff_snapshots(prev, curr)` — if Some, INSERT into `estimate_revision`
//!
//! Idempotent: the snapshot table is naturally append-only (UNIQUE constraint
//! on (symbol, fiscal_period_end, period_kind, snapshot_at) catches double-
//! polls within the same second), and `diff_snapshots` returns None when
//! nothing changed.

use std::time::Duration;

use anyhow::{Context, Result};
use sqlx::Row;
use sqlx::postgres::PgPool;
use tracing::{info, warn};

use super::fmp_estimates::{
    FmpEstimatesAdapter, NormalizedEstimate, decode_response, diff_snapshots, normalize,
};
use crate::platform::store::Store;

/// One pass over scan_pool ∪ universe (#104). Returns revision events emitted.
pub async fn run_once(pool: &PgPool, adapter: &FmpEstimatesAdapter) -> Result<usize> {
    let store = Store { pool: pool.clone() };
    let symbols = store.scan_pool_symbols().await.unwrap_or_default();
    let mut total_revisions = 0;
    for symbol in &symbols {
        match scan_one(pool, adapter, symbol).await {
            Ok(n) => total_revisions += n,
            Err(e) => warn!(symbol = %symbol, error = %e, "fmp_estimates scan_one failed"),
        }
        // Pace under FMP Starter rate limits — 300/min = 5/sec ceiling.
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    Ok(total_revisions)
}

async fn scan_one(pool: &PgPool, adapter: &FmpEstimatesAdapter, symbol: &str) -> Result<usize> {
    let raw = adapter.fetch_one(symbol).await?;
    let rows = decode_response(&raw)?;
    let normalized = normalize(&rows);
    let mut emitted = 0;
    for (i, curr) in normalized.iter().enumerate() {
        let raw_row = raw.get(i).cloned().unwrap_or(serde_json::Value::Null);
        let prev = load_latest_snapshot(pool, symbol, curr.fiscal_period_end).await?;
        let curr_id = insert_snapshot(pool, curr, &raw_row).await?;
        if let Some(delta) = diff_snapshots(prev.as_ref(), curr) {
            let prev_id_for_log: Option<i64> = prev.as_ref().and_then(|_| None);
            // Look up prev snapshot's id (separate query — load_latest_snapshot
            // returned the parsed shape, not the row id, to keep its signature
            // pure-data and let it be tested in isolation if we want).
            let prev_id =
                lookup_prev_snapshot_id(pool, symbol, curr.fiscal_period_end, curr_id).await?;
            let _ = prev_id_for_log; // (keeps explicit logic visible)
            insert_revision(pool, symbol, curr, prev_id, curr_id, &delta).await?;
            emitted += 1;
            info!(
                symbol = %symbol,
                period = %curr.fiscal_period_end,
                direction = delta.direction,
                eps_delta = ?delta.eps_delta,
                rev_delta_pct = ?delta.revenue_delta_pct,
                "estimate revision emitted"
            );
        }
    }
    Ok(emitted)
}

async fn load_latest_snapshot(
    pool: &PgPool,
    symbol: &str,
    period_end: chrono::NaiveDate,
) -> Result<Option<NormalizedEstimate>> {
    let row = sqlx::query(
        r#"SELECT eps_avg, eps_low, eps_high,
                  revenue_avg, revenue_low, revenue_high,
                  num_analysts_eps, num_analysts_revenue
             FROM estimate_snapshot
            WHERE symbol = $1 AND fiscal_period_end = $2 AND period_kind = 'annual'
         ORDER BY snapshot_at DESC LIMIT 1"#,
    )
    .bind(symbol)
    .bind(period_end)
    .fetch_optional(pool)
    .await
    .context("load_latest_snapshot")?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(NormalizedEstimate {
        symbol: symbol.to_string(),
        fiscal_period_end: period_end,
        eps_avg: row.try_get::<Option<f64>, _>("eps_avg")?,
        eps_low: row.try_get::<Option<f64>, _>("eps_low")?,
        eps_high: row.try_get::<Option<f64>, _>("eps_high")?,
        revenue_avg: row.try_get::<Option<f64>, _>("revenue_avg")?,
        revenue_low: row.try_get::<Option<f64>, _>("revenue_low")?,
        revenue_high: row.try_get::<Option<f64>, _>("revenue_high")?,
        num_analysts_eps: row.try_get::<Option<i32>, _>("num_analysts_eps")?,
        num_analysts_revenue: row.try_get::<Option<i32>, _>("num_analysts_revenue")?,
    }))
}

async fn lookup_prev_snapshot_id(
    pool: &PgPool,
    symbol: &str,
    period_end: chrono::NaiveDate,
    curr_id: i64,
) -> Result<Option<i64>> {
    let row = sqlx::query(
        r#"SELECT id FROM estimate_snapshot
            WHERE symbol = $1 AND fiscal_period_end = $2 AND period_kind = 'annual'
              AND id <> $3
         ORDER BY snapshot_at DESC LIMIT 1"#,
    )
    .bind(symbol)
    .bind(period_end)
    .bind(curr_id)
    .fetch_optional(pool)
    .await
    .context("lookup_prev_snapshot_id")?;
    Ok(row.map(|r| r.try_get::<i64, _>("id").unwrap_or(0)))
}

async fn insert_snapshot(
    pool: &PgPool,
    e: &NormalizedEstimate,
    raw: &serde_json::Value,
) -> Result<i64> {
    let row = sqlx::query(
        r#"INSERT INTO estimate_snapshot
             (symbol, fiscal_period_end, period_kind,
              eps_avg, eps_low, eps_high,
              revenue_avg, revenue_low, revenue_high,
              num_analysts_eps, num_analysts_revenue, raw)
           VALUES ($1, $2, 'annual', $3, $4, $5, $6, $7, $8, $9, $10, $11::jsonb)
           RETURNING id"#,
    )
    .bind(&e.symbol)
    .bind(e.fiscal_period_end)
    .bind(e.eps_avg)
    .bind(e.eps_low)
    .bind(e.eps_high)
    .bind(e.revenue_avg)
    .bind(e.revenue_low)
    .bind(e.revenue_high)
    .bind(e.num_analysts_eps)
    .bind(e.num_analysts_revenue)
    .bind(raw)
    .fetch_one(pool)
    .await
    .context("insert_snapshot")?;
    Ok(row.try_get::<i64, _>("id")?)
}

async fn insert_revision(
    pool: &PgPool,
    symbol: &str,
    curr: &NormalizedEstimate,
    prev_snapshot_id: Option<i64>,
    curr_snapshot_id: i64,
    delta: &super::fmp_estimates::RevisionDelta,
) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO estimate_revision
             (symbol, fiscal_period_end, period_kind,
              prev_snapshot_id, curr_snapshot_id,
              eps_delta, eps_delta_pct, revenue_delta, revenue_delta_pct,
              direction)
           VALUES ($1, $2, 'annual', $3, $4, $5, $6, $7, $8, $9)"#,
    )
    .bind(symbol)
    .bind(curr.fiscal_period_end)
    .bind(prev_snapshot_id)
    .bind(curr_snapshot_id)
    .bind(delta.eps_delta)
    .bind(delta.eps_delta_pct)
    .bind(delta.revenue_delta)
    .bind(delta.revenue_delta_pct)
    .bind(delta.direction)
    .execute(pool)
    .await
    .context("insert_revision")?;
    Ok(())
}

/// Long-running service loop.
pub async fn run(pool: PgPool, adapter: FmpEstimatesAdapter, interval: Duration) -> Result<()> {
    info!(interval_secs = interval.as_secs(), "fmp_estimates service started");
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        match run_once(&pool, &adapter).await {
            Ok(n) if n > 0 => info!(revisions = n, "fmp_estimates pass complete"),
            Ok(_) => {}
            Err(e) => warn!(error = %e, "fmp_estimates pass failed"),
        }
    }
}
