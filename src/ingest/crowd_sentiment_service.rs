//! Crowd-sentiment ingest service (#20).
//!
//! Pulls CBOE put/call + VIX once per pass and upserts to `crowd_sentiment`.
//! Idempotent — UNIQUE on (source, metric, observed_at) means repeat polls
//! just no-op insert.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::json;
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
        if res.rows_affected() > 0 {
            inserted += 1;
            let source_ref = json!({
                "table": "crowd_sentiment",
                "source": r.source,
                "metric": r.metric,
                "value": r.value,
                "observed_at": r.observed_at,
            });
            sqlx::query(
                r#"INSERT INTO evidence_item
                     (symbol, kind, observed_at, source, source_id, source_ref,
                      summary, strength, polarity)
                   VALUES (
                     'MARKET', 'crowd_sentiment', $1::date::timestamptz, 'cboe', $2,
                     $3::jsonb, $4, $5, $6
                   )
                   ON CONFLICT (source, source_id) DO UPDATE SET
                     source_ref = EXCLUDED.source_ref,
                     summary = EXCLUDED.summary,
                     strength = EXCLUDED.strength,
                     polarity = EXCLUDED.polarity,
                     updated_at = CASE
                       WHEN evidence_item.source_ref IS DISTINCT FROM EXCLUDED.source_ref
                         OR evidence_item.summary IS DISTINCT FROM EXCLUDED.summary
                         OR evidence_item.strength IS DISTINCT FROM EXCLUDED.strength
                         OR evidence_item.polarity IS DISTINCT FROM EXCLUDED.polarity
                       THEN now()
                       ELSE evidence_item.updated_at
                     END"#,
            )
            .bind(r.observed_at)
            .bind(crowd_sentiment_source_id(r))
            .bind(source_ref)
            .bind(crowd_sentiment_summary(r))
            .bind(crowd_sentiment_strength(r))
            .bind(crowd_sentiment_polarity(r))
            .execute(&mut *tx)
            .await
            .context("upsert crowd_sentiment evidence_item")?;
        }
    }
    tx.commit().await.context("commit crowd_sentiment tx")?;
    Ok(inserted)
}

fn crowd_sentiment_source_id(row: &CrowdRow) -> String {
    format!(
        "crowd_sentiment:{}:{}:{}",
        row.source, row.metric, row.observed_at
    )
}

fn crowd_sentiment_summary(row: &CrowdRow) -> String {
    match row.metric {
        "equity_pcr" => format!("CBOE equity put/call ratio {:.2}", row.value),
        "vix_close" => format!("CBOE VIX close {:.2}", row.value),
        metric => format!("CBOE {metric} {:.2}", row.value),
    }
}

fn crowd_sentiment_strength(row: &CrowdRow) -> Option<f64> {
    let strength = match row.metric {
        "equity_pcr" => ((row.value - 0.70).abs() / 0.40).clamp(0.0, 1.0),
        "vix_close" => ((row.value - 18.0).abs() / 20.0).clamp(0.0, 1.0),
        _ => return None,
    };
    Some(strength)
}

fn crowd_sentiment_polarity(row: &CrowdRow) -> Option<f64> {
    match row.metric {
        "equity_pcr" if row.value >= 0.90 => Some(-0.5),
        "equity_pcr" if row.value <= 0.55 => Some(0.3),
        "equity_pcr" => Some(0.0),
        "vix_close" if row.value >= 25.0 => Some(-0.6),
        "vix_close" if row.value <= 14.0 => Some(0.3),
        "vix_close" => Some(0.0),
        _ => None,
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn row(metric: &'static str, value: f64) -> CrowdRow {
        CrowdRow {
            source: if metric == "equity_pcr" {
                "cboe_pcr"
            } else {
                "cboe_vix"
            },
            metric,
            value,
            observed_at: NaiveDate::from_ymd_opt(2026, 6, 1).unwrap(),
        }
    }

    #[test]
    fn crowd_sentiment_evidence_source_id_is_stable() {
        let r = row("equity_pcr", 0.91);
        assert_eq!(
            crowd_sentiment_source_id(&r),
            "crowd_sentiment:cboe_pcr:equity_pcr:2026-06-01"
        );
    }

    #[test]
    fn crowd_sentiment_evidence_scores_risk_off_and_calm_inputs() {
        let hot_pcr = row("equity_pcr", 1.05);
        let calm_vix = row("vix_close", 13.5);

        assert_eq!(
            crowd_sentiment_summary(&hot_pcr),
            "CBOE equity put/call ratio 1.05"
        );
        assert_eq!(crowd_sentiment_polarity(&hot_pcr), Some(-0.5));
        assert!(crowd_sentiment_strength(&hot_pcr).unwrap() > 0.8);
        assert_eq!(crowd_sentiment_summary(&calm_vix), "CBOE VIX close 13.50");
        assert_eq!(crowd_sentiment_polarity(&calm_vix), Some(0.3));
    }
}
