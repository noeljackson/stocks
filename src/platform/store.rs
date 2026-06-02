//! Postgres access layer (sqlx pool + typed helpers).
//!
//! sqlx::query (not query!) — we keep the macro discipline off for v0 because
//! compile-time SQL checking requires a live DB at build time (DATABASE_URL
//! must be reachable). We can flip to the macro form later by setting
//! SQLX_OFFLINE=true + checking in the sqlx-data.json.

use anyhow::{Context, Result};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use sqlx::{
    Row,
    postgres::{PgPool, PgPoolOptions},
};
use std::time::Duration;

use crate::llm::prompts::{InvocationRecorder, InvocationRow};
use crate::platform::domain::{
    Alert, AlertKind, Condition, MarketStateRow, ThesisDetail, ThesisFreshnessComponent,
    ThesisSubstance, ThesisVersionEvent, TickerContextRow, TickerRow, Watchlist, WatchlistMember,
    WellFormedCondCounts,
};
use crate::platform::technical::TechnicalBar;
use crate::thesis::substance::{self, Thesis as SubstanceInput};

#[derive(Clone)]
pub struct Store {
    pub pool: PgPool,
}

#[derive(Debug, Clone, Copy)]
pub struct IntradayBarCoverage {
    pub oldest: Option<DateTime<Utc>>,
    pub latest: Option<DateTime<Utc>>,
    pub bars: i64,
}

#[derive(Debug, Clone)]
struct ThesisFreshnessSummary {
    score: f64,
    status: String,
    confidence_cap: Option<String>,
    penalties: Vec<String>,
    components: Vec<ThesisFreshnessComponent>,
}

#[derive(Debug, Clone, Copy)]
struct FreshnessThresholds {
    fresh: ChronoDuration,
    stale: ChronoDuration,
    old: ChronoDuration,
}

fn age_component(
    name: &str,
    now: DateTime<Utc>,
    last_at: Option<DateTime<Utc>>,
    thresholds: FreshnessThresholds,
    penalty: &str,
) -> (ThesisFreshnessComponent, Option<String>) {
    let Some(last_at) = last_at else {
        return (
            ThesisFreshnessComponent {
                name: name.to_string(),
                status: "missing".to_string(),
                score: 0.3,
                last_at: None,
                reason: format!("{name} has no observed timestamp"),
            },
            Some(format!("{name}: missing")),
        );
    };
    let age = now
        .signed_duration_since(last_at)
        .max(ChronoDuration::zero());
    let (status, score, reason, component_penalty) = if age <= thresholds.fresh {
        (
            "fresh",
            1.0,
            format!("{name} checked within freshness target"),
            None,
        )
    } else if age <= thresholds.stale {
        (
            "aging",
            0.8,
            format!("{name} is outside ideal freshness"),
            None,
        )
    } else if age <= thresholds.old {
        (
            "stale",
            0.6,
            format!("{name} is stale"),
            Some(format!("{name}: {penalty}")),
        )
    } else {
        (
            "old",
            0.4,
            format!("{name} is too old for high-confidence promotion"),
            Some(format!("{name}: {penalty}")),
        )
    };
    (
        ThesisFreshnessComponent {
            name: name.to_string(),
            status: status.to_string(),
            score,
            last_at: Some(last_at),
            reason,
        },
        component_penalty,
    )
}

fn news_component(
    recent_news_14d: i64,
    last_at: Option<DateTime<Utc>>,
) -> (ThesisFreshnessComponent, Option<String>) {
    let (status, score, reason, penalty) = if recent_news_14d >= 3 {
        (
            "fresh",
            1.0,
            format!("{recent_news_14d} recent articles in the last 14 days"),
            None,
        )
    } else if recent_news_14d > 0 {
        (
            "thin",
            0.7,
            format!("only {recent_news_14d} recent article(s) in the last 14 days"),
            Some("news: narrative evidence is thin".to_string()),
        )
    } else {
        (
            "missing",
            0.5,
            "no recent articles in the last 14 days".to_string(),
            Some("news: cannot rely on sentiment-shift evidence".to_string()),
        )
    };
    (
        ThesisFreshnessComponent {
            name: "news".to_string(),
            status: status.to_string(),
            score,
            last_at,
            reason,
        },
        penalty,
    )
}

fn freshness_status(score: f64) -> String {
    if score >= 0.85 {
        "fresh".to_string()
    } else if score >= 0.50 {
        "stale".to_string()
    } else {
        "limited".to_string()
    }
}

