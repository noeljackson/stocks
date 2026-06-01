//! Discovery service — walks the universe, runs enabled signal detectors,
//! composes them into operator-facing interpretations, and emits attention.

use std::time::Duration;

use anyhow::{Context, Result};
use sqlx::{Row, postgres::PgPool};
use tracing::{info, warn};

use super::{
    Config, SignalHit,
    composer::{self, ComposedSignal, PriceExtension, SignalContext},
    signals,
};
use crate::platform::bus::Bus;
use crate::platform::subjects;

/// One pass: for every active symbol, run all enabled signals; persist
/// candidate-worthy hits and publish discovery.candidate for new candidates.
pub async fn run_once(pool: &PgPool, bus: &Bus) -> Result<usize> {
    // Load active config.
    let row = sqlx::query(
        "SELECT body, version FROM config WHERE name = 'discovery_signals' AND active LIMIT 1",
    )
    .fetch_one(pool)
    .await
    .context("load discovery config")?;
    let body: serde_json::Value = row.try_get("body")?;
    let version: i32 = row.try_get("version")?;
    let cfg: Config = serde_json::from_value(body).context("parse discovery config")?;

    // Scan pool = discovery_pool (broad investible names from FMP screener, #88)
    // UNION active tickers (our curated universe). UNION dedups for us. This
    // lets the scanner find signals on names we don't yet track — the whole
    // point of universe self-expansion.
    let symbols: Vec<String> = sqlx::query_scalar(
        r#"SELECT symbol FROM (
              SELECT symbol FROM discovery_pool WHERE dropped_at IS NULL
              UNION
              SELECT symbol FROM ticker WHERE status = 'active'
           ) s
           ORDER BY symbol"#,
    )
    .fetch_all(pool)
    .await
    .context("load scan pool")?;

    supersede_active_ticker_candidate_reviews(pool).await?;

    let mut total_hits = 0;
    for symbol in &symbols {
        match scan_one(pool, symbol, &cfg).await {
            Ok(hits) => {
                for hit in hits {
                    let should_publish_candidate = match persist(pool, &hit, &version.to_string())
                        .await
                    {
                        Ok(should_publish_candidate) => should_publish_candidate,
                        Err(e) => {
                            warn!(symbol = %hit.symbol, signal = hit.signal_name, error = %e, "persist failed");
                            continue;
                        }
                    };
                    total_hits += 1;
                    if !should_publish_candidate {
                        info!(symbol = %hit.symbol, signal = hit.signal_name, value = hit.value, "discovery hit routed to existing thesis");
                        continue;
                    }
                    let payload = serde_json::json!({
                        "symbol": hit.symbol,
                        "signal_name": hit.signal_name,
                        "value": hit.value,
                        "reasoning": hit.reasoning,
                        "config_version": version,
                    });
                    if let Err(e) = bus
                        .publish(
                            subjects::DISCOVERY_CANDIDATE,
                            payload.to_string().as_bytes(),
                        )
                        .await
                    {
                        warn!(error = %e, "publish discovery.candidate failed (non-fatal)");
                    }
                    info!(symbol = %hit.symbol, signal = hit.signal_name, value = hit.value, "discovery hit");
                }
            }
            Err(e) => warn!(symbol = %symbol, error = %e, "scan_one failed; skipping"),
        }
    }
    Ok(total_hits)
}

