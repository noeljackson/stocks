//! Consensus service — walks the active universe, scores each symbol,
//! persists, emits thesis.fulfilled when exit threshold crosses for
//! discovery theses on the symbol.

use std::time::Duration;

use anyhow::{Context, Result};
use sqlx::{Row, postgres::PgPool};
use tracing::{info, warn};

use super::{Config, components, compose};
use crate::platform::bus::Bus;
use crate::platform::subjects;

/// One pass: load config + universe, score each symbol, persist, optionally
/// emit fulfillment events. Returns symbols scored.
pub async fn run_once(pool: &PgPool, bus: &Bus) -> Result<usize> {
    // 1. Load active config.
    let row = sqlx::query(
        "SELECT body, version FROM config WHERE name = 'consensus' AND active LIMIT 1",
    )
    .fetch_one(pool)
    .await
    .context("load consensus config")?;
    let body: serde_json::Value = row.try_get("body")?;
    let version: i32 = row.try_get("version")?;
    let cfg: Config = serde_json::from_value(body).context("parse consensus config")?;

    // 2. Load active tickers.
    let symbols: Vec<String> = sqlx::query_scalar(
        "SELECT symbol FROM ticker WHERE status = 'active' ORDER BY symbol",
    )
    .fetch_all(pool)
    .await
    .context("load active tickers")?;

    let mut scored = 0;
    for symbol in &symbols {
        match score_symbol(pool, symbol, &cfg).await {
            Ok(score) => {
                // 3. Detect a fresh measurement OR exit crossing — fire once per crossing.
                let prev = previous_thresholds(pool, symbol).await?;
                let measurement_just_crossed = score.measurement_crossed && !prev.measurement;
                let exit_just_crossed = score.exit_crossed && !prev.exit;

                // 4. Persist.
                persist(pool, &score, &version.to_string()).await?;
                scored += 1;

                // 5. Emit on fresh crossings.
                if measurement_just_crossed || exit_just_crossed {
                    let payload = serde_json::json!({
                        "symbol": symbol,
                        "score": score.total,
                        "measurement_crossed": score.measurement_crossed,
                        "exit_crossed": score.exit_crossed,
                        "fresh_measurement_crossing": measurement_just_crossed,
                        "fresh_exit_crossing": exit_just_crossed,
                        "components": score.components,
                        "config_version": version,
                    });
                    // measurement crossing → emit thesis.updated so reflection can
                    // anchor lead_time; exit crossing → emit thesis.fulfilled.
                    let topic = if exit_just_crossed {
                        subjects::THESIS_FULFILLED
                    } else {
                        subjects::THESIS_UPDATED
                    };
                    if let Err(e) = bus.publish(topic, payload.to_string().as_bytes()).await {
                        warn!(error = %e, "consensus publish failed (non-fatal)");
                    } else {
                        info!(
                            symbol = symbol,
                            score = score.total,
                            measurement = score.measurement_crossed,
                            exit = score.exit_crossed,
                            "consensus crossing"
                        );
                    }
                }
            }
            Err(e) => warn!(symbol = %symbol, error = %e, "score_symbol failed; skipping"),
        }
    }
    Ok(scored)
}

async fn score_symbol(
    pool: &PgPool,
    symbol: &str,
    cfg: &Config,
) -> Result<super::Score> {
    // Compute each component. Three were stubs until we got the inputs in
    // PRs #18/#19 — now they read from estimate_revision and news_article.
    // retail_attention still waits on #20 (crowd sentiment).
    let pe_raw = components::price_extension(pool, symbol).await?;
    let pe = weight(pe_raw, cfg.weights.price_extension);

    let coverage = weight(
        components::coverage_expansion(pool, symbol).await?,
        cfg.weights.coverage_expansion,
    );
    let estimate = weight(
        components::estimate_revision_saturation(pool, symbol).await?,
        cfg.weights.estimate_revision_saturation,
    );
    let mainstream = weight(
        components::mainstream_coverage(pool, symbol).await?,
        cfg.weights.mainstream_coverage,
    );
    let retail = weight(
        components::retail_attention(pool).await?,
        cfg.weights.retail_attention,
    );

    Ok(compose(symbol, vec![coverage, estimate, mainstream, retail, pe], cfg))
}

fn weight(mut c: super::ComponentScore, w: f64) -> super::ComponentScore {
    c.weighted = c.raw * w / 100.0;
    c
}

#[derive(Debug, Default)]
struct PrevCrossings {
    measurement: bool,
    exit: bool,
}

async fn previous_thresholds(pool: &PgPool, symbol: &str) -> Result<PrevCrossings> {
    let row = sqlx::query(
        r#"SELECT measurement_crossed, exit_crossed
             FROM consensus_score WHERE symbol = $1
         ORDER BY computed_at DESC LIMIT 1"#,
    )
    .bind(symbol)
    .fetch_optional(pool)
    .await?;
    Ok(match row {
        Some(r) => PrevCrossings {
            measurement: r.try_get("measurement_crossed").unwrap_or(false),
            exit: r.try_get("exit_crossed").unwrap_or(false),
        },
        None => PrevCrossings::default(),
    })
}

async fn persist(pool: &PgPool, score: &super::Score, config_version: &str) -> Result<()> {
    let components_json = serde_json::to_value(&score.components)?;
    sqlx::query(
        r#"INSERT INTO consensus_score
             (symbol, score, components, measurement_crossed, exit_crossed, config_version)
           VALUES ($1, $2, $3, $4, $5, $6)"#,
    )
    .bind(&score.symbol)
    .bind(score.total)
    .bind(&components_json)
    .bind(score.measurement_crossed)
    .bind(score.exit_crossed)
    .bind(config_version)
    .execute(pool)
    .await
    .context("persist consensus_score")?;
    Ok(())
}

/// Long-running entry point.
pub async fn run(pool: PgPool, bus: Bus, interval: Duration) -> Result<()> {
    bus.ensure_stream(subjects::STREAM_THESIS, &["thesis.*"]).await?;
    info!(interval_secs = interval.as_secs(), "consensus service started");
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        match run_once(&pool, &bus).await {
            Ok(n) if n > 0 => info!(scored = n, "consensus pass complete"),
            Ok(_) => {}
            Err(e) => warn!(error = %e, "consensus pass failed"),
        }
    }
}