fn confidence_cap(score: f64, components: &[ThesisFreshnessComponent]) -> Option<String> {
    if score < 0.50 {
        return Some("low".to_string());
    }
    if score < 0.85
        || components
            .iter()
            .any(|c| c.name == "context" && matches!(c.status.as_str(), "stale" | "old"))
    {
        return Some("medium".to_string());
    }
    None
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

    /// Union of active tickers + active discovery pool members. This is broad
    /// discovery scope; expensive source loops should prefer
    /// `priority_scan_symbols` so the brain refreshes active names first.
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

    /// Tiered deep-research universe. Active tickers come first, then the
    /// highest-ranked proposed discovery candidates. This keeps expensive
    /// provider loops inside the freshness SLA instead of re-deep-scanning
    /// the whole screener pool every pass.
    pub async fn priority_scan_symbols(&self, limit: i64) -> Result<Vec<String>> {
        let rows: Vec<(String,)> = sqlx::query_as(
            r#"WITH ranked AS (
                  SELECT target_id AS symbol,
                         -1 AS source_rank,
                         CASE priority
                           WHEN 'blocking' THEN 0
                           WHEN 'high' THEN 1
                           WHEN 'medium' THEN 2
                           ELSE 3
                         END AS tier_rank,
                         100.0 AS fit_rank,
                         due_at AS last_ranked_at
                    FROM source_task
                   WHERE scope = 'symbol'
                     AND target_id <> ''
                     AND (
                         (
                             state IN ('queued', 'no_rows', 'failed', 'rate_limited', 'satisfied')
                             AND due_at <= now()
                         )
                         OR (
                             state = 'fetching'
                             AND updated_at < now() - interval '15 minutes'
                         )
                     )
                  UNION ALL
                  SELECT symbol,
                         0 AS source_rank,
                         tier AS tier_rank,
                         COALESCE(domain_fit::double precision, 0.0) AS fit_rank,
                         added_at AS last_ranked_at
                    FROM ticker
                   WHERE status = 'active'
                  UNION ALL
                  SELECT symbol,
                         1 AS source_rank,
                         COALESCE(proposed_tier, 3) AS tier_rank,
                         COALESCE(domain_fit, 0.0) AS fit_rank,
                         proposed_at AS last_ranked_at
                    FROM discovery_candidate
                   WHERE status = 'proposed'
                     AND COALESCE(proposed_tier, 3) <= 2
               )
               SELECT symbol
                 FROM ranked
             GROUP BY symbol
             ORDER BY
                  MIN(source_rank),
                  MIN(tier_rank),
                  MAX(fit_rank) DESC,
                  MAX(last_ranked_at) DESC,
                  symbol
                LIMIT $1"#,
        )
        .bind(limit.max(1))
        .fetch_all(&self.pool)
        .await
        .context("priority_scan_symbols")?;
        Ok(rows.into_iter().map(|r| r.0).collect())
    }

    pub async fn mark_source_tasks_fetching(
        &self,
        actions: &[&str],
        symbols: &[String],
        owner: &str,
    ) -> Result<u64> {
        self.mark_source_tasks_fetching_for_scope("symbol", actions, symbols, owner)
            .await
    }

    pub async fn mark_source_tasks_fetching_for_scope(
        &self,
        scope: &str,
        actions: &[&str],
        target_ids: &[String],
        owner: &str,
    ) -> Result<u64> {
        if actions.is_empty() || target_ids.is_empty() {
            return Ok(0);
        }
        let actions: Vec<String> = actions.iter().map(|a| (*a).to_string()).collect();
        let res = sqlx::query(
            r#"UPDATE source_task
                  SET state = 'fetching',
                      attempts = attempts + 1,
                      last_error = NULL,
                      updated_at = now(),
                      source_ref = source_ref || jsonb_build_object(
                          'claimed_by', $3,
                          'claimed_at', now()
                      )
                WHERE scope = $4
                  AND target_id = ANY($1::text[])
                  AND action = ANY($2::text[])
                  AND (
                      (
                          state IN ('queued', 'no_rows', 'failed', 'rate_limited', 'satisfied')
                          AND due_at <= now()
                      )
                      OR (
                          state = 'fetching'
                          AND updated_at < now() - interval '15 minutes'
                      )
                  )"#,
        )
        .bind(target_ids)
        .bind(&actions)
        .bind(owner)
        .bind(scope)
        .execute(&self.pool)
        .await
        .context("mark_source_tasks_fetching")?;
        Ok(res.rows_affected())
    }

    pub async fn complete_source_tasks_for_attempt(
        &self,
        action: &str,
        attempted_symbols: &[String],
        symbols_with_rows: &[String],
        owner: &str,
        fresh_for: ChronoDuration,
    ) -> Result<u64> {
        self.complete_source_tasks_for_scope(
            "symbol",
            action,
            attempted_symbols,
            symbols_with_rows,
            owner,
            fresh_for,
        )
        .await
    }

    pub async fn complete_source_tasks_for_scope(
        &self,
        scope: &str,
        action: &str,
        attempted_targets: &[String],
        targets_with_rows: &[String],
        owner: &str,
        fresh_for: ChronoDuration,
    ) -> Result<u64> {
        if attempted_targets.is_empty() {
            return Ok(0);
        }
        let fresh_until = Utc::now() + fresh_for;
        let retry_at = Utc::now() + ChronoDuration::minutes(30);
        let res = sqlx::query(
            r#"UPDATE source_task
                  SET state = CASE
                          WHEN target_id = ANY($3::text[]) THEN 'satisfied'
                          ELSE 'no_rows'
                      END,
                      due_at = CASE
                          WHEN target_id = ANY($3::text[]) THEN $5
                          ELSE $6
                      END,
                      next_retry_at = NULL,
                      last_error = NULL,
                      updated_at = now(),
                      source_ref = source_ref || jsonb_build_object(
                          'completed_by', $4,
                          'completed_at', now(),
                          'result', CASE
                              WHEN target_id = ANY($3::text[]) THEN 'rows_seen'
                              ELSE 'no_rows'
                          END
                      )
                WHERE scope = $7
                  AND target_id = ANY($1::text[])
                  AND action = $2"#,
        )
        .bind(attempted_targets)
        .bind(action)
        .bind(targets_with_rows)
        .bind(owner)
        .bind(fresh_until)
        .bind(retry_at)
        .bind(scope)
        .execute(&self.pool)
        .await
        .context("complete_source_tasks_for_attempt")?;
        Ok(res.rows_affected())
    }

    pub async fn fail_source_tasks_for_attempt(
        &self,
        action: &str,
        attempted_symbols: &[String],
        owner: &str,
        state: &str,
        error: &str,
        retry_after_at: Option<DateTime<Utc>>,
    ) -> Result<u64> {
        self.fail_source_tasks_for_scope(
            "symbol",
            action,
            attempted_symbols,
            owner,
            state,
            error,
            retry_after_at,
        )
        .await
    }

    pub async fn fail_source_tasks_for_scope(
        &self,
        scope: &str,
        action: &str,
        attempted_targets: &[String],
        owner: &str,
        state: &str,
        error: &str,
        retry_after_at: Option<DateTime<Utc>>,
    ) -> Result<u64> {
        if attempted_targets.is_empty() {
            return Ok(0);
        }
        let task_state = if state == "rate_limited" {
            "rate_limited"
        } else {
            "failed"
        };
        let retry_at = retry_after_at.unwrap_or_else(|| Utc::now() + ChronoDuration::minutes(30));
        let res = sqlx::query(
            r#"UPDATE source_task
                  SET state = $3,
                      due_at = $6,
                      next_retry_at = $6,
                      last_error = $5,
                      updated_at = now(),
                      source_ref = source_ref || jsonb_build_object(
                          'failed_by', $4,
                          'failed_at', now()
                      )
                WHERE scope = $7
                  AND target_id = ANY($1::text[])
                  AND action = $2"#,
        )
        .bind(attempted_targets)
        .bind(action)
        .bind(task_state)
        .bind(owner)
        .bind(error.chars().take(500).collect::<String>())
        .bind(retry_at)
        .bind(scope)
        .execute(&self.pool)
        .await
        .context("fail_source_tasks_for_attempt")?;
        Ok(res.rows_affected())
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

    pub async fn transition_attention(
        &self,
        id: i64,
        to_state: &str,
        owner: &str,
        reason: &str,
        next_retry_at: Option<DateTime<Utc>>,
        resurface_at: Option<DateTime<Utc>>,
        source_ref: serde_json::Value,
    ) -> Result<bool> {
        let status = crate::attention::status_for_state(to_state);
        let rows: i64 = sqlx::query_scalar(
            r#"WITH matched AS (
                    SELECT id, fsm_state
                      FROM attention_item
                     WHERE id = $1 AND status = 'open'
                     FOR UPDATE
                 ),
                 updated AS (
                    UPDATE attention_item ai
                       SET status = $2,
                           fsm_state = $3,
                           owner = $4,
                           resolved_at = CASE WHEN $2 <> 'open' THEN now() ELSE NULL END,
                           resolution_kind = CASE WHEN $2 <> 'open' THEN $5 ELSE NULL END,
                           resolution_ref = CASE WHEN $2 <> 'open' THEN $8::jsonb ELSE resolution_ref END,
                           next_retry_at = $6,
                           resurface_at = $7,
                           state_reason = $5,
                           source_ref = ai.source_ref || $8::jsonb
                      FROM matched m
                     WHERE ai.id = m.id
                 RETURNING ai.id,
                           m.fsm_state AS from_state,
                           ai.fsm_state AS to_state,
                           ai.owner,
                           ai.state_reason,
                           ai.next_retry_at,
                           ai.resurface_at,
                           $8::jsonb AS transition_ref
                 ),
                 inserted AS (
                    INSERT INTO attention_state_history
                         (attention_id, from_state, to_state, owner, reason,
                          next_retry_at, resurface_at, source_ref)
                    SELECT id, from_state, to_state, owner, state_reason,
                           next_retry_at, resurface_at, transition_ref
                      FROM updated
                 RETURNING 1
                 )
              SELECT COUNT(*) FROM updated"#,
        )
        .bind(id)
        .bind(status)
        .bind(to_state)
        .bind(owner)
        .bind(reason)
        .bind(next_retry_at)
        .bind(resurface_at)
        .bind(source_ref)
        .fetch_one(&self.pool)
        .await
        .context("transition_attention")?;
        Ok(rows > 0)
    }

    async fn resurface_due_attention(&self) -> Result<u64> {
        let rows: i64 = sqlx::query_scalar(
            r#"WITH matched AS (
                    SELECT id, fsm_state
                      FROM attention_item
                     WHERE status = 'open'
                       AND fsm_state = 'operator_deferred'
                       AND resurface_at IS NOT NULL
                       AND resurface_at <= now()
                     FOR UPDATE
                 ),
                 updated AS (
                    UPDATE attention_item ai
                       SET fsm_state = 'ready_for_review',
                           owner = 'operator',
                           resolution_kind = NULL,
                           resolution_ref = NULL,
                           resurface_at = NULL,
                           state_reason = 'resurfaced'
                      FROM matched m
                     WHERE ai.id = m.id
                 RETURNING ai.id,
                           m.fsm_state AS from_state,
                           ai.fsm_state AS to_state,
                           ai.owner,
                           ai.state_reason,
                           ai.next_retry_at,
                           ai.resurface_at,
                           jsonb_build_object('reason', 'resurfaced') AS transition_ref
                 ),
                 inserted AS (
                    INSERT INTO attention_state_history
                         (attention_id, from_state, to_state, owner, reason,
                          next_retry_at, resurface_at, source_ref)
                    SELECT id, from_state, to_state, owner, state_reason,
                           next_retry_at, resurface_at, transition_ref
                      FROM updated
                 RETURNING 1
                 )
              SELECT COUNT(*) FROM updated"#,
        )
        .fetch_one(&self.pool)
        .await
        .context("resurface_due_attention")?;
        Ok(rows as u64)
    }

    /// Open attention items, severity-then-recency ordering.
    pub async fn list_attention(&self, status: &str, limit: i64) -> Result<Vec<serde_json::Value>> {
        if status == "open" {
            self.resurface_due_attention().await?;
        }
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
                      source_ref, created_at, updated_at, satisfied_at,
                      COALESCE((
                          SELECT jsonb_agg(
                              jsonb_build_object(
                                  'id', st.id,
                                  'action', st.action,
                                  'provider', st.provider,
                                  'state', st.state,
                                  'priority', st.priority,
                                  'due_at', st.due_at,
                                  'next_retry_at', st.next_retry_at,
                                  'attempts', st.attempts,
                                  'last_error', st.last_error,
                                  'updated_at', st.updated_at
                              )
                              ORDER BY
                                  CASE st.state
                                       WHEN 'queued' THEN 0
                                       WHEN 'rate_limited' THEN 1
                                       WHEN 'failed' THEN 2
                                       WHEN 'no_rows' THEN 3
                                       WHEN 'fetching' THEN 4
                                       ELSE 5
                                  END,
                                  st.due_at
                          )
                            FROM source_task st
                           WHERE st.scope = 'symbol'
                             AND st.target_id = evidence_requirement.symbol
                             AND st.requirement_key = evidence_requirement.requirement_key
                      ), '[]'::jsonb) AS source_tasks
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
                    "source_tasks": r.try_get::<serde_json::Value, _>("source_tasks")?,
                    "created_at": created_at,
                    "updated_at": updated_at,
                    "satisfied_at": satisfied_at,
                }))
            })
            .collect()
    }

    pub async fn research_evidence_for_symbol(
        &self,
        symbol: &str,
    ) -> Result<Vec<serde_json::Value>> {
        let rows = sqlx::query(
            r#"WITH ranked AS (
                  SELECT DISTINCT ON (lower(title), COALESCE(published_at, retrieved_at))
                         id, symbol, query, url, title, publisher, published_at,
                         retrieved_at, provider, source_type, credibility, summary, tags
                    FROM research_evidence
                   WHERE symbol = $1
                ORDER BY lower(title),
                         COALESCE(published_at, retrieved_at),
                         (url LIKE 'http://www.bing.com/%') ASC,
                         retrieved_at DESC
              )
              SELECT *
                FROM ranked
            ORDER BY credibility = 'primary' DESC,
                     published_at DESC NULLS LAST,
                     retrieved_at DESC
               LIMIT 50"#,
        )
        .bind(symbol)
        .fetch_all(&self.pool)
        .await
        .context("research_evidence_for_symbol")?;
        rows.into_iter()
            .map(|r| {
                let published_at: Option<DateTime<Utc>> = r.try_get("published_at")?;
                let retrieved_at: DateTime<Utc> = r.try_get("retrieved_at")?;
                Ok(serde_json::json!({
                    "id": r.try_get::<i64, _>("id")?,
                    "symbol": r.try_get::<String, _>("symbol")?,
                    "query": r.try_get::<String, _>("query")?,
                    "url": r.try_get::<String, _>("url")?,
                    "title": r.try_get::<String, _>("title")?,
                    "publisher": r.try_get::<Option<String>, _>("publisher")?,
                    "published_at": published_at,
                    "retrieved_at": retrieved_at,
                    "provider": r.try_get::<String, _>("provider")?,
                    "source_type": r.try_get::<String, _>("source_type")?,
                    "credibility": r.try_get::<String, _>("credibility")?,
                    "summary": r.try_get::<Option<String>, _>("summary")?,
                    "tags": r.try_get::<Vec<String>, _>("tags")?,
                }))
            })
            .collect()
    }

    pub async fn evidence_items_for_symbol(&self, symbol: &str) -> Result<Vec<serde_json::Value>> {
        let rows = sqlx::query(
            r#"SELECT id, symbol, kind, observed_at, source, source_id,
                      source_ref, summary, strength, polarity, url, created_at, updated_at
                 FROM evidence_item
                WHERE symbol = $1
             ORDER BY observed_at DESC, id DESC
                LIMIT 100"#,
        )
        .bind(symbol)
        .fetch_all(&self.pool)
        .await
        .context("evidence_items_for_symbol")?;
        rows.into_iter()
            .map(|r| {
                let observed_at: DateTime<Utc> = r.try_get("observed_at")?;
                let created_at: DateTime<Utc> = r.try_get("created_at")?;
                let updated_at: DateTime<Utc> = r.try_get("updated_at")?;
                Ok(serde_json::json!({
                    "id": r.try_get::<i64, _>("id")?,
                    "symbol": r.try_get::<String, _>("symbol")?,
                    "kind": r.try_get::<String, _>("kind")?,
                    "observed_at": observed_at,
                    "source": r.try_get::<String, _>("source")?,
                    "source_id": r.try_get::<String, _>("source_id")?,
                    "source_ref": r.try_get::<serde_json::Value, _>("source_ref")?,
                    "summary": r.try_get::<String, _>("summary")?,
                    "strength": r.try_get::<Option<f64>, _>("strength")?,
                    "polarity": r.try_get::<Option<f64>, _>("polarity")?,
                    "url": r.try_get::<Option<String>, _>("url")?,
                    "created_at": created_at,
                    "updated_at": updated_at,
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
                      COALESCE(d.sizing->>'instrument', t.instrument) AS instrument,
                      dr.decision_id IS NOT NULL AS has_replay
                 FROM decision d
                 JOIN thesis t USING (thesis_id)
            LEFT JOIN decision_replay dr ON dr.decision_id = d.decision_id
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
                    "has_replay": r.try_get::<bool, _>("has_replay").unwrap_or(false),
                    "at": at,
                }))
            })
            .collect()
    }

    pub async fn decision_replay(
        &self,
        decision_id: uuid::Uuid,
    ) -> Result<Option<serde_json::Value>> {
        let row = sqlx::query(
            r#"SELECT dr.decision_id, dr.symbol, dr.thesis_id, dr.context_version,
                      dr.thesis_snapshot, dr.consensus_score, dr.risk_verdict,
                      dr.evidence_ids, dr.evidence_snapshot, dr.system_confidence,
                      dr.chart_range_seen, dr.captured_at,
                      to_jsonb(d) AS decision_snapshot
                 FROM decision_replay dr
                 JOIN decision d ON d.decision_id = dr.decision_id
                WHERE dr.decision_id = $1"#,
        )
        .bind(decision_id)
        .fetch_optional(&self.pool)
        .await
        .context("decision_replay")?;
        let Some(r) = row else {
            return Ok(None);
        };
        let captured_at: DateTime<Utc> = r.try_get("captured_at")?;
        let evidence_ids: Vec<i64> = r.try_get("evidence_ids").unwrap_or_default();
        Ok(Some(serde_json::json!({
            "decision_id": r.try_get::<uuid::Uuid, _>("decision_id")?,
            "symbol": r.try_get::<String, _>("symbol")?,
            "thesis_id": r.try_get::<Option<uuid::Uuid>, _>("thesis_id")?,
            "context_version": r.try_get::<Option<i32>, _>("context_version")?,
            "thesis_snapshot": r.try_get::<serde_json::Value, _>("thesis_snapshot")?,
            "consensus_score": r.try_get::<Option<f64>, _>("consensus_score")?,
            "risk_verdict": r.try_get::<serde_json::Value, _>("risk_verdict")?,
            "evidence_ids": evidence_ids,
            "evidence_snapshot": r.try_get::<serde_json::Value, _>("evidence_snapshot")?,
            "system_confidence": r.try_get::<Option<String>, _>("system_confidence")?,
            "chart_range_seen": r.try_get::<Option<String>, _>("chart_range_seen")?,
            "decision_snapshot": r.try_get::<serde_json::Value, _>("decision_snapshot")?,
            "captured_at": captured_at,
        })))
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

    pub async fn intraday_bar_coverage(
        &self,
        symbol: &str,
        native_interval: &str,
    ) -> Result<IntradayBarCoverage> {
        let row = sqlx::query(
            r#"SELECT min(ts) AS oldest, max(ts) AS latest, count(*)::int8 AS bars
                 FROM price_bar_intraday
                WHERE symbol = $1 AND interval = $2"#,
        )
        .bind(symbol)
        .bind(native_interval)
        .fetch_one(&self.pool)
        .await
        .context("intraday_bar_coverage")?;
        Ok(IntradayBarCoverage {
            oldest: row.try_get("oldest")?,
            latest: row.try_get("latest")?,
            bars: row.try_get::<i64, _>("bars")?,
        })
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

    pub async fn daily_technical_bars_for(
        &self,
        symbol: &str,
        lookback_days: i64,
    ) -> Result<Vec<TechnicalBar>> {
        let rows = sqlx::query(
            r#"WITH daily AS (
                 SELECT (date_trunc('day', ts AT TIME ZONE 'UTC'))::date AS day,
                        (array_agg(ts ORDER BY ts DESC))[1] AS ts,
                        max(high::float8) AS high,
                        min(low::float8) AS low,
                        (array_agg(close::float8 ORDER BY ts DESC))[1] AS close
                   FROM price_bar
                  WHERE symbol = $1
                    AND ts > now() - ($2 || ' days')::interval
               GROUP BY 1
             )
             SELECT ts, close, high, low
               FROM daily
              ORDER BY day ASC"#,
        )
        .bind(symbol)
        .bind(lookback_days.to_string())
        .fetch_all(&self.pool)
        .await
        .context("daily_technical_bars_for")?;
        rows.into_iter()
            .map(|r| {
                Ok(TechnicalBar {
                    ts: r.try_get("ts")?,
                    close: r.try_get("close")?,
                    high: r.try_get("high")?,
                    low: r.try_get("low")?,
                })
            })
            .collect()
    }

    pub async fn intraday_technical_bars_for(
        &self,
        symbol: &str,
        native_interval: &str,
        lookback_days: i64,
        bucket_minutes: i64,
    ) -> Result<Vec<TechnicalBar>> {
        let rows = sqlx::query(
            r#"WITH bucketed AS (
                 SELECT to_timestamp(floor(extract(epoch FROM ts) / ($4::float8 * 60.0)) * ($4::float8 * 60.0)) AS bucket,
                        ts, close::float8 AS close, high::float8 AS high, low::float8 AS low
                   FROM price_bar_intraday
                  WHERE symbol = $1
                    AND interval = $2
                    AND ts > now() - ($3 || ' days')::interval
             )
             SELECT bucket,
                    (array_agg(close ORDER BY ts DESC))[1] AS close,
                    max(high) AS high,
                    min(low) AS low
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
        .context("intraday_technical_bars_for")?;
        rows.into_iter()
            .map(|r| {
                Ok(TechnicalBar {
                    ts: r.try_get("bucket")?,
                    close: r.try_get("close")?,
                    high: r.try_get("high")?,
                    low: r.try_get("low")?,
                })
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
                      tech.technical_state              AS technical_state,
                      tech.entry_stance                 AS entry_stance,
                      tech.pct_vs_200d                  AS technical_pct_vs_200d,
                      freshness.status                  AS freshness_status,
                      COALESCE(attention.open_count, 0) AS open_attention,
                      COALESCE(attention.states, '[]'::jsonb) AS attention_states,
                      COALESCE(attention.owners, '[]'::jsonb) AS attention_owners,
                      COALESCE(evidence.open_count, 0) AS open_evidence,
                      COALESCE(evidence.blocking_count, 0) AS blocking_evidence,
                      COALESCE(tasks.due_count, 0) AS due_source_tasks,
                      COALESCE(brain.parent_themes, '[]'::jsonb) AS parent_themes,
                      (SELECT count(*) FROM thesis th
                        WHERE th.symbol = t.symbol
                          AND th.state NOT IN ('closed','disqualified')) AS open_theses
                 FROM ticker t
            LEFT JOIN cluster c ON c.id = t.cluster_id
            LEFT JOIN LATERAL (
                SELECT th.thesis_id, th.state, th.forecast->>'direction' AS direction,
                       COALESCE(th.last_evaluated_at, th.updated_at) AS evaluated_at
                  FROM thesis th
                 WHERE th.symbol = t.symbol
                   AND th.state NOT IN ('closed','disqualified')
              ORDER BY th.updated_at DESC
                 LIMIT 1
            ) latest ON TRUE
            LEFT JOIN LATERAL (
                WITH bars AS (
                    SELECT ts, close::float8 AS close, high::float8 AS high
                      FROM price_bar
                     WHERE symbol = t.symbol
                  ORDER BY ts DESC
                     LIMIT 260
                ), ranked AS (
                    SELECT ts, close, high, row_number() OVER (ORDER BY ts DESC) AS rn
                      FROM bars
                ), latest_bar AS (
                    SELECT close
                      FROM ranked
                     WHERE rn = 1
                ), stats AS (
                    SELECT count(*) FILTER (WHERE rn <= 200) AS bars_200,
                           avg(close) FILTER (WHERE rn <= 50) AS sma50,
                           avg(close) FILTER (WHERE rn <= 200) AS sma200,
                           max(high) FILTER (WHERE rn <= 252) AS high252
                      FROM ranked
                ), classified AS (
                    SELECT CASE
                             WHEN stats.bars_200 < 200 OR stats.sma200 IS NULL THEN 'unknown'
                             WHEN ((latest_bar.close - stats.sma200) / NULLIF(stats.sma200, 0) * 100.0) > 20.0
                               OR ((latest_bar.close - stats.high252) / NULLIF(stats.high252, 0) * 100.0) >= -2.0 THEN 'extended'
                             WHEN ((latest_bar.close - stats.sma200) / NULLIF(stats.sma200, 0) * 100.0) < -5.0 THEN 'deteriorating'
                             WHEN stats.sma50 IS NOT NULL
                               AND abs((latest_bar.close - stats.sma50) / NULLIF(stats.sma50, 0) * 100.0) <= 5.0 THEN 'base_building'
                             WHEN ((latest_bar.close - stats.sma200) / NULLIF(stats.sma200, 0) * 100.0) >= 0.0 THEN 'constructive'
                             ELSE 'unknown'
                           END AS technical_state,
                           ((latest_bar.close - stats.sma200) / NULLIF(stats.sma200, 0) * 100.0)::float8 AS pct_vs_200d
                      FROM latest_bar CROSS JOIN stats
                )
                SELECT technical_state,
                       CASE technical_state
                         WHEN 'extended' THEN 'avoid_chase'
                         WHEN 'deteriorating' THEN 'avoid'
                         WHEN 'base_building' THEN 'wait_breakout'
                         WHEN 'constructive' THEN 'constructive'
                         ELSE 'wait_data'
                       END AS entry_stance,
                       pct_vs_200d
                  FROM classified
            ) tech ON TRUE
            LEFT JOIN LATERAL (
                SELECT tc.created_at AS context_at
                  FROM ticker_context tc
                 WHERE tc.symbol = t.symbol
              ORDER BY tc.version DESC
                 LIMIT 1
            ) ctx ON TRUE
            LEFT JOIN LATERAL (
                SELECT count(*) AS rows_count,
                       count(*) FILTER (WHERE er.blocking_state <> 'satisfied') AS open_count,
                       count(*) FILTER (
                         WHERE er.priority = 'blocking'
                           AND er.blocking_state <> 'satisfied'
                       ) AS blocking_count
                  FROM evidence_requirement er
                 WHERE er.symbol = t.symbol
            ) evidence ON TRUE
            LEFT JOIN LATERAL (
                SELECT count(*) FILTER (
                         WHERE st.state IN ('queued', 'no_rows', 'failed', 'rate_limited', 'blocked')
                           AND st.due_at <= now()
                       ) AS due_count,
                       count(*) FILTER (
                         WHERE st.state = 'fetching'
                           AND st.updated_at < now() - interval '30 minutes'
                       ) AS stale_fetching_count,
                       count(*) FILTER (
                         WHERE st.state IN ('failed', 'rate_limited', 'blocked')
                       ) AS blocked_count
                  FROM source_task st
                 WHERE st.scope = 'symbol'
                   AND st.target_id = t.symbol
            ) tasks ON TRUE
            LEFT JOIN LATERAL (
                SELECT CASE
                         WHEN COALESCE(evidence.blocking_count, 0) > 0
                           OR COALESCE(tasks.blocked_count, 0) > 0 THEN 'blocked'
                         WHEN latest.thesis_id IS NULL
                           OR ctx.context_at IS NULL
                           OR COALESCE(evidence.rows_count, 0) = 0 THEN 'missing'
                         WHEN COALESCE(evidence.open_count, 0) > 0
                           OR COALESCE(tasks.due_count, 0) > 0
                           OR COALESCE(tasks.stale_fetching_count, 0) > 0
                           OR ctx.context_at < now() - interval '12 hours'
                           OR latest.evaluated_at IS NULL
                           OR latest.evaluated_at < now() - interval '30 minutes' THEN 'stale'
                         ELSE 'fresh'
                       END AS status
            ) freshness ON TRUE
            LEFT JOIN LATERAL (
                SELECT
                    (SELECT count(*)
                       FROM attention_item ai
                      WHERE ai.symbol = t.symbol
                        AND ai.status = 'open'
                        AND (ai.fsm_state <> 'operator_deferred'
                             OR (ai.resurface_at IS NOT NULL AND ai.resurface_at <= now()))) AS open_count,
                    COALESCE((
                        SELECT jsonb_agg(jsonb_build_object('state', s.fsm_state, 'count', s.n)
                                         ORDER BY s.n DESC, s.fsm_state)
                          FROM (
                              SELECT ai.fsm_state, count(*) AS n
                                FROM attention_item ai
                               WHERE ai.symbol = t.symbol
                                 AND ai.status = 'open'
                                 AND (ai.fsm_state <> 'operator_deferred'
                                      OR (ai.resurface_at IS NOT NULL AND ai.resurface_at <= now()))
                            GROUP BY ai.fsm_state
                          ) s
                    ), '[]'::jsonb) AS states,
                    COALESCE((
                        SELECT jsonb_agg(jsonb_build_object('owner', o.owner, 'count', o.n)
                                         ORDER BY o.n DESC, o.owner)
                          FROM (
                              SELECT ai.owner, count(*) AS n
                                FROM attention_item ai
                               WHERE ai.symbol = t.symbol
                                 AND ai.status = 'open'
                                 AND (ai.fsm_state <> 'operator_deferred'
                                      OR (ai.resurface_at IS NOT NULL AND ai.resurface_at <= now()))
                            GROUP BY ai.owner
                          ) o
                    ), '[]'::jsonb) AS owners
            ) attention ON TRUE
            LEFT JOIN LATERAL (
                SELECT COALESCE(jsonb_agg(
                         jsonb_build_object(
                           'key', bt.key,
                           'name', bt.name,
                           'scope', bt.scope,
                           'state', bt.state,
                           'direction', bt.direction,
                           'role', btt.role,
                           'conviction', btt.conviction
                         )
                         ORDER BY COALESCE(btt.conviction, 0) DESC, bt.name
                       ), '[]'::jsonb) AS parent_themes
                  FROM brain_thesis_ticker btt
                  JOIN brain_thesis bt ON bt.id = btt.brain_thesis_id
                 WHERE btt.symbol = t.symbol
                   AND bt.active = true
            ) brain ON TRUE
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
                    technical_state: row.try_get("technical_state").ok(),
                    entry_stance: row.try_get("entry_stance").ok(),
                    technical_pct_vs_200d: row.try_get("technical_pct_vs_200d").ok(),
                    freshness_status: row
                        .try_get("freshness_status")
                        .unwrap_or_else(|_| "missing".to_string()),
                    open_attention: row.try_get::<i64, _>("open_attention").unwrap_or(0),
                    attention_states: row
                        .try_get("attention_states")
                        .unwrap_or_else(|_| serde_json::json!([])),
                    attention_owners: row
                        .try_get("attention_owners")
                        .unwrap_or_else(|_| serde_json::json!([])),
                    open_evidence: row.try_get::<i64, _>("open_evidence").unwrap_or(0),
                    blocking_evidence: row.try_get::<i64, _>("blocking_evidence").unwrap_or(0),
                    due_source_tasks: row.try_get::<i64, _>("due_source_tasks").unwrap_or(0),
                    parent_themes: row
                        .try_get("parent_themes")
                        .unwrap_or_else(|_| serde_json::json!([])),
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
    async fn thesis_freshness_for_symbol(&self, symbol: &str) -> Result<ThesisFreshnessSummary> {
        let row = sqlx::query(
            r#"SELECT
                  (SELECT created_at
                     FROM ticker_context
                    WHERE symbol = $1
                 ORDER BY version DESC
                    LIMIT 1) AS context_at,
                  (SELECT max(snapshot_at)
                     FROM estimate_snapshot
                    WHERE symbol = $1) AS estimates_at,
                  (SELECT max(ingested_at)
                     FROM news_article
                    WHERE symbol = $1) AS news_at,
                  (SELECT count(*)
                     FROM news_article
                    WHERE symbol = $1
                      AND published_at >= now() - interval '14 days') AS recent_news_14d,
                  (SELECT max(COALESCE(last_success_at, last_started_at, updated_at))
                     FROM source_health
                    WHERE source IN ('fred', 'cboe')) AS market_at"#,
        )
        .bind(symbol)
        .fetch_one(&self.pool)
        .await
        .context("thesis freshness query")?;

        let now = Utc::now();
        let mut penalties = Vec::new();
        let mut components = Vec::new();

        let push = |components: &mut Vec<ThesisFreshnessComponent>,
                    penalties: &mut Vec<String>,
                    item: (ThesisFreshnessComponent, Option<String>)| {
            components.push(item.0);
            if let Some(penalty) = item.1 {
                penalties.push(penalty);
            }
        };

        push(
            &mut components,
            &mut penalties,
            age_component(
                "market",
                now,
                row.try_get("market_at").ok().flatten(),
                FreshnessThresholds {
                    fresh: ChronoDuration::hours(24),
                    stale: ChronoDuration::days(7),
                    old: ChronoDuration::days(30),
                },
                "market regime/crowd evidence is too old for high confidence",
            ),
        );
        push(
            &mut components,
            &mut penalties,
            age_component(
                "context",
                now,
                row.try_get("context_at").ok().flatten(),
                FreshnessThresholds {
                    fresh: ChronoDuration::days(7),
                    stale: ChronoDuration::days(30),
                    old: ChronoDuration::days(90),
                },
                "narrative context is stale",
            ),
        );
        push(
            &mut components,
            &mut penalties,
            age_component(
                "estimates",
                now,
                row.try_get("estimates_at").ok().flatten(),
                FreshnessThresholds {
                    fresh: ChronoDuration::days(14),
                    stale: ChronoDuration::days(60),
                    old: ChronoDuration::days(120),
                },
                "estimate-revision evidence is too old for actionable promotion",
            ),
        );
        push(
            &mut components,
            &mut penalties,
            news_component(
                row.try_get("recent_news_14d").unwrap_or(0),
                row.try_get("news_at").ok().flatten(),
            ),
        );

        let score = components
            .iter()
            .fold(1.0_f64, |acc, component| acc * component.score)
            .clamp(0.0, 1.0);
        let status = freshness_status(score);
        let confidence_cap = confidence_cap(score, &components);

        Ok(ThesisFreshnessSummary {
            score,
            status,
            confidence_cap,
            penalties,
            components,
        })
    }

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
                      created_at, updated_at, last_evaluated_at
                 FROM thesis
                WHERE symbol = $1
             ORDER BY updated_at DESC"#,
        )
        .bind(symbol)
        .fetch_all(&self.pool)
        .await
        .context("theses_for_symbol")?;

        let freshness = self.thesis_freshness_for_symbol(symbol).await?;
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

            let evidence_rows = sqlx::query(
                r#"SELECT ei.id, ei.symbol, ei.kind, ei.observed_at, ei.source,
                          ei.source_id, ei.source_ref, ei.summary, ei.strength,
                          ei.polarity, ei.url, ei.created_at,
                          te.weight, te.added_by
                     FROM thesis_evidence te
                     JOIN evidence_item ei ON ei.id = te.evidence_id
                    WHERE te.thesis_id = $1
                 ORDER BY te.weight DESC NULLS LAST, ei.observed_at DESC, ei.id DESC
                    LIMIT 25"#,
            )
            .bind(thesis_id)
            .fetch_all(&self.pool)
            .await
            .context("thesis_evidence")?;
            let evidence_items: Vec<serde_json::Value> = evidence_rows
                .into_iter()
                .map(|r| {
                    let observed_at: DateTime<Utc> = r.try_get("observed_at")?;
                    let created_at: DateTime<Utc> = r.try_get("created_at")?;
                    Ok(serde_json::json!({
                        "id": r.try_get::<i64, _>("id")?,
                        "symbol": r.try_get::<String, _>("symbol")?,
                        "kind": r.try_get::<String, _>("kind")?,
                        "observed_at": observed_at,
                        "source": r.try_get::<String, _>("source")?,
                        "source_id": r.try_get::<String, _>("source_id")?,
                        "source_ref": r.try_get::<serde_json::Value, _>("source_ref")?,
                        "summary": r.try_get::<String, _>("summary")?,
                        "strength": r.try_get::<Option<f64>, _>("strength")?,
                        "polarity": r.try_get::<Option<f64>, _>("polarity")?,
                        "url": r.try_get::<Option<String>, _>("url")?,
                        "created_at": created_at,
                        "weight": r.try_get::<Option<f64>, _>("weight")?,
                        "added_by": r.try_get::<String, _>("added_by")?,
                    }))
                })
                .collect::<Result<Vec<_>>>()?;

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
                freshness_score: freshness.score,
                freshness_status: freshness.status.clone(),
                confidence_cap: freshness.confidence_cap.clone(),
                freshness_penalties: freshness.penalties.clone(),
                freshness_components: freshness.components.clone(),
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
                last_evaluated_at: row.try_get("last_evaluated_at").ok(),
                history,
                evidence_items,
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
                         dcl.suggested_new_list,
                         COALESCE(parent.parent_themes, '[]'::jsonb) AS parent_themes,
                         parent.parent_theme_fit
                    FROM discovery_candidate dc
                    LEFT JOIN discovery_classification dcl ON dcl.candidate_id = dc.id
                    LEFT JOIN LATERAL (
                        SELECT max(COALESCE(btt.conviction, 50))::double precision
                                  AS parent_theme_fit,
                               jsonb_agg(
                                   jsonb_build_object(
                                       'key', bt.key,
                                       'name', bt.name,
                                       'scope', bt.scope,
                                       'role', btt.role,
                                       'conviction', btt.conviction,
                                       'rationale', btt.rationale
                                   )
                                   ORDER BY COALESCE(btt.conviction, 0) DESC, bt.name
                               ) AS parent_themes
                          FROM brain_thesis_ticker btt
                          JOIN brain_thesis bt ON bt.id = btt.brain_thesis_id
                         WHERE btt.symbol = dc.symbol
                           AND bt.active = true
                           AND bt.scope IN ('sector', 'theme')
                    ) parent ON true
                   WHERE dc.status = 'proposed'
                ORDER BY dc.symbol, dc.signal_name, dc.proposed_at DESC
               ) latest
            ORDER BY proposed_at DESC"#,
        )
        .fetch_all(&self.pool)
        .await
        .context("pending_discovery_candidates")?;
        let mut ranked = rows
            .into_iter()
            .map(|r| {
                let signal_value: Option<f64> = r.try_get("signal_value").ok();
                let proposed_lists: serde_json::Value = r.try_get("proposed_lists")?;
                let parent_themes: serde_json::Value = r.try_get("parent_themes")?;
                let parent_theme_fit: Option<f64> = r.try_get("parent_theme_fit").ok().flatten();
                let suggested_new_list = r
                    .try_get::<Option<serde_json::Value>, _>("suggested_new_list")
                    .unwrap_or(None);
                let proposed_at: chrono::DateTime<chrono::Utc> = r.try_get("proposed_at")?;
                let rank = crate::discovery::ranking::rank_candidate(
                    &r.try_get::<String, _>("signal_name")?,
                    signal_value,
                    r.try_get::<Option<f64>, _>("domain_fit").ok().flatten(),
                    parent_theme_fit,
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
                        "parent_theme_fit": parent_theme_fit,
                        "parent_themes": parent_themes,
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
        let mut research_nominations = 0usize;
        Ok(ranked
            .into_iter()
            .filter_map(|(_, _, value)| {
                if value.get("signal_name").and_then(serde_json::Value::as_str)
                    == Some("research_nomination")
                {
                    if research_nominations >= 100 {
                        return None;
                    }
                    research_nominations += 1;
                }
                Some(value)
            })
            .collect())
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
                      tech.technical_state AS technical_state,
                      tech.entry_stance AS entry_stance,
                      tech.pct_vs_200d AS technical_pct_vs_200d,
                      freshness.status AS freshness_status,
                      COALESCE(attention.open_count, 0) AS open_attention,
                      COALESCE(attention.states, '[]'::jsonb) AS attention_states,
                      COALESCE(attention.owners, '[]'::jsonb) AS attention_owners,
                      COALESCE(evidence.open_count, 0) AS open_evidence,
                      COALESCE(evidence.blocking_count, 0) AS blocking_evidence,
                      COALESCE(tasks.due_count, 0) AS due_source_tasks,
                      COALESCE(brain.parent_themes, '[]'::jsonb) AS parent_themes,
                      (SELECT count(*) FROM thesis th
                        WHERE th.symbol = wm.symbol
                          AND th.state NOT IN ('closed','disqualified')) AS open_theses
                 FROM watchlist_member wm
            LEFT JOIN LATERAL (
                SELECT th.thesis_id, th.state, th.forecast->>'direction' AS direction,
                       COALESCE(th.last_evaluated_at, th.updated_at) AS evaluated_at
                  FROM thesis th
                 WHERE th.symbol = wm.symbol
                   AND th.state NOT IN ('closed','disqualified')
              ORDER BY th.updated_at DESC
                 LIMIT 1
            ) latest ON TRUE
            LEFT JOIN LATERAL (
                WITH bars AS (
                    SELECT ts, close::float8 AS close, high::float8 AS high
                      FROM price_bar
                     WHERE symbol = wm.symbol
                  ORDER BY ts DESC
                     LIMIT 260
                ), ranked AS (
                    SELECT ts, close, high, row_number() OVER (ORDER BY ts DESC) AS rn
                      FROM bars
                ), latest_bar AS (
                    SELECT close
                      FROM ranked
                     WHERE rn = 1
                ), stats AS (
                    SELECT count(*) FILTER (WHERE rn <= 200) AS bars_200,
                           avg(close) FILTER (WHERE rn <= 50) AS sma50,
                           avg(close) FILTER (WHERE rn <= 200) AS sma200,
                           max(high) FILTER (WHERE rn <= 252) AS high252
                      FROM ranked
                ), classified AS (
                    SELECT CASE
                             WHEN stats.bars_200 < 200 OR stats.sma200 IS NULL THEN 'unknown'
                             WHEN ((latest_bar.close - stats.sma200) / NULLIF(stats.sma200, 0) * 100.0) > 20.0
                               OR ((latest_bar.close - stats.high252) / NULLIF(stats.high252, 0) * 100.0) >= -2.0 THEN 'extended'
                             WHEN ((latest_bar.close - stats.sma200) / NULLIF(stats.sma200, 0) * 100.0) < -5.0 THEN 'deteriorating'
                             WHEN stats.sma50 IS NOT NULL
                               AND abs((latest_bar.close - stats.sma50) / NULLIF(stats.sma50, 0) * 100.0) <= 5.0 THEN 'base_building'
                             WHEN ((latest_bar.close - stats.sma200) / NULLIF(stats.sma200, 0) * 100.0) >= 0.0 THEN 'constructive'
                             ELSE 'unknown'
                           END AS technical_state,
                           ((latest_bar.close - stats.sma200) / NULLIF(stats.sma200, 0) * 100.0)::float8 AS pct_vs_200d
                      FROM latest_bar CROSS JOIN stats
                )
                SELECT technical_state,
                       CASE technical_state
                         WHEN 'extended' THEN 'avoid_chase'
                         WHEN 'deteriorating' THEN 'avoid'
                         WHEN 'base_building' THEN 'wait_breakout'
                         WHEN 'constructive' THEN 'constructive'
                         ELSE 'wait_data'
                       END AS entry_stance,
                       pct_vs_200d
                  FROM classified
            ) tech ON TRUE
            LEFT JOIN LATERAL (
                SELECT tc.created_at AS context_at
                  FROM ticker_context tc
                 WHERE tc.symbol = wm.symbol
              ORDER BY tc.version DESC
                 LIMIT 1
            ) ctx ON TRUE
            LEFT JOIN LATERAL (
                SELECT count(*) AS rows_count,
                       count(*) FILTER (WHERE er.blocking_state <> 'satisfied') AS open_count,
                       count(*) FILTER (
                         WHERE er.priority = 'blocking'
                           AND er.blocking_state <> 'satisfied'
                       ) AS blocking_count
                  FROM evidence_requirement er
                 WHERE er.symbol = wm.symbol
            ) evidence ON TRUE
            LEFT JOIN LATERAL (
                SELECT count(*) FILTER (
                         WHERE st.state IN ('queued', 'no_rows', 'failed', 'rate_limited', 'blocked')
                           AND st.due_at <= now()
                       ) AS due_count,
                       count(*) FILTER (
                         WHERE st.state = 'fetching'
                           AND st.updated_at < now() - interval '30 minutes'
                       ) AS stale_fetching_count,
                       count(*) FILTER (
                         WHERE st.state IN ('failed', 'rate_limited', 'blocked')
                       ) AS blocked_count
                  FROM source_task st
                 WHERE st.scope = 'symbol'
                   AND st.target_id = wm.symbol
            ) tasks ON TRUE
            LEFT JOIN LATERAL (
                SELECT CASE
                         WHEN COALESCE(evidence.blocking_count, 0) > 0
                           OR COALESCE(tasks.blocked_count, 0) > 0 THEN 'blocked'
                         WHEN latest.thesis_id IS NULL
                           OR ctx.context_at IS NULL
                           OR COALESCE(evidence.rows_count, 0) = 0 THEN 'missing'
                         WHEN COALESCE(evidence.open_count, 0) > 0
                           OR COALESCE(tasks.due_count, 0) > 0
                           OR COALESCE(tasks.stale_fetching_count, 0) > 0
                           OR ctx.context_at < now() - interval '12 hours'
                           OR latest.evaluated_at IS NULL
                           OR latest.evaluated_at < now() - interval '30 minutes' THEN 'stale'
                         ELSE 'fresh'
                       END AS status
            ) freshness ON TRUE
            LEFT JOIN LATERAL (
                SELECT
                    (SELECT count(*)
                       FROM attention_item ai
                      WHERE ai.symbol = wm.symbol
                        AND ai.status = 'open'
                        AND (ai.fsm_state <> 'operator_deferred'
                             OR (ai.resurface_at IS NOT NULL AND ai.resurface_at <= now()))) AS open_count,
                    COALESCE((
                        SELECT jsonb_agg(jsonb_build_object('state', s.fsm_state, 'count', s.n)
                                         ORDER BY s.n DESC, s.fsm_state)
                          FROM (
                              SELECT ai.fsm_state, count(*) AS n
                                FROM attention_item ai
                               WHERE ai.symbol = wm.symbol
                                 AND ai.status = 'open'
                                 AND (ai.fsm_state <> 'operator_deferred'
                                      OR (ai.resurface_at IS NOT NULL AND ai.resurface_at <= now()))
                            GROUP BY ai.fsm_state
                          ) s
                    ), '[]'::jsonb) AS states,
                    COALESCE((
                        SELECT jsonb_agg(jsonb_build_object('owner', o.owner, 'count', o.n)
                                         ORDER BY o.n DESC, o.owner)
                          FROM (
                              SELECT ai.owner, count(*) AS n
                                FROM attention_item ai
                               WHERE ai.symbol = wm.symbol
                                 AND ai.status = 'open'
                                 AND (ai.fsm_state <> 'operator_deferred'
                                      OR (ai.resurface_at IS NOT NULL AND ai.resurface_at <= now()))
                            GROUP BY ai.owner
                          ) o
                    ), '[]'::jsonb) AS owners
            ) attention ON TRUE
            LEFT JOIN LATERAL (
                SELECT COALESCE(jsonb_agg(
                         jsonb_build_object(
                           'key', bt.key,
                           'name', bt.name,
                           'scope', bt.scope,
                           'state', bt.state,
                           'direction', bt.direction,
                           'role', btt.role,
                           'conviction', btt.conviction
                         )
                         ORDER BY COALESCE(btt.conviction, 0) DESC, bt.name
                       ), '[]'::jsonb) AS parent_themes
                  FROM brain_thesis_ticker btt
                  JOIN brain_thesis bt ON bt.id = btt.brain_thesis_id
                 WHERE btt.symbol = wm.symbol
                   AND bt.active = true
            ) brain ON TRUE
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
                    technical_state: r.try_get("technical_state").ok(),
                    entry_stance: r.try_get("entry_stance").ok(),
                    technical_pct_vs_200d: r.try_get("technical_pct_vs_200d").ok(),
                    open_theses: r.try_get::<i64, _>("open_theses").unwrap_or(0),
                    freshness_status: r
                        .try_get("freshness_status")
                        .unwrap_or_else(|_| "missing".to_string()),
                    open_attention: r.try_get::<i64, _>("open_attention").unwrap_or(0),
                    attention_states: r
                        .try_get("attention_states")
                        .unwrap_or_else(|_| serde_json::json!([])),
                    attention_owners: r
                        .try_get("attention_owners")
                        .unwrap_or_else(|_| serde_json::json!([])),
                    open_evidence: r.try_get::<i64, _>("open_evidence").unwrap_or(0),
                    blocking_evidence: r.try_get::<i64, _>("blocking_evidence").unwrap_or(0),
                    due_source_tasks: r.try_get::<i64, _>("due_source_tasks").unwrap_or(0),
                    parent_themes: r
                        .try_get("parent_themes")
                        .unwrap_or_else(|_| serde_json::json!([])),
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn freshness_components_penalize_stale_context() {
        let now = Utc.with_ymd_and_hms(2026, 6, 2, 12, 0, 0).unwrap();
        let (component, penalty) = age_component(
            "context",
            now,
            Some(Utc.with_ymd_and_hms(2026, 4, 1, 12, 0, 0).unwrap()),
            FreshnessThresholds {
                fresh: ChronoDuration::days(7),
                stale: ChronoDuration::days(30),
                old: ChronoDuration::days(90),
            },
            "narrative context is stale",
        );

        assert_eq!(component.status, "stale");
        assert_eq!(component.score, 0.6);
        assert_eq!(
            penalty,
            Some("context: narrative context is stale".to_string())
        );
    }

    #[test]
    fn freshness_confidence_cap_blocks_sub_high_scores() {
        let components = vec![
            ThesisFreshnessComponent {
                name: "market".to_string(),
                status: "fresh".to_string(),
                score: 1.0,
                last_at: None,
                reason: "fresh".to_string(),
            },
            ThesisFreshnessComponent {
                name: "context".to_string(),
                status: "stale".to_string(),
                score: 0.6,
                last_at: None,
                reason: "stale".to_string(),
            },
        ];

        assert_eq!(freshness_status(0.60), "stale");
        assert_eq!(
            confidence_cap(0.60, &components),
            Some("medium".to_string())
        );
        assert_eq!(confidence_cap(0.40, &components), Some("low".to_string()));
    }

    #[test]
    fn news_component_penalizes_missing_recent_coverage() {
        let (component, penalty) = news_component(0, None);

        assert_eq!(component.status, "missing");
        assert_eq!(component.score, 0.5);
        assert_eq!(
            penalty,
            Some("news: cannot rely on sentiment-shift evidence".to_string())
        );
    }
}