async fn scan_one(pool: &PgPool, symbol: &str, cfg: &Config) -> Result<Vec<ComposedSignal>> {
    // Pull recent bars once; share across signal evaluators.
    let rows = sqlx::query(
        r#"SELECT close::float8 AS close, volume::float8 AS volume
             FROM price_bar WHERE symbol = $1
            ORDER BY ts DESC LIMIT 260"#,
    )
    .bind(symbol)
    .fetch_all(pool)
    .await?;
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let closes: Vec<f64> = rows.iter().map(|r| r.get::<f64, _>("close")).collect();
    let volumes: Vec<f64> = rows.iter().map(|r| r.get::<f64, _>("volume")).collect();

    let mut hits = Vec::new();

    if cfg.enabled("volume_anomaly") {
        if let Some(mult) = signals::volume_anomaly(&volumes, 3.0) {
            hits.push(SignalHit {
                symbol: symbol.to_string(),
                signal_name: "volume_anomaly".to_string(),
                value: mult,
                reasoning: format!("volume {:.1}x 20-day avg", mult),
            });
        }
    }
    if cfg.enabled("base_breakout") {
        if let Some(pct) = signals::base_breakout(&closes, 55, 8.0) {
            hits.push(SignalHit {
                symbol: symbol.to_string(),
                signal_name: "base_breakout".to_string(),
                value: pct,
                reasoning: format!("close {:.2}% above prior 55-day high after tight base", pct),
            });
        }
    }
    // New data-driven signals (#18, #19) — same Option<f64> contract.
    if cfg.enabled("estimate_revision_velocity") {
        let counts = sqlx::query(
            r#"SELECT
                 count(*) FILTER (WHERE direction = 'up')   AS up,
                 count(*) FILTER (WHERE direction = 'down') AS down
               FROM estimate_revision
              WHERE symbol = $1
                AND detected_at > now() - interval '14 days'"#,
        )
        .bind(symbol)
        .fetch_one(pool)
        .await?;
        let up: i64 = counts.try_get("up").unwrap_or(0);
        let down: i64 = counts.try_get("down").unwrap_or(0);
        if let Some(net) = signals::estimate_revision_velocity(up as u32, down as u32, 3) {
            hits.push(SignalHit {
                symbol: symbol.to_string(),
                signal_name: "estimate_revision_velocity".to_string(),
                value: net,
                reasoning: format!(
                    "{} net revisions in last 14d ({}↑ {}↓)",
                    net as i32, up, down
                ),
            });
        }
    }
    if cfg.enabled("news_sentiment_shift") {
        let row = sqlx::query(
            r#"WITH recent AS (
                  SELECT count(*) AS n, COALESCE(avg(sentiment_polarity), 0) AS avg_pol
                    FROM news_article
                   WHERE symbol = $1
                     AND sentiment_polarity IS NOT NULL
                     AND published_at > now() - interval '3 days'
               ), prior AS (
                  SELECT count(*) AS n, COALESCE(avg(sentiment_polarity), 0) AS avg_pol
                    FROM news_article
                   WHERE symbol = $1
                     AND sentiment_polarity IS NOT NULL
                     AND published_at > now() - interval '10 days'
                     AND published_at <= now() - interval '3 days'
               )
               SELECT recent.n::int8 AS rn, recent.avg_pol::float8 AS ra,
                      prior.n::int8 AS pn, prior.avg_pol::float8 AS pa
                 FROM recent, prior"#,
        )
        .bind(symbol)
        .fetch_one(pool)
        .await?;
        let recent_n: i64 = row.try_get("rn")?;
        let recent_avg: f64 = row.try_get("ra")?;
        let prior_n: i64 = row.try_get("pn")?;
        let prior_avg: f64 = row.try_get("pa")?;
        if let Some(shift) = signals::news_sentiment_shift(
            recent_avg,
            recent_n as u32,
            prior_avg,
            prior_n as u32,
            0.3,
            3,
        ) {
            hits.push(SignalHit {
                symbol: symbol.to_string(),
                signal_name: "news_sentiment_shift".to_string(),
                value: shift,
                reasoning: format!(
                    "polarity drift {:+.2} ({}→{} articles, {:+.2}→{:+.2} avg)",
                    shift, prior_n, recent_n, prior_avg, recent_avg
                ),
            });
        }
    }
    let ctx = load_signal_context(pool, symbol).await?;
    let composed = composer::compose(
        symbol,
        &hits,
        PriceExtension::from_closes_desc(&closes),
        &ctx,
    );
    if composed.is_none() && !hits.is_empty() {
        if ctx.is_active_ticker {
            supersede_open_candidates_for_symbol(pool, symbol, "suppressed_tracked_ticker_update")
                .await?;
        } else {
            supersede_raw_open_candidates(pool, symbol, &hits, "suppressed_low_quality_noise")
                .await?;
        }
    }
    Ok(composed.into_iter().collect())
}

async fn load_signal_context(pool: &PgPool, symbol: &str) -> Result<SignalContext> {
    let row = sqlx::query(
        r#"SELECT
             EXISTS (
               SELECT 1 FROM thesis
                WHERE symbol = $1
                  AND state NOT IN ('closed', 'disqualified')
             ) AS has_open_thesis,
             (
               SELECT thesis_id FROM thesis
                WHERE symbol = $1
                  AND state IN ('armed', 'actionable', 'position_open')
                ORDER BY updated_at DESC
                LIMIT 1
             ) AS actionable_thesis_id,
             EXISTS (
               SELECT 1 FROM ticker
                WHERE symbol = $1
                  AND status = 'active'
             ) AS is_active_ticker,
             EXISTS (
               SELECT 1 FROM watchlist_member
                WHERE symbol = $1
             ) AS is_watchlisted"#,
    )
    .bind(symbol)
    .fetch_one(pool)
    .await
    .context("load_signal_context")?;
    let actionable_thesis_id: Option<uuid::Uuid> = row.try_get("actionable_thesis_id")?;
    Ok(SignalContext {
        has_open_thesis: row.try_get("has_open_thesis")?,
        has_actionable_thesis: actionable_thesis_id.is_some(),
        actionable_thesis_id,
        is_active_ticker: row.try_get("is_active_ticker")?,
        is_watchlisted: row.try_get("is_watchlisted")?,
    })
}

