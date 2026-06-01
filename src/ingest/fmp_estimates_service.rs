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
use super::{max_symbols_from_env, rate_limit, source_health};
use crate::platform::store::Store;

/// One pass over the tiered deep-research universe. Returns revision events emitted.
pub async fn run_once(pool: &PgPool, adapter: &FmpEstimatesAdapter) -> Result<usize> {
    let store = Store { pool: pool.clone() };
    let max_symbols = max_symbols_from_env("FMP_ESTIMATES_MAX_SYMBOLS_PER_PASS", 100);
    let symbols = store
        .priority_scan_symbols(max_symbols)
        .await
        .unwrap_or_default();
    source_health::mark_started(pool, "fmp_estimates", symbols.len() as i32).await?;
    store
        .mark_source_tasks_fetching(&["fmp_analyst_estimates"], &symbols, "ingest.fmp_estimates")
        .await?;
    let mut total_revisions = 0;
    let mut rows_seen = 0;
    let mut symbols_failed = 0;
    let mut saw_rate_limit = false;
    let mut symbols_with_rows = std::collections::BTreeSet::new();
    let mut failed_symbols = Vec::new();
    let mut rate_limited_symbols = Vec::new();
    for symbol in &symbols {
        match scan_one(pool, adapter, symbol).await {
            Ok((seen, revisions)) => {
                rows_seen += seen;
                total_revisions += revisions;
                if seen > 0 {
                    symbols_with_rows.insert(symbol.clone());
                }
            }
            Err(e) => {
                symbols_failed += 1;
                failed_symbols.push(symbol.clone());
                if source_health::failure_kind(&e.to_string()) == "rate_limited" {
                    saw_rate_limit = true;
                    rate_limited_symbols.push(symbol.clone());
                }
                warn!(symbol = %symbol, error = %e, "fmp_estimates scan_one failed");
            }
        }
    }
    source_health::record_success(
        pool,
        "fmp_estimates",
        rows_seen as i64,
        rows_seen as i64,
        symbols.len() as i32,
        symbols_failed,
    )
    .await?;
    let successful_symbols: Vec<String> = symbols
        .iter()
        .filter(|s| !failed_symbols.contains(s))
        .cloned()
        .collect();
    let symbols_with_rows: Vec<String> = symbols_with_rows.into_iter().collect();
    store
        .complete_source_tasks_for_attempt(
            "fmp_analyst_estimates",
            &successful_symbols,
            &symbols_with_rows,
            "ingest.fmp_estimates",
            chrono::Duration::minutes(30),
        )
        .await?;
    let non_rate_limited_failures: Vec<String> = failed_symbols
        .iter()
        .filter(|s| !rate_limited_symbols.contains(s))
        .cloned()
        .collect();
    if !non_rate_limited_failures.is_empty() {
        store
            .fail_source_tasks_for_attempt(
                "fmp_analyst_estimates",
                &non_rate_limited_failures,
                "ingest.fmp_estimates",
                "failed",
                "one or more FMP estimates requests failed",
                None,
            )
            .await?;
    }
    if saw_rate_limit {
        let retry_after_at = rate_limit::fmp().retry_after_at().await;
        source_health::record_failure(
            pool,
            "fmp_estimates",
            "rate_limited",
            "one or more FMP estimates requests were rate limited",
            retry_after_at,
        )
        .await?;
        store
            .fail_source_tasks_for_attempt(
                "fmp_analyst_estimates",
                &rate_limited_symbols,
                "ingest.fmp_estimates",
                "rate_limited",
                "one or more FMP estimates requests were rate limited",
                retry_after_at,
            )
            .await?;
    }
    Ok(total_revisions)
}

