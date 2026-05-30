//! Postgres access layer (sqlx pool + typed helpers).
//!
//! sqlx::query (not query!) — we keep the macro discipline off for v0 because
//! compile-time SQL checking requires a live DB at build time (DATABASE_URL
//! must be reachable). We can flip to the macro form later by setting
//! SQLX_OFFLINE=true + checking in the sqlx-data.json.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::{
    Row,
    postgres::{PgPool, PgPoolOptions},
};
use std::time::Duration;

use crate::platform::domain::{Alert, AlertKind};

#[derive(Clone)]
pub struct Store {
    pub pool: PgPool,
}

impl Store {
    pub async fn connect(url: &str) -> Result<Self> {
        // Strip the sslmode=disable querystring noise that pgx accepts but
        // sqlx doesn't always: prefer ssl-mode in connection options.
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .acquire_timeout(Duration::from_secs(5))
            .connect(url)
            .await
            .with_context(|| format!("db connect {url}"))?;
        Ok(Self { pool })
    }

    /// Stores a raw event append-only (SPEC §4 PIT corpus). Returns `true`
    /// if newly inserted; `false` if `content_hash` already existed (dedup).
    pub async fn append_ingest_event(
        &self,
        source: &str,
        kind: &str,
        symbol: Option<&str>,
        payload: &[u8],
        content_hash: &str,
        source_ts: Option<DateTime<Utc>>,
    ) -> Result<bool> {
        let payload_str = std::str::from_utf8(payload).context("payload utf-8")?;
        let res = sqlx::query(
            r#"INSERT INTO ingest_event (source, kind, symbol, payload, content_hash, source_ts)
               VALUES ($1, $2, $3, $4::jsonb, $5, $6)
               ON CONFLICT (content_hash) DO NOTHING"#,
        )
        .bind(source)
        .bind(kind)
        .bind(symbol)
        .bind(payload_str)
        .bind(content_hash)
        .bind(source_ts)
        .execute(&self.pool)
        .await
        .context("append_ingest_event")?;
        Ok(res.rows_affected() > 0)
    }

    /// Returns the active config body (raw JSON) + version for `name`.
    pub async fn active_config(&self, name: &str) -> Result<(serde_json::Value, i32)> {
        let row =
            sqlx::query("SELECT body, version FROM config WHERE name = $1 AND active LIMIT 1")
                .bind(name)
                .fetch_one(&self.pool)
                .await
                .with_context(|| format!("active_config {name}"))?;
        let body: serde_json::Value = row.try_get("body")?;
        let version: i32 = row.try_get("version")?;
        Ok((body, version))
    }

    /// Inserts an alert and returns its id.
    pub async fn insert_alert(
        &self,
        kind: AlertKind,
        symbol: Option<&str>,
        payload: &[u8],
    ) -> Result<i64> {
        let payload_str = std::str::from_utf8(payload).context("payload utf-8")?;
        let row = sqlx::query(
            "INSERT INTO alert (kind, symbol, payload) VALUES ($1, $2, $3::jsonb) RETURNING id",
        )
        .bind(kind.as_str())
        .bind(symbol)
        .bind(payload_str)
        .fetch_one(&self.pool)
        .await
        .context("insert_alert")?;
        let id: i64 = row.try_get("id")?;
        Ok(id)
    }

    /// Returns the most recent alerts for the UI feed.
    pub async fn recent_alerts(&self, limit: i64) -> Result<Vec<Alert>> {
        let rows = sqlx::query(
            r#"SELECT id, thesis_id, symbol, kind, payload, acknowledged, created_at
                 FROM alert ORDER BY created_at DESC LIMIT $1"#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .context("recent_alerts")?;

        rows.into_iter()
            .map(|row| {
                let kind_s: String = row.try_get("kind")?;
                let kind: AlertKind = serde_json::from_value(serde_json::Value::String(kind_s))
                    .map_err(|e| anyhow::anyhow!("decode AlertKind: {e}"))?;
                Ok(Alert {
                    id: row.try_get("id")?,
                    thesis_id: row.try_get("thesis_id")?,
                    symbol: row
                        .try_get::<Option<String>, _>("symbol")?
                        .unwrap_or_default(),
                    kind,
                    payload: row.try_get("payload")?,
                    acknowledged: row.try_get("acknowledged")?,
                    created_at: row.try_get("created_at")?,
                })
            })
            .collect()
    }

    /// Writes a regime classification row (SPEC §5.4). `as_of` is PK; conflicts
    /// overwrite. `config_version` is stored as text per schema typing.
    pub async fn upsert_market_state(
        &self,
        as_of: DateTime<Utc>,
        regime: &str,
        capitulation: bool,
        indicators: &serde_json::Value,
        config_version: i32,
    ) -> Result<()> {
        sqlx::query(
            r#"INSERT INTO market_state (as_of, regime, capitulation, indicators, config_version)
               VALUES ($1, $2, $3, $4, $5)
               ON CONFLICT (as_of) DO UPDATE SET
                 regime = EXCLUDED.regime,
                 capitulation = EXCLUDED.capitulation,
                 indicators = EXCLUDED.indicators,
                 config_version = EXCLUDED.config_version"#,
        )
        .bind(as_of)
        .bind(regime)
        .bind(capitulation)
        .bind(indicators)
        .bind(config_version.to_string())
        .execute(&self.pool)
        .await
        .context("upsert_market_state")?;
        Ok(())
    }
}