/// Idempotent. Returns whether this hit should also publish discovery.candidate.
/// Skips when an OPEN candidate (status='proposed') already exists for
/// (symbol, signal_name) — re-firing the same signal at 5-minute intervals
/// would otherwise create dozens of duplicate rows per ticker (#105). Once the
/// operator confirms or rejects, a fresh firing CAN create a new candidate.
///
/// On insert, also fires an attention_item(candidate_review) so the operator
/// surface (#86) picks it up immediately. The attention table's partial
/// unique on (kind, candidate_id) WHERE status='open' dedups across runs.
async fn persist(pool: &PgPool, hit: &ComposedSignal, config_version: &str) -> Result<bool> {
    use crate::attention::{initial_assignment, kind, severity, source, title_for_candidate};
    if hit.kind == composer::DiscoveryInterpretationKind::ExistingThesisTrigger {
        if let Some(thesis_id) = hit.thesis_id {
            persist_existing_thesis_trigger(pool, hit, config_version, thesis_id).await?;
            return Ok(false);
        }
    }
    supersede_stale_open_candidates(pool, hit).await?;
    refresh_open_attention_title(pool, hit).await?;
    let row = sqlx::query(
        r#"INSERT INTO discovery_candidate
                 (symbol, signal_name, signal_value, reasoning, config_version)
           SELECT $1, $2, $3, $4, $5
            WHERE NOT EXISTS (
                  SELECT 1 FROM discovery_candidate
                   WHERE symbol = $1
                     AND signal_name = $2
                     AND status = 'proposed'
                  )
           ON CONFLICT DO NOTHING
           RETURNING id"#,
    )
    .bind(&hit.symbol)
    .bind(&hit.signal_name)
    .bind(hit.value)
    .bind(&hit.reasoning)
    .bind(config_version)
    .fetch_optional(pool)
    .await?;
    if let Some(row) = row {
        let id: i64 = row.try_get("id")?;
        let source_ref = serde_json::json!({
            "candidate_id": id,
            "signal_value": hit.value,
            "interpretation_kind": hit.kind,
            "raw_signals": hit.raw_signals,
            "price_extension": hit.price_extension,
            "config_version": config_version,
        });
        let (fsm_state, owner) =
            initial_assignment(kind::CANDIDATE_REVIEW, severity::REVIEW, source::DISCOVERY);
        if let Err(e) = sqlx::query(
            r#"INSERT INTO attention_item
                 (kind, symbol, candidate_id, severity, title, reason, source, source_ref,
                  fsm_state, owner, state_reason)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8::jsonb, $9, $10, $11)
               ON CONFLICT DO NOTHING"#,
        )
        .bind(kind::CANDIDATE_REVIEW)
        .bind(&hit.symbol)
        .bind(id)
        .bind(severity::REVIEW)
        .bind(title_for_candidate(&hit.symbol, &hit.signal_name))
        .bind(&hit.reasoning)
        .bind(source::DISCOVERY)
        .bind(source_ref)
        .bind(fsm_state)
        .bind(owner)
        .bind(hit.kind.signal_name())
        .execute(pool)
        .await
        {
            tracing::warn!(error = %e, "attention candidate_review insert failed (non-fatal)");
        }
    }
    Ok(true)
}

