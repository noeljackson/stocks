//! Crowd-sentiment ingest service (#20).
//!
//! Pulls CBOE put/call + VIX once per pass and upserts to `crowd_sentiment`.
//! Idempotent — UNIQUE on (source, metric, observed_at) means repeat polls
//! just no-op insert.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use sqlx::postgres::PgPool;
use tracing::{info, warn};

use super::cboe::{CboeAdapter, CrowdRow};
use super::source_health;
use crate::platform::store::Store;

const CBOE_TASK_ACTION: &str = "cboe_crowd_sentiment";
const MACRO_TARGET: &str = "macro_regime";

/// One pass: fetch all configured sources, upsert. Returns total rows inserted.
pub async fn run_once(pool: &PgPool, adapter: &CboeAdapter) -> Result<usize> {
    source_health::mark_started(pool, "cboe", 0).await?;
    let mut inserted = 0;
    let mut rows_seen = 0;
    let mut failures = 0;
    match adapter.fetch_pcr().await {
        Ok(rows) => {
            rows_seen += rows.len();
            inserted += upsert_many(pool, &rows).await?;
        }
        Err(e) => {
            failures += 1;
            warn!(error = %e, "cboe pcr fetch failed");
        }
    }
    match adapter.fetch_vix().await {
        Ok(rows) => {
            rows_seen += rows.len();
            inserted += upsert_many(pool, &rows).await?;
        }
        Err(e) => {
            failures += 1;
            warn!(error = %e, "cboe vix fetch failed");
        }
    }
    if failures == 2 {
        bail!("all cboe crowd sentiment fetches failed");
    }
    source_health::record_success(pool, "cboe", rows_seen as i64, inserted as i64, 2, failures)
        .await?;
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

async fn mark_macro_task_fetching(store: &Store) {
    if let Err(e) = store
        .mark_source_tasks_fetching_for_scope(
            "benchmark",
            &[CBOE_TASK_ACTION],
            &[MACRO_TARGET.to_string()],
            "ingest.cboe",
        )
        .await
    {
        warn!(error = %e, "cboe source task claim failed");
    }
}

async fn complete_macro_task(store: &Store, rows_seen: bool) {
    let targets_with_rows = if rows_seen {
        vec![MACRO_TARGET.to_string()]
    } else {
        Vec::new()
    };
    if let Err(e) = store
        .complete_source_tasks_for_scope(
            "benchmark",
            CBOE_TASK_ACTION,
            &[MACRO_TARGET.to_string()],
            &targets_with_rows,
            "ingest.cboe",
            chrono::Duration::minutes(30),
        )
        .await
    {
        warn!(error = %e, "cboe source task completion failed");
    }
}

async fn fail_macro_task(store: &Store, error: &str) {
    if let Err(e) = store
        .fail_source_tasks_for_scope(
            "benchmark",
            CBOE_TASK_ACTION,
            &[MACRO_TARGET.to_string()],
            "ingest.cboe",
            source_health::failure_kind(error),
            error,
            None,
        )
        .await
    {
        warn!(error = %e, "cboe source task failure record failed");
    }
}

pub async fn run(store: Store, adapter: CboeAdapter, interval: Duration) -> Result<()> {
    let pool = store.pool.clone();
    info!(
        interval_secs = interval.as_secs(),
        "crowd_sentiment service started"
    );
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        mark_macro_task_fetching(&store).await;
        match run_once(&pool, &adapter).await {
            Ok(n) if n > 0 => {
                complete_macro_task(&store, true).await;
                info!(inserted = n, "crowd_sentiment pass complete");
            }
            Ok(_) => {
                complete_macro_task(&store, false).await;
            }
            Err(e) => {
                let message = e.to_string();
                fail_macro_task(&store, &message).await;
                if let Err(record_err) = source_health::record_failure(
                    &pool,
                    "cboe",
                    source_health::failure_kind(&message),
                    &message,
                    None,
                )
                .await
                {
                    warn!(error = %record_err, "cboe source health failure record failed");
                }
                warn!(error = %e, "crowd_sentiment pass failed");
            }
        }
    }
}
