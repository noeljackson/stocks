//! Consensus service — walks the active universe, scores each symbol,
//! persists, emits thesis.fulfilled when exit threshold crosses for
//! discovery theses on the symbol.

use std::time::Duration;

use anyhow::{Context, Result};
use sqlx::{Row, postgres::PgPool};
use tracing::{info, warn};
use uuid::Uuid;

use super::{Config, components, compose};
use crate::attention::{kind, severity, source};
use crate::platform::bus::Bus;
use crate::platform::subjects;

/// One pass: load config + universe, score each symbol, persist, optionally
/// emit fulfillment events. Returns symbols scored.
pub async fn run_once(pool: &PgPool, bus: &Bus) -> Result<usize> {
    // 1. Load active config.
    let row =
        sqlx::query("SELECT body, version FROM config WHERE name = 'consensus' AND active LIMIT 1")
            .fetch_one(pool)
            .await
            .context("load consensus config")?;
    let body: serde_json::Value = row.try_get("body")?;
    let version: i32 = row.try_get("version")?;
    let cfg: Config = serde_json::from_value(body).context("parse consensus config")?;

    // 2. Load active tickers.
    let symbols: Vec<String> =
        sqlx::query_scalar("SELECT symbol FROM ticker WHERE status = 'active' ORDER BY symbol")
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
                    let open_thesis_id = latest_open_thesis_id(pool, symbol).await?;
                    let Some(topic) =
                        thesis_crossing_topic(open_thesis_id.is_some(), exit_just_crossed)
                    else {
                        record_missing_thesis_attention(pool, bus, &score, version).await?;
                        info!(
                            symbol = symbol,
                            score = score.total,
                            "consensus crossing queued for cognition; no open thesis"
                        );
                        continue;
                    };
                    let Some(thesis_id) = open_thesis_id else {
                        continue;
                    };
                    let payload = serde_json::json!({
                        "thesis_id": thesis_id,
                        "symbol": symbol,
                        "score": score.total,
                        "measurement_crossed": score.measurement_crossed,
                        "exit_crossed": score.exit_crossed,
                        "fresh_measurement_crossing": measurement_just_crossed,
                        "fresh_exit_crossing": exit_just_crossed,
                        "components": score.components,
                        "config_version": version,
                        "version": version,
                        "rationale": "Consensus threshold crossing",
                    });
                    // measurement crossing → emit thesis.updated so reflection can
                    // anchor lead_time; exit crossing → emit thesis.fulfilled.
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

async fn score_symbol(pool: &PgPool, symbol: &str, cfg: &Config) -> Result<super::Score> {
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

    Ok(compose(
        symbol,
        vec![coverage, estimate, mainstream, retail, pe],
        cfg,
    ))
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

async fn latest_open_thesis_id(pool: &PgPool, symbol: &str) -> Result<Option<Uuid>> {
    sqlx::query_scalar(
        r#"SELECT thesis_id
             FROM thesis
            WHERE symbol = $1
              AND state NOT IN ('closed', 'disqualified')
         ORDER BY updated_at DESC
            LIMIT 1"#,
    )
    .bind(symbol)
    .fetch_optional(pool)
    .await
    .context("latest_open_thesis_id")
}

fn thesis_crossing_topic(has_open_thesis: bool, exit_just_crossed: bool) -> Option<&'static str> {
    if !has_open_thesis {
        return None;
    }
    Some(if exit_just_crossed {
        subjects::THESIS_FULFILLED
    } else {
        subjects::THESIS_UPDATED
    })
}

fn missing_thesis_attention_text(symbol: &str, score: f64) -> (String, String) {
    (
        format!("{symbol} needs a thesis after consensus crossing"),
        format!(
            "Consensus score {score:.1} crossed the measurement threshold, but {symbol} has no open thesis. Run cognition to create or decline a current standing view."
        ),
    )
}

async fn record_missing_thesis_attention(
    pool: &PgPool,
    bus: &Bus,
    score: &super::Score,
    config_version: i32,
) -> Result<()> {
    let (title, reason) = missing_thesis_attention_text(&score.symbol, score.total);
    let source_ref = serde_json::json!({
        "score": score.total,
        "measurement_crossed": score.measurement_crossed,
        "exit_crossed": score.exit_crossed,
        "components": score.components,
        "config_version": config_version,
        "trigger": "consensus_crossing_without_thesis",
    });
    sqlx::query(
        r#"INSERT INTO attention_item
             (kind, symbol, severity, title, reason, source, source_ref)
           VALUES ($1, $2, $3, $4, $5, $6, $7::jsonb)
           ON CONFLICT DO NOTHING"#,
    )
    .bind(kind::THESIS_INCOMPLETE)
    .bind(&score.symbol)
    .bind(severity::REVIEW)
    .bind(&title)
    .bind(&reason)
    .bind(source::CONSENSUS)
    .bind(&source_ref)
    .execute(pool)
    .await
    .context("insert missing-thesis consensus attention")?;
    sqlx::query(
        r#"UPDATE attention_item
              SET title = $3,
                  reason = $4,
                  source_ref = $5::jsonb
            WHERE kind = $1
              AND symbol = $2
              AND status = 'open'
              AND thesis_id IS NULL
              AND candidate_id IS NULL"#,
    )
    .bind(kind::THESIS_INCOMPLETE)
    .bind(&score.symbol)
    .bind(&title)
    .bind(&reason)
    .bind(&source_ref)
    .execute(pool)
    .await
    .context("refresh missing-thesis consensus attention")?;

    let payload = serde_json::json!({
        "symbol": score.symbol,
        "source": "consensus_crossing_without_thesis",
        "score": score.total,
        "config_version": config_version,
    });
    if let Err(e) = bus
        .publish(
            subjects::DISCOVERY_CONFIRMED,
            payload.to_string().as_bytes(),
        )
        .await
    {
        warn!(symbol = %score.symbol, error = %e, "publish cognition kickoff failed (non-fatal)");
    }
    Ok(())
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
    bus.ensure_stream(subjects::STREAM_THESIS, &["thesis.*"])
        .await?;
    bus.ensure_stream(subjects::STREAM_MARKET, &["regime.*", "discovery.*"])
        .await?;
    info!(
        interval_secs = interval.as_secs(),
        "consensus service started"
    );
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crossing_without_open_thesis_does_not_emit_thesis_event() {
        assert_eq!(thesis_crossing_topic(false, false), None);
        assert_eq!(thesis_crossing_topic(false, true), None);
    }

    #[test]
    fn crossing_with_open_thesis_routes_to_lifecycle_subjects() {
        assert_eq!(
            thesis_crossing_topic(true, false),
            Some(subjects::THESIS_UPDATED)
        );
        assert_eq!(
            thesis_crossing_topic(true, true),
            Some(subjects::THESIS_FULFILLED)
        );
    }

    #[test]
    fn missing_thesis_attention_copy_names_the_gap() {
        let (title, reason) = missing_thesis_attention_text("CRDO", 63.457);

        assert_eq!(title, "CRDO needs a thesis after consensus crossing");
        assert!(reason.contains("Consensus score 63.5"));
        assert!(reason.contains("has no open thesis"));
    }
}