async fn persist_existing_thesis_trigger(
    pool: &PgPool,
    hit: &ComposedSignal,
    config_version: &str,
    thesis_id: uuid::Uuid,
) -> Result<()> {
    use crate::attention::{
        initial_assignment, kind, severity, source, title_for_thesis_actionable,
    };
    supersede_stale_open_candidates(pool, hit).await?;
    let source_ref = serde_json::json!({
        "signal_value": hit.value,
        "interpretation_kind": hit.kind,
        "raw_signals": hit.raw_signals,
        "price_extension": hit.price_extension,
        "config_version": config_version,
    });
    let (fsm_state, owner) = initial_assignment(
        kind::THESIS_ACTIONABLE,
        severity::DECISION,
        source::DISCOVERY,
    );
    sqlx::query(
        r#"INSERT INTO attention_item
             (kind, symbol, thesis_id, severity, title, reason, source, source_ref,
              fsm_state, owner, state_reason)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8::jsonb, $9, $10, $11)
           ON CONFLICT DO NOTHING"#,
    )
    .bind(kind::THESIS_ACTIONABLE)
    .bind(&hit.symbol)
    .bind(thesis_id)
    .bind(severity::DECISION)
    .bind(title_for_thesis_actionable(&hit.symbol))
    .bind(&hit.reasoning)
    .bind(source::DISCOVERY)
    .bind(&source_ref)
    .bind(fsm_state)
    .bind(owner)
    .bind("existing_thesis_trigger")
    .execute(pool)
    .await
    .context("insert existing thesis trigger attention")?;
    sqlx::query(
        r#"UPDATE attention_item
              SET title = $3,
                  reason = $4,
                  source_ref = source_ref || $5::jsonb,
                  fsm_state = $6,
                  owner = $7,
                  state_reason = $8
            WHERE kind = $1
              AND thesis_id = $2
              AND status = 'open'"#,
    )
    .bind(kind::THESIS_ACTIONABLE)
    .bind(thesis_id)
    .bind(title_for_thesis_actionable(&hit.symbol))
    .bind(&hit.reasoning)
    .bind(serde_json::json!({ "latest_discovery_trigger": source_ref }))
    .bind(fsm_state)
    .bind(owner)
    .bind("existing_thesis_trigger")
    .execute(pool)
    .await
    .context("refresh existing thesis trigger attention")?;
    Ok(())
}

async fn refresh_open_attention_title(pool: &PgPool, hit: &ComposedSignal) -> Result<()> {
    use crate::attention::{kind, title_for_candidate};
    sqlx::query(
        r#"UPDATE attention_item ai
              SET title = $3,
                  reason = $4
             FROM discovery_candidate dc
            WHERE ai.candidate_id = dc.id
              AND ai.kind = $5
              AND ai.status = 'open'
              AND dc.status = 'proposed'
              AND dc.symbol = $1
              AND dc.signal_name = $2"#,
    )
    .bind(&hit.symbol)
    .bind(&hit.signal_name)
    .bind(title_for_candidate(&hit.symbol, &hit.signal_name))
    .bind(&hit.reasoning)
    .bind(kind::CANDIDATE_REVIEW)
    .execute(pool)
    .await
    .context("refresh_open_attention_title")?;
    Ok(())
}

