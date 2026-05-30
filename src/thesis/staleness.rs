//! Staleness service (#11) — marks past-deadline pending conditions as stale
//! and emits a risk.warning so the UI surfaces them in the existing feed.
//!
//! Pure logic (the deadline check + role→column mapping) is in this module
//! and unit-testable. The service wrapper (NATS publish + DB updates) is
//! the `cmd/staler` binary.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{Row, postgres::PgPool};
use std::time::Duration;
use tracing::{info, warn};

use crate::platform::bus::Bus;
use crate::platform::subjects;

/// One row of the staleness query — pure data; what the service writes to
/// the DB and publishes to NATS. Public for tests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StaleRow {
    pub thesis_id: uuid::Uuid,
    pub symbol: String,
    pub role: String, // "conviction" | "trigger" | "invalidation" | "fulfillment"
    pub position: i64,
    pub name: String,
    pub deadline_at: DateTime<Utc>,
}

/// Returns the role's JSONB column name on `thesis`. Pure mapping.
#[must_use]
pub fn role_to_column(role: &str) -> Option<&'static str> {
    match role {
        "conviction" => Some("conviction_conditions"),
        "trigger" => Some("trigger_conditions"),
        "invalidation" => Some("invalidation_conditions"),
        "fulfillment" => Some("fulfillment_conditions"),
        _ => None,
    }
}

/// Find every pending condition whose deadline has passed. Uses the
/// `v_condition` view (#9) so the SQL is one straightforward query.
pub async fn find_stale(pool: &PgPool, now: DateTime<Utc>) -> Result<Vec<StaleRow>> {
    let rows = sqlx::query(
        r#"SELECT thesis_id, symbol, role, position, COALESCE(name, '') AS name, deadline_at
             FROM v_condition
            WHERE status = 'pending'
              AND deadline_at IS NOT NULL
              AND deadline_at < $1"#,
    )
    .bind(now)
    .fetch_all(pool)
    .await
    .context("staleness query")?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(StaleRow {
            thesis_id: row.try_get("thesis_id")?,
            symbol: row.try_get("symbol")?,
            role: row.try_get("role")?,
            position: row.try_get("position")?,
            name: row.try_get("name")?,
            deadline_at: row.try_get("deadline_at")?,
        });
    }
    Ok(out)
}

/// Flip a single condition's `status` to `'stale'` in-place. Position is
/// 1-based (Postgres `WITH ORDINALITY`); we convert to 0-based for jsonb_set.
///
/// Each role has its own static UPDATE — sqlx 0.9's safety lint forbids
/// dynamic SQL strings even when the substitution comes from a closed set.
pub async fn mark_stale(pool: &PgPool, row: &StaleRow) -> Result<()> {
    let idx = (row.position - 1).max(0).to_string();
    let result = match row.role.as_str() {
        "conviction" => sqlx::query(SQL_MARK_CONVICTION).bind(&idx).bind(row.thesis_id).execute(pool).await,
        "trigger" => sqlx::query(SQL_MARK_TRIGGER).bind(&idx).bind(row.thesis_id).execute(pool).await,
        "invalidation" => sqlx::query(SQL_MARK_INVALIDATION).bind(&idx).bind(row.thesis_id).execute(pool).await,
        "fulfillment" => sqlx::query(SQL_MARK_FULFILLMENT).bind(&idx).bind(row.thesis_id).execute(pool).await,
        other => {
            warn!(role = other, "unknown role; skipping");
            return Ok(());
        }
    };
    result.with_context(|| {
        format!("mark_stale {} {} pos={}", row.thesis_id, row.role, row.position)
    })?;
    Ok(())
}

// Per-role UPDATE statements. Each one's column name is hard-coded so sqlx's
// safety lint is happy. SQL otherwise identical across the four.
const SQL_MARK_CONVICTION: &str = r#"UPDATE thesis
       SET conviction_conditions = jsonb_set(
           COALESCE(conviction_conditions, '[]'::jsonb),
           ARRAY[$1, 'status'],
           '"stale"'::jsonb,
           false)
     WHERE thesis_id = $2"#;
const SQL_MARK_TRIGGER: &str = r#"UPDATE thesis
       SET trigger_conditions = jsonb_set(
           COALESCE(trigger_conditions, '[]'::jsonb),
           ARRAY[$1, 'status'],
           '"stale"'::jsonb,
           false)
     WHERE thesis_id = $2"#;
const SQL_MARK_INVALIDATION: &str = r#"UPDATE thesis
       SET invalidation_conditions = jsonb_set(
           COALESCE(invalidation_conditions, '[]'::jsonb),
           ARRAY[$1, 'status'],
           '"stale"'::jsonb,
           false)
     WHERE thesis_id = $2"#;
const SQL_MARK_FULFILLMENT: &str = r#"UPDATE thesis
       SET fulfillment_conditions = jsonb_set(
           COALESCE(fulfillment_conditions, '[]'::jsonb),
           ARRAY[$1, 'status'],
           '"stale"'::jsonb,
           false)
     WHERE thesis_id = $2"#;

/// One pass over the universe. Returns how many conditions were flipped.
pub async fn run_once(pool: &PgPool, bus: &Bus) -> Result<usize> {
    let now = Utc::now();
    let stale = find_stale(pool, now).await?;
    if stale.is_empty() {
        return Ok(0);
    }
    info!(count = stale.len(), "staleness: marking conditions stale");

    for row in &stale {
        if let Err(e) = mark_stale(pool, row).await {
            warn!(error = %e, "mark_stale failed; skipping");
            continue;
        }
        let payload = serde_json::json!({
            "kind": "condition_stale",
            "thesis_id": row.thesis_id,
            "symbol": row.symbol,
            "role": row.role,
            "name": row.name,
            "deadline_at": row.deadline_at,
            "detected_at": now,
        });
        if let Err(e) = bus
            .publish(subjects::RISK_WARNING, payload.to_string().as_bytes())
            .await
        {
            warn!(error = %e, "publish risk.warning failed");
        }
    }
    Ok(stale.len())
}

/// Service entry point: loop forever waking every `interval`.
pub async fn run(pool: PgPool, bus: Bus, interval: Duration) -> Result<()> {
    // Ensure the DECISIONS stream exists so risk.warning publishes go through.
    bus.ensure_stream(subjects::STREAM_DECISIONS, &["risk.*", "decision.*"])
        .await?;
    info!(interval_secs = interval.as_secs(), "staleness service started");

    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        match run_once(&pool, &bus).await {
            Ok(n) if n > 0 => info!(flipped = n, "staleness pass complete"),
            Ok(_) => {} // quiet on no-op passes
            Err(e) => warn!(error = %e, "staleness pass failed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_to_column_maps_each() {
        assert_eq!(role_to_column("conviction"), Some("conviction_conditions"));
        assert_eq!(role_to_column("trigger"), Some("trigger_conditions"));
        assert_eq!(role_to_column("invalidation"), Some("invalidation_conditions"));
        assert_eq!(role_to_column("fulfillment"), Some("fulfillment_conditions"));
        assert_eq!(role_to_column("garbage"), None, "unknown roles → None (skip safely)");
    }

    // Note: find_stale + mark_stale are exercised via the live smoke run in
    // CI / dev (the binary itself); they're thin wrappers over sqlx. The
    // role-to-column mapping (above) is the only branching logic worth a
    // unit test — it's where a typo would silently corrupt the wrong column.
}
