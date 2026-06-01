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
    Alert, AlertKind, Condition, MarketStateRow, ThesisDetail, ThesisSubstance, ThesisVersionEvent,
    TickerContextRow, TickerRow, Watchlist, WatchlistMember, WellFormedCondCounts,
};
use crate::thesis::substance::{self, Thesis as SubstanceInput};

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

    pub async fn mark_source_started(&self, source: &str, symbols_attempted: i32) -> Result<()> {
        sqlx::query(
            r#"INSERT INTO source_health
                 (source, last_started_at, last_status, symbols_attempted, updated_at)
               VALUES ($1, now(), 'running', $2, now())
               ON CONFLICT (source) DO UPDATE SET
                   last_started_at = EXCLUDED.last_started_at,
                   last_status = 'running',
                   symbols_attempted = EXCLUDED.symbols_attempted,
                   updated_at = now()"#,
        )
        .bind(source)
        .bind(symbols_attempted)
        .execute(&self.pool)
        .await
        .with_context(|| format!("mark_source_started {source}"))?;
        Ok(())
    }

    pub async fn record_source_success(
        &self,
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
        .execute(&self.pool)
        .await
        .with_context(|| format!("record_source_success {source}"))?;
        Ok(())
    }

    pub async fn record_source_failure(
        &self,
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
        .execute(&self.pool)
        .await
        .with_context(|| format!("record_source_failure {source}"))?;
        Ok(())
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

    /// Reads the operator-set portfolio frame (#26). Returns the singleton
    /// row; `account_size_usd` is `None` until the operator sets it.
    pub async fn portfolio_settings(&self) -> Result<crate::risk::PortfolioSettings> {
        let row = sqlx::query(
            r#"SELECT account_size_usd::float8 AS acct,
                      high_water_mark_usd::float8 AS hwm
                 FROM portfolio_settings WHERE id = 1"#,
        )
        .fetch_optional(&self.pool)
        .await
        .context("portfolio_settings")?;
        let Some(row) = row else {
            return Ok(crate::risk::PortfolioSettings::default());
        };
        Ok(crate::risk::PortfolioSettings {
            account_size_usd: row.try_get::<Option<f64>, _>("acct").ok().flatten(),
            high_water_mark_usd: row.try_get::<Option<f64>, _>("hwm").ok().flatten(),
        })
    }

    /// Upsert operator-set account size + high-water mark. Either field may
    /// be left `None` (caller's intent: "don't touch this field").
    pub async fn upsert_portfolio_settings(
        &self,
        account_size_usd: Option<f64>,
        high_water_mark_usd: Option<f64>,
        updated_by: &str,
    ) -> Result<()> {
        sqlx::query(
            r#"INSERT INTO portfolio_settings (id, account_size_usd, high_water_mark_usd, updated_at, updated_by)
               VALUES (1, $1, $2, now(), $3)
               ON CONFLICT (id) DO UPDATE SET
                   account_size_usd = COALESCE(EXCLUDED.account_size_usd, portfolio_settings.account_size_usd),
                   high_water_mark_usd = COALESCE(EXCLUDED.high_water_mark_usd, portfolio_settings.high_water_mark_usd),
                   updated_at = now(),
                   updated_by = EXCLUDED.updated_by"#,
        )
        .bind(account_size_usd)
        .bind(high_water_mark_usd)
        .bind(updated_by)
        .execute(&self.pool)
        .await
        .context("upsert_portfolio_settings")?;
        Ok(())
    }

    /// Union of active tickers + active discovery pool members. Use this
    /// from any cognition-supporting ingest (news, estimates, XBRL) so the
    /// data follows the broader pool (#104) — not just the curated universe.
    pub async fn scan_pool_symbols(&self) -> Result<Vec<String>> {
        let rows: Vec<(String,)> = sqlx::query_as(
            r#"SELECT symbol FROM (
                  SELECT symbol FROM ticker WHERE status = 'active'
                  UNION
                  SELECT symbol FROM discovery_pool WHERE dropped_at IS NULL
               ) s
               ORDER BY symbol"#,
        )
        .fetch_all(&self.pool)
        .await
        .context("scan_pool_symbols")?;
        Ok(rows.into_iter().map(|r| r.0).collect())
    }

    /// Active discovery pool symbols (not dropped). Used by the discovery
    /// scanner instead of `ticker` so it can fire signals on names we
    /// don't yet track (#88).
    pub async fn discovery_pool_symbols(&self) -> Result<Vec<String>> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT symbol FROM discovery_pool WHERE dropped_at IS NULL ORDER BY symbol",
        )
        .fetch_all(&self.pool)
        .await
        .context("discovery_pool_symbols")?;
        Ok(rows.into_iter().map(|r| r.0).collect())
    }

    /// For each symbol, return the OLDEST bar timestamp we have (None when
    /// we have no bars yet). Lets the price ingest decide cold-start vs
    /// incremental backfill per ticker.
    pub async fn oldest_bar_per_symbol(
        &self,
        symbols: &[String],
    ) -> Result<std::collections::HashMap<String, Option<DateTime<Utc>>>> {
        let mut out: std::collections::HashMap<String, Option<DateTime<Utc>>> =
            symbols.iter().map(|s| (s.clone(), None)).collect();
        if symbols.is_empty() {
            return Ok(out);
        }
        let rows = sqlx::query(
            r#"SELECT symbol, MIN(ts) AS min_ts
                 FROM price_bar
                WHERE symbol = ANY($1)
             GROUP BY symbol"#,
        )
        .bind(symbols)
        .fetch_all(&self.pool)
        .await
        .context("oldest_bar_per_symbol")?;
        for r in rows {
            let s: String = r.try_get("symbol")?;
            let ts: Option<DateTime<Utc>> = r.try_get("min_ts")?;
            out.insert(s, ts);
        }
        Ok(out)
    }

    /// All open positions in the shape the risk overlay consumes.
    // ---------- attention_item (#86) ----------

    /// Upsert an attention item. The partial-unique indexes mean a second
    /// open item for the same (kind, candidate_id) / (kind, thesis_id) /
    /// (kind, symbol) will collide; we no-op on conflict so producers can
    /// fire freely without dedup logic in each call site.
    pub async fn upsert_attention(
        &self,
        kind: &str,
        symbol: Option<&str>,
        thesis_id: Option<uuid::Uuid>,
        candidate_id: Option<i64>,
        severity: &str,
        title: &str,
        reason: Option<&str>,
        source: &str,
        source_ref: serde_json::Value,
    ) -> Result<()> {
        let (fsm_state, owner) = crate::attention::initial_assignment(kind, severity, source);
        sqlx::query(
            r#"INSERT INTO attention_item
                 (kind, symbol, thesis_id, candidate_id, severity, title,
                  reason, source, source_ref, fsm_state, owner, state_reason)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9::jsonb, $10, $11, $12)
               ON CONFLICT DO NOTHING"#,
        )
        .bind(kind)
        .bind(symbol)
        .bind(thesis_id)
        .bind(candidate_id)
        .bind(severity)
        .bind(title)
        .bind(reason)
        .bind(source)
        .bind(source_ref)
        .bind(fsm_state)
        .bind(owner)
        .bind(kind)
        .execute(&self.pool)
        .await
        .context("upsert_attention")?;
        Ok(())
    }

    /// Resolve attention items matching a filter. Idempotent (resolves only
    /// items still 'open'). Returns how many rows transitioned.
    pub async fn resolve_attention(
        &self,
        kind: &str,
        thesis_id: Option<uuid::Uuid>,
        candidate_id: Option<i64>,
        resolution_kind: &str,
        resolution_ref: serde_json::Value,
    ) -> Result<u64> {
        let rows: i64 = sqlx::query_scalar(
            r#"WITH matched AS (
                    SELECT id, fsm_state
                      FROM attention_item
                     WHERE status = 'open'
                       AND kind = $1
                       AND ($2::uuid IS NULL OR thesis_id = $2)
                       AND ($3::bigint IS NULL OR candidate_id = $3)
                     FOR UPDATE
                 ),
                 updated AS (
                    UPDATE attention_item ai
                       SET status = 'resolved',
                           fsm_state = 'resolved',
                           owner = 'system',
                           resolved_at = now(),
                           resolution_kind = $4,
                           resolution_ref = $5::jsonb,
                           next_retry_at = NULL,
                           resurface_at = NULL,
                           state_reason = $4
                      FROM matched m
                     WHERE ai.id = m.id
                 RETURNING ai.id,
                           m.fsm_state AS from_state,
                           ai.fsm_state AS to_state,
                           ai.owner,
                           ai.state_reason,
                           ai.next_retry_at,
                           ai.resurface_at,
                           ai.resolution_ref
                 ),
                 inserted AS (
                    INSERT INTO attention_state_history
                         (attention_id, from_state, to_state, owner, reason,
                          next_retry_at, resurface_at, source_ref)
                    SELECT id, from_state, to_state, owner, state_reason,
                           next_retry_at, resurface_at, resolution_ref
                      FROM updated
                 RETURNING 1
                 )
              SELECT COUNT(*) FROM updated"#,
        )
        .bind(kind)
        .bind(thesis_id)
        .bind(candidate_id)
        .bind(resolution_kind)
        .bind(resolution_ref)
        .fetch_one(&self.pool)
        .await
        .context("resolve_attention")?;
        Ok(rows as u64)
    }

    /// Mark items as dismissed (operator chose "not relevant"). Same filter
    /// shape as resolve_attention.
    pub async fn dismiss_attention(&self, id: i64, reason: Option<&str>) -> Result<bool> {
        let rows: i64 = if reason == Some("defer") {
            sqlx::query_scalar(
                r#"WITH matched AS (
                        SELECT id, fsm_state
                          FROM attention_item
                         WHERE id = $1 AND status = 'open'
                         FOR UPDATE
                     ),
                     updated AS (
                        UPDATE attention_item ai
                           SET status = 'open',
                               fsm_state = 'operator_deferred',
                               owner = 'operator',
                               resolved_at = NULL,
                               resolution_kind = 'deferred',
                               resolution_ref = jsonb_build_object('reason', 'defer'),
                               next_retry_at = NULL,
                               resurface_at = now() + interval '7 days',
                               state_reason = 'defer'
                          FROM matched m
                         WHERE ai.id = m.id
                     RETURNING ai.id,
                               m.fsm_state AS from_state,
                               ai.fsm_state AS to_state,
                               ai.owner,
                               ai.state_reason,
                               ai.next_retry_at,
                               ai.resurface_at,
                               ai.resolution_ref
                     ),
                     inserted AS (
                        INSERT INTO attention_state_history
                             (attention_id, from_state, to_state, owner, reason,
                              next_retry_at, resurface_at, source_ref)
                        SELECT id, from_state, to_state, owner, state_reason,
                               next_retry_at, resurface_at, resolution_ref
                          FROM updated
                     RETURNING 1
                     )
                  SELECT COUNT(*) FROM updated"#,
            )
            .bind(id)
            .fetch_one(&self.pool)
            .await
            .context("defer_attention")?
        } else {
            sqlx::query_scalar(
                r#"WITH matched AS (
                        SELECT id, fsm_state
                          FROM attention_item
                         WHERE id = $1 AND status = 'open'
                         FOR UPDATE
                     ),
                     updated AS (
                        UPDATE attention_item ai
                           SET status = 'dismissed',
                               fsm_state = 'dismissed',
                               owner = 'operator',
                               resolved_at = now(),
                               resolution_kind = 'dismissed',
                               resolution_ref = jsonb_build_object('reason', COALESCE($2::text, '')),
                               next_retry_at = NULL,
                               resurface_at = NULL,
                               state_reason = COALESCE(NULLIF($2::text, ''), 'dismissed')
                          FROM matched m
                         WHERE ai.id = m.id
                     RETURNING ai.id,
                               m.fsm_state AS from_state,
                               ai.fsm_state AS to_state,
                               ai.owner,
                               ai.state_reason,
                               ai.next_retry_at,
                               ai.resurface_at,
                               ai.resolution_ref
                     ),
                     inserted AS (
                        INSERT INTO attention_state_history
                             (attention_id, from_state, to_state, owner, reason,
                              next_retry_at, resurface_at, source_ref)
                        SELECT id, from_state, to_state, owner, state_reason,
                               next_retry_at, resurface_at, resolution_ref
                          FROM updated
                     RETURNING 1
                     )
                  SELECT COUNT(*) FROM updated"#,
            )
            .bind(id)
            .bind(reason)
            .fetch_one(&self.pool)
            .await
            .context("dismiss_attention")?
        };
        Ok(rows > 0)
    }

    /// Open attention items, severity-then-recency ordering.
    pub async fn list_attention(&self, status: &str, limit: i64) -> Result<Vec<serde_json::Value>> {
        let rows = sqlx::query(
            r#"SELECT id, kind, symbol, thesis_id, candidate_id, severity,
                      status, fsm_state, owner, title, reason, source, source_ref,
                      created_at, resolved_at, resolution_kind,
                      next_retry_at, resurface_at, state_reason
                 FROM attention_item
                WHERE status = $1
                  AND (
                    $1 <> 'open'
                    OR fsm_state <> 'operator_deferred'
                    OR (resurface_at IS NOT NULL AND resurface_at <= now())
                  )
             ORDER BY
                CASE severity WHEN 'blocked' THEN 0 WHEN 'decision' THEN 1
                              WHEN 'review' THEN 2 ELSE 3 END,
                created_at DESC
                LIMIT $2"#,
        )
        .bind(status)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .context("list_attention")?;
        rows.into_iter()
            .map(|r| {
                let created_at: DateTime<Utc> = r.try_get("created_at")?;
                let resolved_at: Option<DateTime<Utc>> = r.try_get("resolved_at")?;
                let next_retry_at: Option<DateTime<Utc>> = r.try_get("next_retry_at")?;
                let resurface_at: Option<DateTime<Utc>> = r.try_get("resurface_at")?;
                Ok(serde_json::json!({
                    "id": r.try_get::<i64, _>("id")?,
                    "kind": r.try_get::<String, _>("kind")?,
                    "symbol": r.try_get::<Option<String>, _>("symbol")?,
                    "thesis_id": r.try_get::<Option<uuid::Uuid>, _>("thesis_id")?,
                    "candidate_id": r.try_get::<Option<i64>, _>("candidate_id")?,
                    "severity": r.try_get::<String, _>("severity")?,
                    "status": r.try_get::<String, _>("status")?,
                    "fsm_state": r.try_get::<String, _>("fsm_state")?,
                    "owner": r.try_get::<String, _>("owner")?,
                    "title": r.try_get::<String, _>("title")?,
                    "reason": r.try_get::<Option<String>, _>("reason")?,
                    "source": r.try_get::<String, _>("source")?,
                    "source_ref": r.try_get::<serde_json::Value, _>("source_ref")?,
                    "created_at": created_at,
                    "resolved_at": resolved_at,
                    "resolution_kind": r.try_get::<Option<String>, _>("resolution_kind")?,
                    "next_retry_at": next_retry_at,
                    "resurface_at": resurface_at,
                    "state_reason": r.try_get::<Option<String>, _>("state_reason")?,
                }))
            })
            .collect()
    }

    pub async fn thesis_declines_for_symbol(
        &self,
        symbol: &str,
        limit: i64,
    ) -> Result<Vec<serde_json::Value>> {
        let rows = sqlx::query(
            r#"SELECT id, symbol, candidate_id, severity, status, title, reason,
                      source_ref, created_at, resolved_at, resolution_kind
                 FROM attention_item
                WHERE symbol = $1
                  AND kind = 'thesis_incomplete'
             ORDER BY created_at DESC
                LIMIT $2"#,
        )
        .bind(symbol)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .context("thesis_declines_for_symbol")?;
        rows.into_iter()
            .map(|r| {
                let created_at: DateTime<Utc> = r.try_get("created_at")?;
                let resolved_at: Option<DateTime<Utc>> = r.try_get("resolved_at")?;
                Ok(serde_json::json!({
                    "id": r.try_get::<i64, _>("id")?,
                    "symbol": r.try_get::<Option<String>, _>("symbol")?,
                    "candidate_id": r.try_get::<Option<i64>, _>("candidate_id")?,
                    "severity": r.try_get::<String, _>("severity")?,
                    "status": r.try_get::<String, _>("status")?,
                    "title": r.try_get::<String, _>("title")?,
                    "reason": r.try_get::<Option<String>, _>("reason")?,
                    "source_ref": r.try_get::<serde_json::Value, _>("source_ref")?,
                    "created_at": created_at,
                    "resolved_at": resolved_at,
                    "resolution_kind": r.try_get::<Option<String>, _>("resolution_kind")?,
                }))
            })
            .collect()
    }

    pub async fn evidence_requirements_for_symbol(
        &self,
        symbol: &str,
    ) -> Result<Vec<serde_json::Value>> {
        let rows = sqlx::query(
            r#"SELECT id, symbol, requirement_key, source_type, reason, priority,
                      blocking_state, attempts, next_retry_at, last_error,
                      source_ref, created_at, updated_at, satisfied_at
                 FROM evidence_requirement
                WHERE symbol = $1
             ORDER BY
                  CASE priority
                       WHEN 'blocking' THEN 0
                       WHEN 'high' THEN 1
                       WHEN 'medium' THEN 2
                       ELSE 3
                  END,
                  CASE blocking_state
                       WHEN 'missing' THEN 0
                       WHEN 'partial' THEN 1
                       WHEN 'blocked' THEN 2
                       WHEN 'fetching' THEN 3
                       ELSE 4
                  END,
                  updated_at DESC"#,
        )
        .bind(symbol)
        .fetch_all(&self.pool)
        .await
        .context("evidence_requirements_for_symbol")?;
        rows.into_iter()
            .map(|r| {
                let created_at: DateTime<Utc> = r.try_get("created_at")?;
                let updated_at: DateTime<Utc> = r.try_get("updated_at")?;
                let next_retry_at: Option<DateTime<Utc>> = r.try_get("next_retry_at")?;
                let satisfied_at: Option<DateTime<Utc>> = r.try_get("satisfied_at")?;
                Ok(serde_json::json!({
                    "id": r.try_get::<i64, _>("id")?,
                    "symbol": r.try_get::<String, _>("symbol")?,
                    "requirement_key": r.try_get::<String, _>("requirement_key")?,
                    "source_type": r.try_get::<String, _>("source_type")?,
                    "reason": r.try_get::<String, _>("reason")?,
                    "priority": r.try_get::<String, _>("priority")?,
                    "blocking_state": r.try_get::<String, _>("blocking_state")?,
                    "attempts": r.try_get::<i32, _>("attempts")?,
                    "next_retry_at": next_retry_at,
                    "last_error": r.try_get::<Option<String>, _>("last_error")?,
                    "source_ref": r.try_get::<serde_json::Value, _>("source_ref")?,
                    "created_at": created_at,
                    "updated_at": updated_at,
                    "satisfied_at": satisfied_at,
                }))
            })
            .collect()
    }

    /// Recent decisions for a given symbol — joins through thesis to filter.
    pub async fn decisions_for_symbol(&self, symbol: &str) -> Result<Vec<serde_json::Value>> {
        let rows = sqlx::query(
            r#"SELECT d.decision_id, d.thesis_id, d.action, d.user_choice,
                      d.sizing, d.at, t.state AS thesis_state,
                      t.forecast->>'direction' AS thesis_direction,
                      COALESCE(d.sizing->>'side', '') AS side,
                      COALESCE(d.sizing->>'instrument', t.instrument) AS instrument
                 FROM decision d
                 JOIN thesis t USING (thesis_id)
                WHERE t.symbol = $1
             ORDER BY d.at DESC LIMIT 100"#,
        )
        .bind(symbol)
        .fetch_all(&self.pool)
        .await
        .context("decisions_for_symbol")?;
        rows.into_iter()
            .map(|r| {
                let at: DateTime<Utc> = r.try_get("at")?;
                Ok(serde_json::json!({
                    "decision_id": r.try_get::<uuid::Uuid, _>("decision_id")?,
                    "thesis_id": r.try_get::<Option<uuid::Uuid>, _>("thesis_id")?,
                    "action": r.try_get::<String, _>("action")?,
                    "user_choice": r.try_get::<Option<String>, _>("user_choice")?,
                    "sizing": r.try_get::<Option<serde_json::Value>, _>("sizing")?,
                    "thesis_state": r.try_get::<String, _>("thesis_state")?,
                    "thesis_direction": r.try_get::<Option<String>, _>("thesis_direction")?,
                    "side": r.try_get::<String, _>("side")?,
                    "instrument": r.try_get::<Option<String>, _>("instrument")?,
                    "at": at,
                }))
            })
            .collect()
    }

    /// Returns timestamped events for a symbol — thesis state transitions,
    /// risk alerts, decisions — for chart marker overlays (#57 PR3).
    pub async fn symbol_events(
        &self,
        symbol: &str,
        lookback_days: i64,
    ) -> Result<Vec<serde_json::Value>> {
        let rows = sqlx::query(
            r#"
            -- thesis state transitions (one row per state hop)
            SELECT 'state_transition' AS kind,
                   tsh.at AS at,
                   t.thesis_id::text AS thesis_id,
                   tsh.to_state AS label,
                   COALESCE(tsh.rationale, '') AS detail
              FROM thesis_state_history tsh
              JOIN thesis t USING (thesis_id)
             WHERE t.symbol = $1 AND tsh.at > now() - ($2 || ' days')::interval
            UNION ALL
            -- risk + state-transition alerts
            SELECT a.kind AS kind,
                   a.created_at AS at,
                   COALESCE(a.thesis_id::text, '') AS thesis_id,
                   COALESCE(a.payload->>'kind', a.kind) AS label,
                   COALESCE(a.payload->>'reasons', '') AS detail
              FROM alert a
             WHERE a.symbol = $1 AND a.created_at > now() - ($2 || ' days')::interval
            UNION ALL
            -- recorded decisions
            SELECT 'decision' AS kind,
                   d.at AS at,
                   COALESCE(d.thesis_id::text, '') AS thesis_id,
                   CASE
                     WHEN d.action = 'enter' AND COALESCE(d.sizing->>'side', '') <> ''
                       THEN d.action || ' ' || (d.sizing->>'side')
                     WHEN d.action = 'enter' AND t.forecast->>'direction' = 'down'
                       THEN 'enter bearish'
                     WHEN d.action = 'enter' AND t.forecast->>'direction' = 'up'
                       THEN 'enter bullish'
                     ELSE d.action
                   END AS label,
                   COALESCE(d.user_choice, '') AS detail
              FROM decision d
              JOIN thesis t USING (thesis_id)
             WHERE t.symbol = $1 AND d.at > now() - ($2 || ' days')::interval
         ORDER BY at ASC"#,
        )
        .bind(symbol)
        .bind(lookback_days.to_string())
        .fetch_all(&self.pool)
        .await
        .context("symbol_events")?;
        rows.into_iter()
            .map(|r| {
                let at: DateTime<Utc> = r.try_get("at")?;
                Ok(serde_json::json!({
                    "kind": r.try_get::<String, _>("kind")?,
                    "time": at.format("%Y-%m-%d").to_string(),
                    "thesis_id": r.try_get::<String, _>("thesis_id")?,
                    "label": r.try_get::<String, _>("label")?,
                    "detail": r.try_get::<String, _>("detail")?,
                }))
            })
            .collect()
    }

    /// Daily-or-higher candles for `symbol` over the last `lookback_days`, oldest first.
    /// Shaped for lightweight-charts (each row has `time` as ISO date + OHLCV).
    ///
    /// `price_bar` can contain multiple timestamps on the same UTC date when
    /// backfills and refreshes come from different feeds. The chart library
    /// requires strictly increasing unique times, so collapse bars to one
    /// candle per date at the API boundary.
    pub async fn candles_for(
        &self,
        symbol: &str,
        lookback_days: i64,
        interval: &str,
    ) -> Result<Vec<serde_json::Value>> {
        let rows = sqlx::query(
            r#"WITH daily AS (
                 SELECT (date_trunc('day', ts AT TIME ZONE 'UTC'))::date AS day,
                        (array_agg(open::float8 ORDER BY ts ASC))[1] AS open,
                        max(high::float8) AS high,
                        min(low::float8) AS low,
                        (array_agg(close::float8 ORDER BY ts DESC))[1] AS close,
                        sum(volume::float8) AS volume
                   FROM price_bar
                  WHERE symbol = $1
                    AND ts > now() - ($2 || ' days')::interval
               GROUP BY 1
             ), bucketed AS (
                 SELECT CASE
                          WHEN $3 = '1W' THEN date_trunc('week', day::timestamp)::date
                          WHEN $3 = '3W' THEN (DATE '1970-01-05' + ((((day - DATE '1970-01-05') / 21)::int) * 21))
                          WHEN $3 = '1M' THEN date_trunc('month', day::timestamp)::date
                          ELSE day
                        END AS bucket,
                        day, open, high, low, close, volume
                   FROM daily
             )
             SELECT bucket AS day,
                    (array_agg(open ORDER BY day ASC))[1] AS open,
                    max(high) AS high,
                    min(low) AS low,
                    (array_agg(close ORDER BY day DESC))[1] AS close,
                    sum(volume) AS volume
               FROM bucketed
              GROUP BY bucket
              ORDER BY bucket ASC"#,
        )
        .bind(symbol)
        .bind(lookback_days.to_string())
        .bind(interval)
        .fetch_all(&self.pool)
        .await
        .context("candles_for")?;
        rows.into_iter()
            .map(|r| {
                let day: chrono::NaiveDate = r.try_get("day")?;
                Ok(serde_json::json!({
                    "time": day.format("%Y-%m-%d").to_string(),
                    "open": r.try_get::<f64, _>("open")?,
                    "high": r.try_get::<f64, _>("high")?,
                    "low": r.try_get::<f64, _>("low")?,
                    "close": r.try_get::<f64, _>("close")?,
                    "volume": r.try_get::<f64, _>("volume")?,
                }))
            })
            .collect()
    }

    pub async fn latest_intraday_bar_ts(
        &self,
        symbol: &str,
        native_interval: &str,
    ) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
        let row = sqlx::query(
            "SELECT max(ts) AS ts FROM price_bar_intraday WHERE symbol = $1 AND interval = $2",
        )
        .bind(symbol)
        .bind(native_interval)
        .fetch_one(&self.pool)
        .await
        .context("latest_intraday_bar_ts")?;
        Ok(row.try_get("ts")?)
    }

    pub async fn intraday_candles_for(
        &self,
        symbol: &str,
        native_interval: &str,
        lookback_days: i64,
        bucket_minutes: i64,
    ) -> Result<Vec<serde_json::Value>> {
        let rows = sqlx::query(
            r#"WITH bucketed AS (
                 SELECT to_timestamp(floor(extract(epoch FROM ts) / ($4::float8 * 60.0)) * ($4::float8 * 60.0)) AS bucket,
                        ts, open::float8 AS open, high::float8 AS high, low::float8 AS low,
                        close::float8 AS close, volume::float8 AS volume
                   FROM price_bar_intraday
                  WHERE symbol = $1
                    AND interval = $2
                    AND ts > now() - ($3 || ' days')::interval
             )
             SELECT bucket,
                    (array_agg(open ORDER BY ts ASC))[1] AS open,
                    max(high) AS high,
                    min(low) AS low,
                    (array_agg(close ORDER BY ts DESC))[1] AS close,
                    sum(volume) AS volume
               FROM bucketed
              GROUP BY bucket
              ORDER BY bucket ASC"#,
        )
        .bind(symbol)
        .bind(native_interval)
        .bind(lookback_days.to_string())
        .bind(bucket_minutes)
        .fetch_all(&self.pool)
        .await
        .context("intraday_candles_for")?;
        rows.into_iter()
            .map(|r| {
                let bucket: chrono::DateTime<chrono::Utc> = r.try_get("bucket")?;
                Ok(serde_json::json!({
                    "time": bucket.to_rfc3339(),
                    "open": r.try_get::<f64, _>("open")?,
                    "high": r.try_get::<f64, _>("high")?,
                    "low": r.try_get::<f64, _>("low")?,
                    "close": r.try_get::<f64, _>("close")?,
                    "volume": r.try_get::<f64, _>("volume")?,
                }))
            })
            .collect()
    }

    pub async fn open_positions_for_risk(&self) -> Result<Vec<crate::risk::Position>> {
        let rows = sqlx::query(
            r#"SELECT p.symbol,
                      COALESCE(t.cluster_id, '') AS cluster,
                      p.instrument,
                      COALESCE(p.delta_notional, 0)::float8 AS delta_notional,
                      COALESCE(p.premium_at_risk, 0)::float8 AS premium_at_risk
                 FROM position p
                 LEFT JOIN ticker t ON t.symbol = p.symbol
                WHERE p.closed_at IS NULL"#,
        )
        .fetch_all(&self.pool)
        .await
        .context("open_positions_for_risk")?;
        rows.into_iter()
            .map(|row| {
                Ok(crate::risk::Position {
                    symbol: row.try_get("symbol")?,
                    cluster: row.try_get("cluster")?,
                    instrument: row.try_get("instrument")?,
                    delta_notional: row.try_get::<f64, _>("delta_notional")?,
                    premium_at_risk: row.try_get::<f64, _>("premium_at_risk")?,
                })
            })
            .collect()
    }

    /// Sum of realized PnL across closed positions. Used by the risk overlay
    /// to compute realized drawdown (#26). Treats NULL as 0.
    pub async fn realized_pnl_total(&self) -> Result<f64> {
        let row = sqlx::query(
            r#"SELECT COALESCE(SUM(realized_pnl), 0)::float8 AS total
                 FROM position WHERE closed_at IS NOT NULL"#,
        )
        .fetch_one(&self.pool)
        .await
        .context("realized_pnl_total")?;
        Ok(row.try_get::<f64, _>("total")?)
    }

    /// Inserts an alert and returns its id.
    pub async fn insert_alert(
        &self,
        kind: AlertKind,
        symbol: Option<&str>,
        payload: &[u8],
    ) -> Result<i64> {
        let payload_str = std::str::from_utf8(payload).context("payload utf-8")?;
        let payload_json: serde_json::Value = serde_json::from_str(payload_str).unwrap_or_default();
        let inferred_symbol = symbol.map(str::to_string).or_else(|| {
            payload_json
                .get("symbol")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        });
        let thesis_id = payload_json
            .get("thesis_id")
            .and_then(|v| v.as_str())
            .and_then(|s| uuid::Uuid::parse_str(s).ok());
        let row = sqlx::query(
            r#"INSERT INTO alert (kind, symbol, thesis_id, payload)
               VALUES ($1, $2, $3, $4::jsonb)
            RETURNING id"#,
        )
        .bind(kind.as_str())
        .bind(inferred_symbol)
        .bind(thesis_id)
        .bind(payload_str)
        .fetch_one(&self.pool)
        .await
        .context("insert_alert")?;
        let id: i64 = row.try_get("id")?;
        Ok(id)
    }

    /// Marks an alert acknowledged. Idempotent — re-acking is a no-op.
    /// Returns true if a row was updated, false if no such alert existed.
    pub async fn acknowledge_alert(&self, id: i64) -> Result<bool> {
        let res = sqlx::query("UPDATE alert SET acknowledged = true WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .context("acknowledge_alert")?;
        Ok(res.rows_affected() > 0)
    }

    /// Returns the most recent alerts for the UI feed. When
    /// `only_unacked` is true (the default for the live-feed view), filters
    /// out alerts the user has already dismissed.
    pub async fn recent_alerts_filtered(
        &self,
        limit: i64,
        only_unacked: bool,
    ) -> Result<Vec<Alert>> {
        let rows = if only_unacked {
            sqlx::query(
                r#"SELECT id, thesis_id, symbol, kind, payload, acknowledged, created_at
                     FROM alert WHERE acknowledged = false
                 ORDER BY created_at DESC LIMIT $1"#,
            )
        } else {
            sqlx::query(
                r#"SELECT id, thesis_id, symbol, kind, payload, acknowledged, created_at
                     FROM alert ORDER BY created_at DESC LIMIT $1"#,
            )
        }
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .context("recent_alerts_filtered")?;

        rows.into_iter().map(decode_alert).collect()
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

        rows.into_iter().map(decode_alert).collect()
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
                      latest.thesis_id                  AS latest_thesis_id,
                      latest.state                      AS thesis_state,
                      latest.direction                   AS thesis_direction,
                      (SELECT count(*) FROM thesis th
                        WHERE th.symbol = t.symbol
                          AND th.state NOT IN ('closed','disqualified')) AS open_theses
                 FROM ticker t
            LEFT JOIN cluster c ON c.id = t.cluster_id
            LEFT JOIN LATERAL (
                SELECT th.thesis_id, th.state, th.forecast->>'direction' AS direction
                  FROM thesis th
                 WHERE th.symbol = t.symbol
                   AND th.state NOT IN ('closed','disqualified')
              ORDER BY th.updated_at DESC
                 LIMIT 1
            ) latest ON TRUE
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
                    latest_thesis_id: row.try_get("latest_thesis_id").ok(),
                    thesis_state: row.try_get("thesis_state").ok(),
                    thesis_direction: row.try_get("thesis_direction").ok(),
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

    /// Loads a single thesis by id, with the same enrichment that
    /// `theses_for_symbol` produces (substance, history). Returns
    /// `Vec<ThesisDetail>` (will have 0 or 1 entry) so the caller can reuse
    /// the existing per-symbol code path.
    pub async fn theses_for_symbol_id(&self, thesis_id: uuid::Uuid) -> Result<Vec<ThesisDetail>> {
        let symbol: Option<String> =
            sqlx::query_scalar("SELECT symbol FROM thesis WHERE thesis_id = $1")
                .bind(thesis_id)
                .fetch_optional(&self.pool)
                .await
                .context("symbol lookup")?;
        let Some(symbol) = symbol else {
            return Ok(vec![]);
        };
        let all = self.theses_for_symbol(&symbol).await?;
        Ok(all
            .into_iter()
            .filter(|t| t.thesis_id == thesis_id)
            .collect())
    }

    /// Apply a state transition (#15). Caller must have already validated the
    /// edge via `thesis::substance::promotion_allowed`. Writes both the new
    /// state on the thesis row and an append-only `thesis_state_history` row.
    pub async fn apply_state_transition(
        &self,
        thesis_id: uuid::Uuid,
        from: crate::platform::domain::ThesisState,
        to: crate::platform::domain::ThesisState,
        rationale: &str,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.context("begin tx")?;
        sqlx::query("UPDATE thesis SET state = $1, updated_at = now() WHERE thesis_id = $2")
            .bind(to.as_str())
            .bind(thesis_id)
            .execute(&mut *tx)
            .await
            .context("update thesis state")?;
        sqlx::query(
            r#"INSERT INTO thesis_state_history (thesis_id, from_state, to_state, rationale)
               VALUES ($1, $2, $3, NULLIF($4, ''))"#,
        )
        .bind(thesis_id)
        .bind(from.as_str())
        .bind(to.as_str())
        .bind(rationale)
        .execute(&mut *tx)
        .await
        .context("insert state history")?;
        // Attention queue producers/resolvers (#86) for state transitions.
        // Entering 'actionable' fires thesis_actionable; leaving 'actionable'
        // (forward to position_open OR backward to disqualified) resolves it.
        use crate::platform::domain::ThesisState;
        if matches!(to, ThesisState::Actionable) {
            // Look up the symbol for the title.
            let symbol: String =
                sqlx::query_scalar("SELECT symbol FROM thesis WHERE thesis_id = $1")
                    .bind(thesis_id)
                    .fetch_one(&mut *tx)
                    .await
                    .unwrap_or_default();
            let (fsm_state, owner) = crate::attention::initial_assignment(
                crate::attention::kind::THESIS_ACTIONABLE,
                crate::attention::severity::DECISION,
                crate::attention::source::THESIS,
            );
            sqlx::query(
                r#"INSERT INTO attention_item
                     (kind, symbol, thesis_id, severity, title, reason, source, source_ref,
                      fsm_state, owner, state_reason)
                   VALUES ('thesis_actionable', $1, $2, 'decision', $3, $4, 'thesis',
                           jsonb_build_object('from', $5::text, 'to', 'actionable'),
                           $6, $7, 'thesis_actionable')
                   ON CONFLICT DO NOTHING"#,
            )
            .bind(&symbol)
            .bind(thesis_id)
            .bind(format!("{symbol} thesis ready to act on"))
            .bind(if rationale.is_empty() {
                None
            } else {
                Some(rationale)
            })
            .bind(from.as_str())
            .bind(fsm_state)
            .bind(owner)
            .execute(&mut *tx)
            .await
            .context("attention thesis_actionable")?;
        }
        if matches!(from, ThesisState::Actionable) {
            sqlx::query(
                r#"WITH matched AS (
                        SELECT id, fsm_state
                          FROM attention_item
                         WHERE status = 'open'
                           AND kind = 'thesis_actionable'
                           AND thesis_id = $1
                         FOR UPDATE
                     ),
                     updated AS (
                        UPDATE attention_item ai
                           SET status = 'resolved',
                               fsm_state = 'resolved',
                               owner = 'system',
                               resolved_at = now(),
                               resolution_kind = 'thesis_advanced',
                               resolution_ref = jsonb_build_object('to', $2::text),
                               next_retry_at = NULL,
                               resurface_at = NULL,
                               state_reason = 'thesis_advanced'
                          FROM matched m
                         WHERE ai.id = m.id
                     RETURNING ai.id,
                               m.fsm_state AS from_state,
                               ai.fsm_state AS to_state,
                               ai.owner,
                               ai.state_reason,
                               ai.next_retry_at,
                               ai.resurface_at,
                               ai.resolution_ref
                     ),
                     inserted AS (
                        INSERT INTO attention_state_history
                             (attention_id, from_state, to_state, owner, reason,
                              next_retry_at, resurface_at, source_ref)
                        SELECT id, from_state, to_state, owner, state_reason,
                               next_retry_at, resurface_at, resolution_ref
                          FROM updated
                     RETURNING 1
                     )
                  SELECT COUNT(*) FROM updated"#,
            )
            .bind(thesis_id)
            .bind(to.as_str())
            .execute(&mut *tx)
            .await
            .context("attention thesis_actionable resolve")?;
        }
        tx.commit().await.context("commit tx")?;
        Ok(())
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

            let forecast: serde_json::Value = row.try_get("forecast")?;
            let conviction_conditions: serde_json::Value = row.try_get("conviction_conditions")?;
            let trigger_conditions: serde_json::Value = row.try_get("trigger_conditions")?;
            let invalidation_conditions: serde_json::Value =
                row.try_get("invalidation_conditions")?;
            let fulfillment_conditions: serde_json::Value =
                row.try_get("fulfillment_conditions")?;
            let intended_size: serde_json::Value = row.try_get("intended_size")?;

            let parse_conds = |v: &serde_json::Value| -> Vec<Condition> {
                serde_json::from_value(v.clone()).unwrap_or_default()
            };
            let conviction = parse_conds(&conviction_conditions);
            let trigger = parse_conds(&trigger_conditions);
            let invalidation = parse_conds(&invalidation_conditions);
            let fulfillment = parse_conds(&fulfillment_conditions);

            // Substance is "present" when forecast/intended_size is a non-null
            // populated value. The thesis engine writes `null` for absent.
            let forecast_present = !forecast.is_null()
                && !matches!(&forecast, serde_json::Value::Object(o) if o.is_empty());
            let intended_size_present = !intended_size.is_null()
                && !matches!(&intended_size, serde_json::Value::Object(o) if o.is_empty());
            let sub_input = SubstanceInput {
                forecast_present,
                intended_size_present,
                conviction: conviction.clone(),
                trigger: trigger.clone(),
                invalidation: invalidation.clone(),
                fulfillment: fulfillment.clone(),
            };
            let wfc = sub_input.well_formed_counts();
            let report = substance::substance_report(&sub_input);
            let substance_summary = ThesisSubstance {
                score: report.score,
                max_score: report.max_score,
                missing: report.missing,
                blocked_at: report.blocked_at,
                well_formed: WellFormedCondCounts {
                    conviction: u32::try_from(wfc.conviction).unwrap_or(0),
                    trigger: u32::try_from(wfc.trigger).unwrap_or(0),
                    invalidation: u32::try_from(wfc.invalidation).unwrap_or(0),
                    fulfillment: u32::try_from(wfc.fulfillment).unwrap_or(0),
                },
            };

            out.push(ThesisDetail {
                thesis_id,
                symbol: row.try_get("symbol")?,
                cluster_id: row.try_get("cluster_id").ok(),
                cluster_thesis: row.try_get("cluster_thesis").ok(),
                state,
                edge_rationale: row.try_get("edge_rationale")?,
                bull_case: row.try_get("bull_case").ok(),
                bear_case: row.try_get("bear_case").ok(),
                forecast,
                conviction_conditions,
                trigger_conditions,
                invalidation_conditions,
                fulfillment_conditions,
                conviction_tier: row.try_get("conviction_tier").ok(),
                instrument: row.try_get("instrument").ok(),
                intended_size,
                version: row.try_get("version")?,
                immutable_original: row.try_get("immutable_original")?,
                created_at: row.try_get("created_at")?,
                updated_at: row.try_get("updated_at")?,
                history,
                substance: Some(substance_summary),
            });
        }
        Ok(out)
    }

    /// List pending discovery candidates with their LLM classification (if any).
    /// Used by the review UI in #54 phase B.
    pub async fn pending_discovery_candidates(&self) -> Result<Vec<serde_json::Value>> {
        // Dedupe by (symbol, signal_name) — show only the most recent proposed
        // candidate per signal. Schema allows multiple rows per (sym, sig)
        // because the same signal can re-fire on different days, but the
        // user only wants one entry in the review queue per pending signal.
        let rows = sqlx::query(
            r#"SELECT * FROM (
                  SELECT DISTINCT ON (dc.symbol, dc.signal_name)
                         dc.id, dc.symbol, dc.signal_name, dc.signal_value, dc.domain_fit,
                         dc.proposed_tier, dc.reasoning, dc.proposed_at,
                         COALESCE(dcl.proposed_lists, '[]'::jsonb) AS proposed_lists,
                         dcl.suggested_new_list
                    FROM discovery_candidate dc
                    LEFT JOIN discovery_classification dcl ON dcl.candidate_id = dc.id
                   WHERE dc.status = 'proposed'
                ORDER BY dc.symbol, dc.signal_name, dc.proposed_at DESC
               ) latest
            ORDER BY proposed_at DESC
               LIMIT 100"#,
        )
        .fetch_all(&self.pool)
        .await
        .context("pending_discovery_candidates")?;
        let mut ranked = rows
            .into_iter()
            .map(|r| {
                let signal_value: Option<f64> = r.try_get("signal_value").ok();
                let proposed_lists: serde_json::Value = r.try_get("proposed_lists")?;
                let suggested_new_list = r
                    .try_get::<Option<serde_json::Value>, _>("suggested_new_list")
                    .unwrap_or(None);
                let proposed_at: chrono::DateTime<chrono::Utc> = r.try_get("proposed_at")?;
                let rank = crate::discovery::ranking::rank_candidate(
                    &r.try_get::<String, _>("signal_name")?,
                    signal_value,
                    r.try_get::<Option<f64>, _>("domain_fit").ok().flatten(),
                    r.try_get::<i32, _>("proposed_tier").unwrap_or(2),
                    &proposed_lists,
                    suggested_new_list.is_some(),
                );
                Ok((
                    rank.score,
                    proposed_at,
                    serde_json::json!({
                        "id": r.try_get::<i64, _>("id")?,
                        "symbol": r.try_get::<String, _>("symbol")?,
                        "signal_name": r.try_get::<String, _>("signal_name")?,
                        "signal_value": signal_value,
                        "domain_fit": r.try_get::<Option<f64>, _>("domain_fit").ok().flatten(),
                        "proposed_tier": r.try_get::<i32, _>("proposed_tier").unwrap_or(2),
                        "reasoning": r.try_get::<Option<String>, _>("reasoning").ok(),
                        "proposed_at": proposed_at,
                        "proposed_lists": proposed_lists,
                        "suggested_new_list": suggested_new_list,
                        "rank_score": rank.score,
                        "rank_bucket": rank.bucket,
                        "rank_reasons": rank.reasons,
                    }),
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        ranked.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.1.cmp(&a.1))
        });
        Ok(ranked.into_iter().map(|(_, _, value)| value).collect())
    }

    /// Confirm a candidate to one or more watchlists. Updates status, adds
    /// the symbol to each list (idempotent), records timestamp.
    pub async fn confirm_discovery_candidate(
        &self,
        candidate_id: i64,
        watchlist_ids: &[uuid::Uuid],
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.context("begin tx")?;
        let row = sqlx::query("SELECT symbol, signal_name FROM discovery_candidate WHERE id = $1")
            .bind(candidate_id)
            .fetch_one(&mut *tx)
            .await
            .context("load candidate")?;
        let symbol: String = row.try_get("symbol")?;
        let signal_name: String = row.try_get("signal_name")?;
        let added_by = format!("discovery:{signal_name}");
        // Ensure ticker exists (tier=2 default for fresh discoveries).
        sqlx::query("INSERT INTO ticker (symbol, tier) VALUES ($1, 2) ON CONFLICT DO NOTHING")
            .bind(&symbol)
            .execute(&mut *tx)
            .await?;
        for id in watchlist_ids {
            sqlx::query(
                r#"INSERT INTO watchlist_member (watchlist_id, symbol, added_by)
                   VALUES ($1, $2, $3) ON CONFLICT DO NOTHING"#,
            )
            .bind(id)
            .bind(&symbol)
            .bind(&added_by)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query(
            "UPDATE discovery_candidate SET status = 'confirmed', decided_at = now() WHERE id = $1",
        )
        .bind(candidate_id)
        .execute(&mut *tx)
        .await?;
        // Resolve the matching attention item (#86) inside the same tx so
        // queue + candidate status stay consistent.
        sqlx::query(
            r#"WITH matched AS (
                    SELECT id, fsm_state
                      FROM attention_item
                     WHERE status = 'open'
                       AND kind = 'candidate_review'
                       AND candidate_id = $1
                     FOR UPDATE
                 ),
                 updated AS (
                    UPDATE attention_item ai
                       SET status = 'resolved',
                           fsm_state = 'resolved',
                           owner = 'system',
                           resolved_at = now(),
                           resolution_kind = 'candidate_confirmed',
                           resolution_ref = jsonb_build_object('watchlist_ids', $2::text[]),
                           next_retry_at = NULL,
                           resurface_at = NULL,
                           state_reason = 'candidate_confirmed'
                      FROM matched m
                     WHERE ai.id = m.id
                 RETURNING ai.id,
                           m.fsm_state AS from_state,
                           ai.fsm_state AS to_state,
                           ai.owner,
                           ai.state_reason,
                           ai.next_retry_at,
                           ai.resurface_at,
                           ai.resolution_ref
                 ),
                 inserted AS (
                    INSERT INTO attention_state_history
                         (attention_id, from_state, to_state, owner, reason,
                          next_retry_at, resurface_at, source_ref)
                    SELECT id, from_state, to_state, owner, state_reason,
                           next_retry_at, resurface_at, resolution_ref
                      FROM updated
                 RETURNING 1
                 )
              SELECT COUNT(*) FROM updated"#,
        )
        .bind(candidate_id)
        .bind(
            watchlist_ids
                .iter()
                .map(uuid::Uuid::to_string)
                .collect::<Vec<_>>(),
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await.context("commit tx")?;
        Ok(())
    }

    pub async fn reject_discovery_candidate(&self, candidate_id: i64) -> Result<bool> {
        let mut tx = self.pool.begin().await.context("begin tx")?;
        let res = sqlx::query(
            "UPDATE discovery_candidate SET status = 'rejected', decided_at = now() WHERE id = $1",
        )
        .bind(candidate_id)
        .execute(&mut *tx)
        .await
        .context("reject_discovery_candidate")?;
        // Dismiss the matching attention item.
        sqlx::query(
            r#"WITH matched AS (
                    SELECT id, fsm_state
                      FROM attention_item
                     WHERE status = 'open'
                       AND kind = 'candidate_review'
                       AND candidate_id = $1
                     FOR UPDATE
                 ),
                 updated AS (
                    UPDATE attention_item ai
                       SET status = 'dismissed',
                           fsm_state = 'dismissed',
                           owner = 'operator',
                           resolved_at = now(),
                           resolution_kind = 'candidate_rejected',
                           resolution_ref = jsonb_build_object('reason', 'candidate_rejected'),
                           next_retry_at = NULL,
                           resurface_at = NULL,
                           state_reason = 'candidate_rejected'
                      FROM matched m
                     WHERE ai.id = m.id
                 RETURNING ai.id,
                           m.fsm_state AS from_state,
                           ai.fsm_state AS to_state,
                           ai.owner,
                           ai.state_reason,
                           ai.next_retry_at,
                           ai.resurface_at,
                           ai.resolution_ref
                 ),
                 inserted AS (
                    INSERT INTO attention_state_history
                         (attention_id, from_state, to_state, owner, reason,
                          next_retry_at, resurface_at, source_ref)
                    SELECT id, from_state, to_state, owner, state_reason,
                           next_retry_at, resurface_at, resolution_ref
                      FROM updated
                 RETURNING 1
                 )
              SELECT COUNT(*) FROM updated"#,
        )
        .bind(candidate_id)
        .execute(&mut *tx)
        .await?;
        tx.commit()
            .await
            .context("commit reject_discovery_candidate")?;
        Ok(res.rows_affected() > 0)
    }

    /// All watchlists with member counts (#54). Most-recent first; system
    /// lists rendered with a chip in the UI.
    pub async fn list_watchlists(&self) -> Result<Vec<Watchlist>> {
        let rows = sqlx::query(
            r#"SELECT w.id, w.name, w.description, w.color, w.is_system, w.created_at,
                      COUNT(m.symbol) AS member_count
                 FROM watchlist w
                 LEFT JOIN watchlist_member m ON m.watchlist_id = w.id
             GROUP BY w.id
             ORDER BY w.is_system DESC, w.name ASC"#,
        )
        .fetch_all(&self.pool)
        .await
        .context("list_watchlists")?;
        rows.into_iter()
            .map(|r| {
                Ok(Watchlist {
                    id: r.try_get("id")?,
                    name: r.try_get("name")?,
                    description: r.try_get("description").ok(),
                    color: r.try_get("color").ok(),
                    is_system: r.try_get("is_system")?,
                    created_at: r.try_get("created_at")?,
                    member_count: r.try_get::<i64, _>("member_count").unwrap_or(0),
                })
            })
            .collect()
    }

    /// Members of one watchlist (UI loads on click).
    pub async fn list_watchlist_members(&self, id: uuid::Uuid) -> Result<Vec<WatchlistMember>> {
        let rows = sqlx::query(
            r#"SELECT wm.watchlist_id,
                      wm.symbol,
                      wm.added_at,
                      wm.added_by,
                      latest.thesis_id AS latest_thesis_id,
                      latest.state AS thesis_state,
                      latest.direction AS thesis_direction,
                      (SELECT count(*) FROM thesis th
                        WHERE th.symbol = wm.symbol
                          AND th.state NOT IN ('closed','disqualified')) AS open_theses
                 FROM watchlist_member wm
            LEFT JOIN LATERAL (
                SELECT th.thesis_id, th.state, th.forecast->>'direction' AS direction
                  FROM thesis th
                 WHERE th.symbol = wm.symbol
                   AND th.state NOT IN ('closed','disqualified')
              ORDER BY th.updated_at DESC
                 LIMIT 1
            ) latest ON TRUE
                WHERE wm.watchlist_id = $1
             ORDER BY wm.added_at DESC"#,
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await
        .context("list_watchlist_members")?;
        rows.into_iter()
            .map(|r| {
                Ok(WatchlistMember {
                    watchlist_id: r.try_get("watchlist_id")?,
                    symbol: r.try_get("symbol")?,
                    added_at: r.try_get("added_at")?,
                    added_by: r.try_get("added_by").ok(),
                    latest_thesis_id: r.try_get("latest_thesis_id").ok(),
                    thesis_state: r.try_get("thesis_state").ok(),
                    thesis_direction: r.try_get("thesis_direction").ok(),
                    open_theses: r.try_get::<i64, _>("open_theses").unwrap_or(0),
                })
            })
            .collect()
    }

    pub async fn create_watchlist(
        &self,
        name: &str,
        description: Option<&str>,
        color: Option<&str>,
    ) -> Result<uuid::Uuid> {
        let row = sqlx::query(
            r#"INSERT INTO watchlist (name, description, color, is_system)
               VALUES ($1, $2, $3, false)
               RETURNING id"#,
        )
        .bind(name)
        .bind(description)
        .bind(color)
        .fetch_one(&self.pool)
        .await
        .context("create_watchlist")?;
        Ok(row.try_get("id")?)
    }

    /// Adds symbol to watchlist. Idempotent on (watchlist_id, symbol) PK;
    /// inserts the ticker row if it doesn't exist (default tier=2 — watch-only).
    pub async fn add_to_watchlist(
        &self,
        watchlist_id: uuid::Uuid,
        symbol: &str,
        added_by: &str,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.context("begin tx")?;
        // Ensure ticker exists; default tier=2 (watch-only) for fresh adds.
        sqlx::query("INSERT INTO ticker (symbol, tier) VALUES ($1, 2) ON CONFLICT DO NOTHING")
            .bind(symbol)
            .execute(&mut *tx)
            .await
            .context("ensure ticker row")?;
        sqlx::query(
            r#"INSERT INTO watchlist_member (watchlist_id, symbol, added_by)
               VALUES ($1, $2, $3) ON CONFLICT DO NOTHING"#,
        )
        .bind(watchlist_id)
        .bind(symbol)
        .bind(added_by)
        .execute(&mut *tx)
        .await
        .context("add_to_watchlist")?;
        tx.commit().await.context("commit tx")?;
        Ok(())
    }

    pub async fn remove_from_watchlist(
        &self,
        watchlist_id: uuid::Uuid,
        symbol: &str,
    ) -> Result<bool> {
        let res =
            sqlx::query("DELETE FROM watchlist_member WHERE watchlist_id = $1 AND symbol = $2")
                .bind(watchlist_id)
                .bind(symbol)
                .execute(&self.pool)
                .await
                .context("remove_from_watchlist")?;
        Ok(res.rows_affected() > 0)
    }

    /// Delete a watchlist + its memberships. Refuses to drop system lists.
    pub async fn delete_watchlist(&self, id: uuid::Uuid) -> Result<bool> {
        let res = sqlx::query("DELETE FROM watchlist WHERE id = $1 AND is_system = false")
            .bind(id)
            .execute(&self.pool)
            .await
            .context("delete_watchlist")?;
        Ok(res.rows_affected() > 0)
    }

    /// Upsert a batch of price bars (#17). Primary key (symbol, ts) handles
    /// dedup; same-day re-polls overwrite (a later intraday bar replaces an
    /// earlier one with the same date).
    pub async fn upsert_price_bars(
        &self,
        rows: &[crate::ingest::massive::PriceBarRow],
    ) -> Result<usize> {
        if rows.is_empty() {
            return Ok(0);
        }
        let mut inserted = 0;
        let mut tx = self.pool.begin().await.context("begin tx")?;
        for r in rows {
            let res = sqlx::query(
                r#"INSERT INTO price_bar (symbol, ts, open, high, low, close, volume)
                   VALUES ($1, $2, $3, $4, $5, $6, $7)
                   ON CONFLICT (symbol, ts) DO UPDATE SET
                     open   = EXCLUDED.open,
                     high   = EXCLUDED.high,
                     low    = EXCLUDED.low,
                     close  = EXCLUDED.close,
                     volume = EXCLUDED.volume"#,
            )
            .bind(&r.symbol)
            .bind(r.ts)
            .bind(r.open)
            .bind(r.high)
            .bind(r.low)
            .bind(r.close)
            .bind(r.volume)
            .execute(&mut *tx)
            .await
            .context("upsert_price_bars")?;
            if res.rows_affected() > 0 {
                inserted += 1;
            }
        }
        tx.commit().await.context("commit tx")?;
        Ok(inserted)
    }

    pub async fn upsert_intraday_price_bars(
        &self,
        rows: &[crate::ingest::fmp_intraday::IntradayPriceBarRow],
    ) -> Result<usize> {
        if rows.is_empty() {
            return Ok(0);
        }
        let mut inserted = 0;
        let mut tx = self.pool.begin().await.context("begin tx")?;
        for r in rows {
            let res = sqlx::query(
                r#"INSERT INTO price_bar_intraday
                     (symbol, interval, ts, open, high, low, close, volume, source, fetched_at)
                   VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'fmp', now())
                   ON CONFLICT (symbol, interval, ts) DO UPDATE SET
                     open       = EXCLUDED.open,
                     high       = EXCLUDED.high,
                     low        = EXCLUDED.low,
                     close      = EXCLUDED.close,
                     volume     = EXCLUDED.volume,
                     fetched_at = now()"#,
            )
            .bind(&r.symbol)
            .bind(&r.interval)
            .bind(r.ts)
            .bind(r.open)
            .bind(r.high)
            .bind(r.low)
            .bind(r.close)
            .bind(r.volume)
            .execute(&mut *tx)
            .await
            .context("upsert_intraday_price_bars")?;
            if res.rows_affected() > 0 {
                inserted += 1;
            }
        }
        tx.commit().await.context("commit tx")?;
        Ok(inserted)
    }

    /// Upsert a batch of XBRL facts. Idempotent via the unique index on
    /// (symbol, taxonomy, concept, period_end, accession). Returns number
    /// of rows actually inserted (vs already-present).
    pub async fn upsert_company_facts(
        &self,
        rows: &[crate::ingest::xbrl::FactRow],
    ) -> Result<usize> {
        if rows.is_empty() {
            return Ok(0);
        }
        let mut inserted = 0;
        let mut tx = self.pool.begin().await.context("begin tx")?;
        for r in rows {
            let res = sqlx::query(
                r#"INSERT INTO company_fact
                     (symbol, cik, taxonomy, concept, period_end, period_start,
                      value, unit, form, fiscal_year, fiscal_period,
                      accession, filed_at)
                   VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
                   ON CONFLICT DO NOTHING"#,
            )
            .bind(&r.symbol)
            .bind(&r.cik)
            .bind(&r.taxonomy)
            .bind(&r.concept)
            .bind(r.period_end)
            .bind(r.period_start)
            .bind(r.value)
            .bind(&r.unit)
            .bind(r.form.as_deref())
            .bind(r.fiscal_year)
            .bind(r.fiscal_period.as_deref())
            .bind(r.accession.as_deref())
            .bind(r.filed_at)
            .execute(&mut *tx)
            .await
            .context("upsert_company_facts")?;
            if res.rows_affected() > 0 {
                inserted += 1;
            }
        }
        tx.commit().await.context("commit tx")?;
        Ok(inserted)
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

/// Decode one `alert` row into [`Alert`]. Shared by `recent_alerts` and
/// `recent_alerts_filtered`.
fn decode_alert(row: sqlx::postgres::PgRow) -> Result<Alert> {
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