async fn supersede_stale_open_candidates(pool: &PgPool, hit: &ComposedSignal) -> Result<()> {
    use crate::attention::kind;
    let other_composed = [
        "early_accumulation",
        "breakout_confirmation",
        "extended_momentum",
        "consensus_arrival",
        "possible_exhaustion",
        "existing_thesis_trigger",
    ]
    .into_iter()
    .filter(|name| *name != hit.signal_name)
    .map(String::from)
    .collect::<Vec<_>>();
    let raw_signals = hit.raw_signals.clone();
    sqlx::query(
        r#"WITH stale AS (
             UPDATE discovery_candidate
                SET status = 'superseded'
              WHERE symbol = $1
                AND status = 'proposed'
                AND (
                  signal_name = ANY($2::text[])
                  OR signal_name = ANY($3::text[])
                )
              RETURNING id
           ),
           matched AS (
             SELECT ai.id, ai.fsm_state
               FROM attention_item ai
               JOIN stale ON ai.candidate_id = stale.id
              WHERE ai.kind = $5
                AND ai.status = 'open'
              FOR UPDATE OF ai
           ),
           updated AS (
             UPDATE attention_item ai
                SET status = 'dismissed',
                    fsm_state = 'dismissed',
                    owner = 'system',
                    resolved_at = COALESCE(resolved_at, now()),
                    resolution_kind = 'superseded_by_composed_discovery',
                    resolution_ref = jsonb_build_object(
                    'symbol', $1,
                    'signal_name', $4,
                    'raw_signals', to_jsonb($2::text[])
                  ),
                    next_retry_at = NULL,
                    resurface_at = NULL,
                    state_reason = 'superseded_by_composed_discovery'
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
    .bind(&hit.symbol)
    .bind(&raw_signals)
    .bind(&other_composed)
    .bind(&hit.signal_name)
    .bind(kind::CANDIDATE_REVIEW)
    .execute(pool)
    .await
    .context("supersede_stale_open_candidates")?;
    Ok(())
}

async fn supersede_raw_open_candidates(
    pool: &PgPool,
    symbol: &str,
    raw_hits: &[SignalHit],
    reason: &str,
) -> Result<()> {
    use crate::attention::kind;
    let raw_signals = raw_hits
        .iter()
        .map(|hit| hit.signal_name.clone())
        .collect::<Vec<_>>();
    sqlx::query(
        r#"WITH stale AS (
             UPDATE discovery_candidate
                SET status = 'superseded'
              WHERE symbol = $1
                AND status = 'proposed'
                AND signal_name = ANY($2::text[])
              RETURNING id
           ),
           matched AS (
             SELECT ai.id, ai.fsm_state
               FROM attention_item ai
               JOIN stale ON ai.candidate_id = stale.id
              WHERE ai.kind = $4
                AND ai.status = 'open'
              FOR UPDATE OF ai
           ),
           updated AS (
             UPDATE attention_item ai
                SET status = 'dismissed',
                    fsm_state = 'dismissed',
                    owner = 'system',
                    resolved_at = COALESCE(resolved_at, now()),
                    resolution_kind = $3,
                    resolution_ref = jsonb_build_object(
                    'symbol', $1,
                    'raw_signals', to_jsonb($2::text[])
                  ),
                    next_retry_at = NULL,
                    resurface_at = NULL,
                    state_reason = $3
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
    .bind(symbol)
    .bind(&raw_signals)
    .bind(reason)
    .bind(kind::CANDIDATE_REVIEW)
    .execute(pool)
    .await
    .context("supersede_raw_open_candidates")?;
    Ok(())
}

async fn supersede_open_candidates_for_symbol(
    pool: &PgPool,
    symbol: &str,
    reason: &str,
) -> Result<()> {
    use crate::attention::kind;
    sqlx::query(
        r#"WITH stale AS (
             UPDATE discovery_candidate
                SET status = 'superseded'
              WHERE symbol = $1
                AND status = 'proposed'
              RETURNING id
           ),
           matched AS (
             SELECT ai.id, ai.fsm_state
               FROM attention_item ai
               JOIN stale ON ai.candidate_id = stale.id
              WHERE ai.kind = $3
                AND ai.status = 'open'
              FOR UPDATE OF ai
           ),
           updated AS (
             UPDATE attention_item ai
                SET status = 'dismissed',
                    fsm_state = 'dismissed',
                    owner = 'system',
                    resolved_at = COALESCE(resolved_at, now()),
                    resolution_kind = $2,
                    resolution_ref = jsonb_build_object('symbol', $1),
                    next_retry_at = NULL,
                    resurface_at = NULL,
                    state_reason = $2
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
    .bind(symbol)
    .bind(reason)
    .bind(kind::CANDIDATE_REVIEW)
    .execute(pool)
    .await
    .context("supersede_open_candidates_for_symbol")?;
    Ok(())
}

async fn supersede_active_ticker_candidate_reviews(pool: &PgPool) -> Result<()> {
    use crate::attention::kind;
    sqlx::query(
        r#"WITH stale AS (
             UPDATE discovery_candidate dc
                SET status = 'superseded'
               FROM ticker t
              WHERE dc.symbol = t.symbol
                AND t.status = 'active'
                AND dc.status = 'proposed'
              RETURNING dc.id, dc.symbol
           ),
           matched AS (
             SELECT ai.id, ai.fsm_state, stale.symbol
               FROM attention_item ai
               JOIN stale ON ai.candidate_id = stale.id
              WHERE ai.kind = $1
                AND ai.status = 'open'
              FOR UPDATE OF ai
           ),
           updated AS (
             UPDATE attention_item ai
                SET status = 'dismissed',
                    fsm_state = 'dismissed',
                    owner = 'system',
                    resolved_at = COALESCE(resolved_at, now()),
                    resolution_kind = 'suppressed_tracked_ticker_update',
                    resolution_ref = jsonb_build_object('symbol', m.symbol),
                    next_retry_at = NULL,
                    resurface_at = NULL,
                    state_reason = 'suppressed_tracked_ticker_update'
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
    .bind(kind::CANDIDATE_REVIEW)
    .execute(pool)
    .await
    .context("supersede_active_ticker_candidate_reviews")?;
    Ok(())
}

/// Long-running entry point.
pub async fn run(pool: PgPool, bus: Bus, interval: Duration) -> Result<()> {
    bus.ensure_stream(subjects::STREAM_MARKET, &["regime.*", "discovery.*"])
        .await?;
    info!(
        interval_secs = interval.as_secs(),
        "discovery scanner started"
    );
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        match run_once(&pool, &bus).await {
            Ok(n) if n > 0 => info!(hits = n, "discovery pass complete"),
            Ok(_) => {}
            Err(e) => warn!(error = %e, "discovery pass failed"),
        }
    }
}
