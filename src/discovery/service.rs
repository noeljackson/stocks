//! Discovery service — walks the universe, runs enabled signal detectors,
//! upserts discovery_candidate rows for fresh hits, emits discovery.candidate
//! on the MARKET stream.

use std::time::Duration;

use anyhow::{Context, Result};
use sqlx::{Row, postgres::PgPool};
use tracing::{info, warn};

use super::{Config, SignalHit, signals};
use crate::platform::bus::Bus;
use crate::platform::subjects;

/// One pass: for every active symbol, run all enabled signals; persist hits
/// as discovery_candidate rows; publish discovery.candidate per hit.
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

    let mut total_hits = 0;
    for symbol in &symbols {
        match scan_one(pool, symbol, &cfg).await {
            Ok(hits) => {
                for hit in hits {
                    if let Err(e) = persist(pool, &hit, &version.to_string()).await {
                        warn!(symbol = %hit.symbol, signal = hit.signal_name, error = %e, "persist failed");
                        continue;
                    }
                    total_hits += 1;
                    let payload = serde_json::json!({
                        "symbol": hit.symbol,
                        "signal_name": hit.signal_name,
                        "value": hit.value,
                        "reasoning": hit.reasoning,
                        "config_version": version,
                    });
                    if let Err(e) = bus
                        .publish(subjects::DISCOVERY_CANDIDATE, payload.to_string().as_bytes())
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

async fn scan_one(pool: &PgPool, symbol: &str, cfg: &Config) -> Result<Vec<SignalHit>> {
    // Pull recent bars once; share across signal evaluators.
    let rows = sqlx::query(
        r#"SELECT close::float8 AS close, volume::float8 AS volume
             FROM price_bar WHERE symbol = $1
            ORDER BY ts DESC LIMIT 60"#,
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
                signal_name: "volume_anomaly",
                value: mult,
                reasoning: format!("volume {:.1}x 20-day avg", mult),
            });
        }
    }
    if cfg.enabled("base_breakout") {
        if let Some(pct) = signals::base_breakout(&closes, 55, 8.0) {
            hits.push(SignalHit {
                symbol: symbol.to_string(),
                signal_name: "base_breakout",
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
                signal_name: "estimate_revision_velocity",
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
            recent_avg, recent_n as u32, prior_avg, prior_n as u32, 0.3, 3,
        ) {
            hits.push(SignalHit {
                symbol: symbol.to_string(),
                signal_name: "news_sentiment_shift",
                value: shift,
                reasoning: format!(
                    "polarity drift {:+.2} ({}→{} articles, {:+.2}→{:+.2} avg)",
                    shift, prior_n, recent_n, prior_avg, recent_avg
                ),
            });
        }
    }
    Ok(hits)
}

/// Idempotent within a (symbol, signal, second) tuple — UNIQUE constraint
/// on (symbol, signal_name, proposed_at) catches re-runs.
async fn persist(pool: &PgPool, hit: &SignalHit, config_version: &str) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO discovery_candidate (symbol, signal_name, signal_value, reasoning, config_version)
           VALUES ($1, $2, $3, $4, $5)
           ON CONFLICT DO NOTHING"#,
    )
    .bind(&hit.symbol)
    .bind(hit.signal_name)
    .bind(hit.value)
    .bind(&hit.reasoning)
    .bind(config_version)
    .execute(pool)
    .await?;
    Ok(())
}

/// Long-running entry point.
pub async fn run(pool: PgPool, bus: Bus, interval: Duration) -> Result<()> {
    bus.ensure_stream(subjects::STREAM_MARKET, &["regime.*", "discovery.*"])
        .await?;
    info!(interval_secs = interval.as_secs(), "discovery scanner started");
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
