//! Crowd-sentiment ingest service (#20).
//!
//! Pulls CBOE put/call + VIX once per pass and upserts to `crowd_sentiment`.
//! Idempotent — UNIQUE on (source, metric, observed_at) means repeat polls
//! just no-op insert.

use std::time::Duration;

use anyhow::{Context, Result};
use sqlx::postgres::PgPool;
use tracing::{info, warn};

use super::cboe::{CboeAdapter, CrowdRow};

/// One pass: fetch all configured sources, upsert. Returns total rows inserted.
pub async fn run_once(pool: &PgPool, adapter: &CboeAdapter) -> Result<usize> {
    let mut inserted = 0;
    match adapter.fetch_pcr().await {
        Ok(rows) => inserted += upsert_many(pool, &rows).await?,
        Err(e) => warn!(error = %e, "cboe pcr fetch failed"),
    }
    match adapter.fetch_vix().await {
        Ok(rows) => inserted += upsert_many(pool, &rows).await?,
        Err(e) => warn!(error = %e, "cboe vix fetch failed"),
    }
    Ok(inserted)
}

async fn upsert_many(pool: &PgPool, rows: &[CrowdRow]) -> Result<usize> {
    if rows.is_empty() {
        return Ok(0);
    }
    let mut tx = pool.begin().await.context("begin tx")?;
    let mut inserted = 0;
    for r in rows {
        let res = sqlx::query(
            r#"INSERT INTO crowd_sentiment (source, metric, value, observed_at)
               VALUES ($1, $2, $3, $4)
               ON CONFLICT (source, metric, observed_at) DO NOTHING"#,
        )
        .bind(r.source)
        .bind(r.metric)
        .bind(r.value)
        .bind(r.observed_at)
        .execute(&mut *tx)
        .await
        .context("upsert crowd_sentiment")?;
        inserted += res.rows_affected() as usize;
    }
    tx.commit().await.context("commit crowd_sentiment tx")?;
    Ok(inserted)
}

pub async fn run(pool: PgPool, adapter: CboeAdapter, interval: Duration) -> Result<()> {
    info!(interval_secs = interval.as_secs(), "crowd_sentiment service started");
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        match run_once(&pool, &adapter).await {
            Ok(n) if n > 0 => info!(inserted = n, "crowd_sentiment pass complete"),
            Ok(_) => {}
            Err(e) => warn!(error = %e, "crowd_sentiment pass failed"),
        }
    }
}
