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

use crate::llm::prompts::{InvocationRecorder, InvocationRow};
use crate::platform::domain::{
    Alert, AlertKind, MarketStateRow, ThesisDetail, ThesisVersionEvent, TickerContextRow, TickerRow,
};

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

    /// Returns the latest market_state row for the UI. None if the table is empty.
    pub async fn latest_market_state(&self) -> Result<Option<MarketStateRow>> {
        let row = sqlx::query(
            r#"SELECT as_of, regime, capitulation, indicators
                 FROM market_state ORDER BY as_of DESC LIMIT 1"#,
        )
        .fetch_optional(&self.pool)
        .await
        .context("latest_market_state")?;
        let Some(row) = row else { return Ok(None) };
        Ok(Some(MarketStateRow {
            as_of: row.try_get("as_of")?,
            regime: row.try_get("regime")?,
            capitulation: row.try_get("capitulation")?,
            indicators: row.try_get("indicators")?,
        }))
    }

    /// Lists active tracked tickers with their cluster + tier for the UI sidebar.
    pub async fn active_tickers(&self) -> Result<Vec<TickerRow>> {
        // Cast NUMERIC → float8 in SQL to avoid the bigdecimal feature pull-in.
        let rows = sqlx::query(
            r#"SELECT t.symbol,
                      COALESCE(t.cluster_id, '')        AS cluster_id,
                      c.name                            AS cluster_name,
                      t.tier,
                      t.options_eligible,
                      t.domain_fit::float8              AS domain_fit,
                      t.added_at,
                      (SELECT count(*) FROM thesis th
                        WHERE th.symbol = t.symbol
                          AND th.state NOT IN ('closed','disqualified')) AS open_theses
                 FROM ticker t
            LEFT JOIN cluster c ON c.id = t.cluster_id
                WHERE t.status = 'active'
             ORDER BY t.tier ASC, t.symbol ASC"#,
        )
        .fetch_all(&self.pool)
        .await
        .context("active_tickers")?;
        rows.into_iter()
            .map(|row| {
                Ok(TickerRow {
                    symbol: row.try_get("symbol")?,
                    cluster_id: row.try_get("cluster_id")?,
                    cluster_name: row.try_get::<Option<String>, _>("cluster_name")?,
                    tier: row.try_get("tier")?,
                    options_eligible: row.try_get("options_eligible")?,
                    domain_fit: row.try_get::<Option<f64>, _>("domain_fit").ok().flatten(),
                    added_at: row.try_get("added_at")?,
                    open_theses: row.try_get::<i64, _>("open_theses").unwrap_or(0),
                })
            })
            .collect()
    }

    /// Latest `ticker_context` row for a symbol. None if never synthesized.
    pub async fn latest_ticker_context(&self, symbol: &str) -> Result<Option<TickerContextRow>> {
        let row = sqlx::query(
            r#"SELECT symbol, version,
                      COALESCE(structural, '{}'::jsonb) AS structural,
                      structural_as_of,
                      COALESCE(narrative,  '{}'::jsonb) AS narrative,
                      narrative_as_of,
                      COALESCE(market,     '{}'::jsonb) AS market,
                      market_as_of,
                      created_at
                 FROM ticker_context
                WHERE symbol = $1
             ORDER BY version DESC
                LIMIT 1"#,
        )
        .bind(symbol)
        .fetch_optional(&self.pool)
        .await
        .context("latest_ticker_context")?;
        let Some(row) = row else { return Ok(None) };
        Ok(Some(TickerContextRow {
            symbol: row.try_get("symbol")?,
            version: row.try_get("version")?,
            structural: row.try_get("structural")?,
            structural_as_of: row.try_get("structural_as_of")?,
            narrative: row.try_get("narrative")?,
            narrative_as_of: row.try_get("narrative_as_of")?,
            market: row.try_get("market")?,
            market_as_of: row.try_get("market_as_of")?,
            created_at: row.try_get("created_at")?,
        }))
    }

    /// Loads all theses for a symbol plus their version-history audit trail.
    /// Returns most-recently-updated first so the UI sees the latest thesis on
    /// top when there are multiple.
    pub async fn theses_for_symbol(&self, symbol: &str) -> Result<Vec<ThesisDetail>> {
        let rows = sqlx::query(
            r#"SELECT thesis_id, symbol, cluster_id, cluster_thesis, state,
                      edge_rationale, bull_case, bear_case,
                      COALESCE(forecast, 'null'::jsonb)               AS forecast,
                      COALESCE(conviction_conditions, '[]'::jsonb)    AS conviction_conditions,
                      COALESCE(trigger_conditions, '[]'::jsonb)       AS trigger_conditions,
                      COALESCE(invalidation_conditions, '[]'::jsonb)  AS invalidation_conditions,
                      COALESCE(fulfillment_conditions, '[]'::jsonb)   AS fulfillment_conditions,
                      conviction_tier, instrument,
                      COALESCE(intended_size, 'null'::jsonb)          AS intended_size,
                      version,
                      COALESCE(immutable_original, '{}'::jsonb)       AS immutable_original,
                      created_at, updated_at
                 FROM thesis
                WHERE symbol = $1
             ORDER BY updated_at DESC"#,
        )
        .bind(symbol)
        .fetch_all(&self.pool)
        .await
        .context("theses_for_symbol")?;

        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let thesis_id: uuid::Uuid = row.try_get("thesis_id")?;
            let state_s: String = row.try_get("state")?;
            let state = serde_json::from_value(serde_json::Value::String(state_s))
                .map_err(|e| anyhow::anyhow!("decode ThesisState: {e}"))?;

            // Version history for this thesis.
            let hist_rows = sqlx::query(
                r#"SELECT version, weakens_invalidation,
                          COALESCE(diff, '{}'::jsonb) AS diff,
                          rationale, at
                     FROM thesis_version_history
                    WHERE thesis_id = $1
                 ORDER BY version DESC, at DESC"#,
            )
            .bind(thesis_id)
            .fetch_all(&self.pool)
            .await
            .context("thesis_version_history")?;

            let history: Vec<ThesisVersionEvent> = hist_rows
                .into_iter()
                .map(|r| ThesisVersionEvent {
                    version: r.try_get("version").unwrap_or(0),
                    weakens_invalidation: r.try_get("weakens_invalidation").unwrap_or(false),
                    diff: r.try_get("diff").unwrap_or(serde_json::Value::Null),
                    rationale: r.try_get::<Option<String>, _>("rationale").unwrap_or(None),
                    at: r.try_get("at").unwrap_or_else(|_| chrono::Utc::now()),
                })
                .collect();

            out.push(ThesisDetail {
                thesis_id,
                symbol: row.try_get("symbol")?,
                cluster_id: row.try_get("cluster_id").ok(),
                cluster_thesis: row.try_get("cluster_thesis").ok(),
                state,
                edge_rationale: row.try_get("edge_rationale")?,
                bull_case: row.try_get("bull_case").ok(),
                bear_case: row.try_get("bear_case").ok(),
                forecast: row.try_get("forecast")?,
                conviction_conditions: row.try_get("conviction_conditions")?,
                trigger_conditions: row.try_get("trigger_conditions")?,
                invalidation_conditions: row.try_get("invalidation_conditions")?,
                fulfillment_conditions: row.try_get("fulfillment_conditions")?,
                conviction_tier: row.try_get("conviction_tier").ok(),
                instrument: row.try_get("instrument").ok(),
                intended_size: row.try_get("intended_size")?,
                version: row.try_get("version")?,
                immutable_original: row.try_get("immutable_original")?,
                created_at: row.try_get("created_at")?,
                updated_at: row.try_get("updated_at")?,
                history,
            });
        }
        Ok(out)
    }

    /// Records a single LLM call to the audit table (#6). Pair with
    /// `llm::prompts::invoke` — the recorder is wired via the trait impl
    /// below.
    pub async fn record_llm_invocation(
        &self,
        prompt_name: &str,
        prompt_hash: &str,
        provider: &str,
        model: &str,
        input_tokens: i32,
        output_tokens: i32,
        latency_ms: i32,
        request_summary: &str,
        response_summary: &str,
    ) -> Result<()> {
        sqlx::query(
            r#"INSERT INTO llm_invocation
                 (prompt_name, prompt_hash, provider, model,
                  input_tokens, output_tokens, latency_ms,
                  request_summary, response_summary)
               VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)"#,
        )
        .bind(prompt_name)
        .bind(prompt_hash)
        .bind(provider)
        .bind(model)
        .bind(input_tokens)
        .bind(output_tokens)
        .bind(latency_ms)
        .bind(request_summary)
        .bind(response_summary)
        .execute(&self.pool)
        .await
        .context("record_llm_invocation")?;
        Ok(())
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

#[async_trait::async_trait]
impl InvocationRecorder for Store {
    async fn record(&self, row: InvocationRow<'_>) -> Result<()> {
        // i32 cast is fine: token counts ≤ ~200k per call; latency ≤ ~10min.
        self.record_llm_invocation(
            row.prompt_name,
            row.prompt_hash,
            row.provider,
            row.model,
            i32::try_from(row.input_tokens).unwrap_or(i32::MAX),
            i32::try_from(row.output_tokens).unwrap_or(i32::MAX),
            i32::try_from(row.latency_ms).unwrap_or(i32::MAX),
            row.request_summary,
            row.response_summary,
        )
        .await
    }
}
