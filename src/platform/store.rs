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
    Alert, AlertKind, Condition, MarketStateRow, ThesisDetail, ThesisSubstance,
    ThesisVersionEvent, TickerContextRow, TickerRow, Watchlist, WatchlistMember,
    WellFormedCondCounts,
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

    /// All open positions in the shape the risk overlay consumes.
    /// Daily candles for `symbol` over the last `lookback_days`, oldest first.
    /// Shaped for lightweight-charts (each row has `time` as ISO date + OHLCV).
    pub async fn candles_for(
        &self,
        symbol: &str,
        lookback_days: i64,
    ) -> Result<Vec<serde_json::Value>> {
        let rows = sqlx::query(
            r#"SELECT ts, open::float8 AS open, high::float8 AS high,
                      low::float8 AS low, close::float8 AS close,
                      volume::float8 AS volume
                 FROM price_bar
                WHERE symbol = $1
                  AND ts > now() - ($2 || ' days')::interval
             ORDER BY ts ASC"#,
        )
        .bind(symbol)
        .bind(lookback_days.to_string())
        .fetch_all(&self.pool)
        .await
        .context("candles_for")?;
        rows.into_iter()
            .map(|r| {
                let ts: DateTime<Utc> = r.try_get("ts")?;
                Ok(serde_json::json!({
                    "time": ts.format("%Y-%m-%d").to_string(),
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

    /// Loads a single thesis by id, with the same enrichment that
    /// `theses_for_symbol` produces (substance, history). Returns
    /// `Vec<ThesisDetail>` (will have 0 or 1 entry) so the caller can reuse
    /// the existing per-symbol code path.
    pub async fn theses_for_symbol_id(&self, thesis_id: uuid::Uuid) -> Result<Vec<ThesisDetail>> {
        let symbol: Option<String> = sqlx::query_scalar(
            "SELECT symbol FROM thesis WHERE thesis_id = $1",
        )
        .bind(thesis_id)
        .fetch_optional(&self.pool)
        .await
        .context("symbol lookup")?;
        let Some(symbol) = symbol else { return Ok(vec![]) };
        let all = self.theses_for_symbol(&symbol).await?;
        Ok(all.into_iter().filter(|t| t.thesis_id == thesis_id).collect())
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
                         dc.id, dc.symbol, dc.signal_name, dc.signal_value, dc.reasoning,
                         dc.proposed_at,
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
        rows.into_iter()
            .map(|r| {
                let signal_value: Option<f64> = r.try_get("signal_value").ok();
                Ok(serde_json::json!({
                    "id": r.try_get::<i64, _>("id")?,
                    "symbol": r.try_get::<String, _>("symbol")?,
                    "signal_name": r.try_get::<String, _>("signal_name")?,
                    "signal_value": signal_value,
                    "reasoning": r.try_get::<Option<String>, _>("reasoning").ok(),
                    "proposed_at": r.try_get::<chrono::DateTime<chrono::Utc>, _>("proposed_at")?,
                    "proposed_lists": r.try_get::<serde_json::Value, _>("proposed_lists")?,
                    "suggested_new_list": r
                        .try_get::<Option<serde_json::Value>, _>("suggested_new_list")
                        .unwrap_or(None),
                }))
            })
            .collect()
    }

    /// Confirm a candidate to one or more watchlists. Updates status, adds
    /// the symbol to each list (idempotent), records timestamp.
    pub async fn confirm_discovery_candidate(
        &self,
        candidate_id: i64,
        watchlist_ids: &[uuid::Uuid],
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.context("begin tx")?;
        let row =
            sqlx::query("SELECT symbol, signal_name FROM discovery_candidate WHERE id = $1")
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
        tx.commit().await.context("commit tx")?;
        Ok(())
    }

    pub async fn reject_discovery_candidate(&self, candidate_id: i64) -> Result<bool> {
        let res = sqlx::query(
            "UPDATE discovery_candidate SET status = 'rejected', decided_at = now() WHERE id = $1",
        )
        .bind(candidate_id)
        .execute(&self.pool)
        .await
        .context("reject_discovery_candidate")?;
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
            r#"SELECT watchlist_id, symbol, added_at, added_by
                 FROM watchlist_member
                WHERE watchlist_id = $1
             ORDER BY added_at DESC"#,
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
        let res = sqlx::query(
            "DELETE FROM watchlist_member WHERE watchlist_id = $1 AND symbol = $2",
        )
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
