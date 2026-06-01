use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::PgPool;

pub async fn mark_started(pool: &PgPool, source: &str, symbols_attempted: i32) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO source_health
             (source, last_started_at, last_status, symbols_attempted,
              symbols_failed, rows_seen, rows_inserted, updated_at)
           VALUES ($1, now(), 'running', $2, 0, 0, 0, now())
           ON CONFLICT (source) DO UPDATE SET
               last_started_at = EXCLUDED.last_started_at,
               last_status = 'running',
               symbols_attempted = EXCLUDED.symbols_attempted,
               symbols_failed = 0,
               rows_seen = 0,
               rows_inserted = 0,
               last_failure_kind = NULL,
               last_error = NULL,
               retry_after_at = NULL,
               updated_at = now()"#,
    )
    .bind(source)
    .bind(symbols_attempted)
    .execute(pool)
    .await
    .with_context(|| format!("source_health mark_started {source}"))?;
    Ok(())
}

pub async fn record_success(
    pool: &PgPool,
    source: &str,
    rows_seen: i64,
    rows_inserted: i64,
    symbols_attempted: i32,
    symbols_failed: i32,
) -> Result<()> {
    let status = if rows_inserted == 0 {
        "no_new_rows"
    } else {
        "ok"
    };
    sqlx::query(
        r#"INSERT INTO source_health
             (source, last_success_at, last_status, last_failure_kind,
              last_error, retry_after_at, rows_seen, rows_inserted,
              symbols_attempted, symbols_failed, updated_at)
           VALUES ($1, now(), $2, NULL, NULL, NULL, $3, $4, $5, $6, now())
           ON CONFLICT (source) DO UPDATE SET
               last_success_at = EXCLUDED.last_success_at,
               last_status = EXCLUDED.last_status,
               last_failure_kind = NULL,
               last_error = NULL,
               retry_after_at = NULL,
               rows_seen = EXCLUDED.rows_seen,
               rows_inserted = EXCLUDED.rows_inserted,
               symbols_attempted = EXCLUDED.symbols_attempted,
               symbols_failed = EXCLUDED.symbols_failed,
               updated_at = now()"#,
    )
    .bind(source)
    .bind(status)
    .bind(rows_seen)
    .bind(rows_inserted)
    .bind(symbols_attempted)
    .bind(symbols_failed)
    .execute(pool)
    .await
    .with_context(|| format!("source_health record_success {source}"))?;
    Ok(())
}

pub async fn record_failure(
    pool: &PgPool,
    source: &str,
    failure_kind: &str,
    error: &str,
    retry_after_at: Option<DateTime<Utc>>,
) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO source_health
             (source, last_failure_at, last_status, last_failure_kind,
              last_error, retry_after_at, updated_at)
           VALUES ($1, now(), 'failed', $2, $3, $4, now())
           ON CONFLICT (source) DO UPDATE SET
               last_failure_at = EXCLUDED.last_failure_at,
               last_status = EXCLUDED.last_status,
               last_failure_kind = EXCLUDED.last_failure_kind,
               last_error = EXCLUDED.last_error,
               retry_after_at = EXCLUDED.retry_after_at,
               updated_at = now()"#,
    )
    .bind(source)
    .bind(failure_kind)
    .bind(error.chars().take(500).collect::<String>())
    .bind(retry_after_at)
    .execute(pool)
    .await
    .with_context(|| format!("source_health record_failure {source}"))?;
    Ok(())
}

#[must_use]
pub fn failure_kind(error: &str) -> &'static str {
    let lower = error.to_ascii_lowercase();
    if lower.contains("429") || lower.contains("rate limit") {
        "rate_limited"
    } else {
        "error"
    }
}