async fn scan_one(
    pool: &PgPool,
    adapter: &FmpEstimatesAdapter,
    symbol: &str,
) -> Result<(usize, usize)> {
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
    Ok((normalized.len(), emitted))
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
    let row = sqlx::query(
        r#"INSERT INTO estimate_revision
             (symbol, fiscal_period_end, period_kind,
              prev_snapshot_id, curr_snapshot_id,
              eps_delta, eps_delta_pct, revenue_delta, revenue_delta_pct,
              direction)
           VALUES ($1, $2, 'annual', $3, $4, $5, $6, $7, $8, $9)
           RETURNING id"#,
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
    .fetch_one(pool)
    .await
    .context("insert_revision")?;
    let revision_id: i64 = row.try_get("id")?;
    if delta.direction != "initial" {
        upsert_estimate_revision_evidence_item(
            pool,
            revision_id,
            symbol,
            curr,
            prev_snapshot_id,
            curr_snapshot_id,
            delta,
        )
        .await?;
    }
    Ok(())
}

async fn upsert_estimate_revision_evidence_item(
    pool: &PgPool,
    revision_id: i64,
    symbol: &str,
    curr: &NormalizedEstimate,
    prev_snapshot_id: Option<i64>,
    curr_snapshot_id: i64,
    delta: &super::fmp_estimates::RevisionDelta,
) -> Result<()> {
    let strength = ([
        delta.eps_delta_pct.map(f64::abs),
        delta.revenue_delta_pct.map(f64::abs),
        if delta.direction == "initial" {
            Some(5.0)
        } else {
            None
        },
    ]
    .into_iter()
    .flatten()
    .fold(0.0_f64, f64::max)
        / 20.0)
        .clamp(0.0, 1.0);
    let polarity = match delta.direction {
        "up" => Some(0.7),
        "down" => Some(-0.7),
        "mixed" => Some(0.0),
        _ => None,
    };
    let summary = estimate_revision_summary(symbol, curr, delta);
    sqlx::query(
        r#"INSERT INTO evidence_item
             (symbol, kind, observed_at, source, source_id, source_ref,
              summary, strength, polarity)
           VALUES (
             $1, 'estimate_revision', now(), 'fmp_estimates', $2,
             jsonb_build_object(
                'table', 'estimate_revision',
                'id', $3::bigint,
                'fiscal_period_end', $4::date,
                'period_kind', 'annual',
                'prev_snapshot_id', $5::bigint,
                'curr_snapshot_id', $6::bigint,
                'direction', $7::text,
                'eps_delta_pct', $8::double precision,
                'revenue_delta_pct', $9::double precision
             ),
             $10, $11, $12
           )
           ON CONFLICT (source, source_id) DO NOTHING"#,
    )
    .bind(symbol)
    .bind(format!("estimate_revision:{revision_id}"))
    .bind(revision_id)
    .bind(curr.fiscal_period_end)
    .bind(prev_snapshot_id)
    .bind(curr_snapshot_id)
    .bind(delta.direction)
    .bind(delta.eps_delta_pct)
    .bind(delta.revenue_delta_pct)
    .bind(summary)
    .bind(strength)
    .bind(polarity)
    .execute(pool)
    .await
    .context("insert estimate_revision evidence_item")?;
    Ok(())
}

fn estimate_revision_summary(
    symbol: &str,
    curr: &NormalizedEstimate,
    delta: &super::fmp_estimates::RevisionDelta,
) -> String {
    let mut parts = vec![
        symbol.to_string(),
        curr.fiscal_period_end.to_string(),
        format!("estimate revision {}", delta.direction),
    ];
    if let Some(v) = delta.eps_delta_pct {
        parts.push(format!("EPS {v:.1}%"));
    }
    if let Some(v) = delta.revenue_delta_pct {
        parts.push(format!("revenue {v:.1}%"));
    }
    parts.join(" ")
}

/// Long-running service loop.
pub async fn run(pool: PgPool, adapter: FmpEstimatesAdapter, interval: Duration) -> Result<()> {
    info!(
        interval_secs = interval.as_secs(),
        "fmp_estimates service started"
    );
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        match run_once(&pool, &adapter).await {
            Ok(n) if n > 0 => info!(revisions = n, "fmp_estimates pass complete"),
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
                    "fmp_estimates",
                    source_health::failure_kind(&message),
                    &message,
                    retry_after_at,
                )
                .await
                {
                    warn!(error = %record_err, "fmp_estimates source health failure record failed");
                }
                warn!(error = %e, "fmp_estimates pass failed");
            }
        }
    }
}
