//! Reflection service — two durable JetStream consumers on THESIS.
//! - thesis.actionable → INSERT prediction row
//! - thesis.fulfilled  → INSERT outcome row + compute Brier + lead-time
//! - thesis.invalidated → INSERT outcome row marking refutation
//!
//! Plus calibration_summary() that the gateway's /api/calibration consumes.

use anyhow::{Context, Result};
use async_nats::jetstream;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use sqlx::{Row, postgres::PgPool};
use tracing::{info, warn};

use super::scoring::{self, CalibrationSummary};
use crate::platform::bus::{Bus, ConsumerHandle};
use crate::platform::subjects;

// ---------- consumed event shapes ----------

#[derive(Debug, Clone, Deserialize)]
struct ThesisActionable {
    #[serde(default)]
    thesis_id: Option<String>,
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    forecast: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
struct ThesisOutcome {
    #[serde(default)]
    thesis_id: Option<String>,
    /// Used as fallback prediction lookup when thesis_id doesn't resolve
    /// (smoke tests / replayed messages).
    #[serde(default)]
    symbol: Option<String>,
    /// Consensus crossing fields (set when this event came from the consensus
    /// service); ignored when the publisher is the thesis engine directly.
    #[serde(default)]
    score: Option<f64>,
    #[serde(default)]
    exit_crossed: Option<bool>,
}

// ---------- service ----------

pub struct Service {
    pub pool: PgPool,
    pub bus: Bus,
}

impl Service {
    /// Bind both durable consumers; return their handles. Drop the handles
    /// to stop.
    pub async fn start(&self) -> Result<Vec<ConsumerHandle>> {
        self.bus
            .ensure_stream(subjects::STREAM_THESIS, &["thesis.*"])
            .await?;

        let pool_a = self.pool.clone();
        let h_actionable = self.bus.consume(
            subjects::STREAM_THESIS,
            "reflection-actionable",
            subjects::THESIS_ACTIONABLE,
            move |msg| {
                let pool = pool_a.clone();
                async move { record_prediction(&pool, msg).await }
            },
        ).await?;

        let pool_f = self.pool.clone();
        let h_fulfilled = self.bus.consume(
            subjects::STREAM_THESIS,
            "reflection-fulfilled",
            subjects::THESIS_FULFILLED,
            move |msg| {
                let pool = pool_f.clone();
                async move { record_outcome(&pool, msg, true).await }
            },
        ).await?;

        let pool_i = self.pool.clone();
        let h_invalidated = self.bus.consume(
            subjects::STREAM_THESIS,
            "reflection-invalidated",
            subjects::THESIS_INVALIDATED,
            move |msg| {
                let pool = pool_i.clone();
                async move { record_outcome(&pool, msg, false).await }
            },
        ).await?;

        info!("reflection: 3 durable consumers bound");
        Ok(vec![h_actionable, h_fulfilled, h_invalidated])
    }
}

async fn record_prediction(pool: &PgPool, msg: jetstream::Message) -> Result<()> {
    let Ok(event) = serde_json::from_slice::<ThesisActionable>(&msg.payload) else {
        warn!("dropping malformed thesis.actionable");
        return Ok(());
    };
    let parsed_uuid = event.thesis_id.as_deref().and_then(|s| uuid::Uuid::parse_str(s).ok());
    // Only set the FK when the thesis actually exists — otherwise persist
    // the prediction with NULL thesis_id so smoke tests + replay don't lose
    // data. Real-production actionables always reference an existing thesis.
    let thesis_uuid = match parsed_uuid {
        Some(u) => {
            let exists: Option<uuid::Uuid> = sqlx::query_scalar(
                "SELECT thesis_id FROM thesis WHERE thesis_id = $1",
            )
            .bind(u)
            .fetch_optional(pool)
            .await
            .unwrap_or(None);
            exists
        }
        None => None,
    };
    // Default to an empty object so the NOT NULL constraint holds when the
    // engine fires actionable without a forecast (it shouldn't, but be safe).
    let forecast = event
        .forecast
        .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
    if let Err(e) = sqlx::query(
        r#"INSERT INTO prediction (thesis_id, symbol, kind, claim)
           VALUES ($1, $2, 'direction', $3)"#,
    )
    .bind(thesis_uuid)
    .bind(event.symbol.as_deref())
    .bind(&forecast)
    .execute(pool)
    .await
    {
        warn!(error = %e, thesis = ?event.thesis_id, "insert prediction failed");
        return Err(e).context("insert prediction");
    }
    info!(thesis_id = ?event.thesis_id, "prediction recorded");
    Ok(())
}

async fn record_outcome(pool: &PgPool, msg: jetstream::Message, realised_up: bool) -> Result<()> {
    let Ok(event) = serde_json::from_slice::<ThesisOutcome>(&msg.payload) else {
        warn!("dropping malformed thesis outcome");
        return Ok(());
    };
    // Mirror the record_prediction FK check: only attempt thesis_id match if
    // the row actually exists. Otherwise fall back to symbol-based lookup so
    // smoke / replay flows still record outcomes.
    let parsed_uuid = event.thesis_id.as_deref().and_then(|s| uuid::Uuid::parse_str(s).ok());
    let thesis_uuid: Option<uuid::Uuid> = match parsed_uuid {
        Some(u) => sqlx::query_scalar("SELECT thesis_id FROM thesis WHERE thesis_id = $1")
            .bind(u)
            .fetch_optional(pool)
            .await
            .unwrap_or(None),
        None => None,
    };

    // Pull the most-recent open prediction. Match strategy:
    // 1. If thesis_id present + resolves to a thesis row, match by that.
    // 2. Otherwise (synthetic / FK-missing), match by symbol as fallback.
    let pred = sqlx::query(
        r#"SELECT p.prediction_id, p.claim, p.at
             FROM prediction p
            LEFT JOIN outcome o ON o.prediction_id = p.prediction_id
            WHERE o.outcome_id IS NULL
              AND (
                    ($1::uuid IS NOT NULL AND p.thesis_id = $1)
                 OR ($1::uuid IS NULL AND p.symbol = $2)
              )
         ORDER BY p.at DESC
            LIMIT 1"#,
    )
    .bind(thesis_uuid)
    .bind(event.symbol.as_deref())
    .fetch_optional(pool)
    .await?;
    let Some(p) = pred else {
        info!(
            thesis_id = ?event.thesis_id,
            resolved_uuid = ?thesis_uuid,
            symbol = ?event.symbol,
            "no open prediction to score for this outcome — skipping"
        );
        return Ok(());
    };
    let prediction_id: uuid::Uuid = p.try_get("prediction_id")?;
    let claim: serde_json::Value = p.try_get("claim")?;
    let alert_at: DateTime<Utc> = p.try_get("at")?;

    // Pull a directional prob from the claim; defaults to 0.5 when absent.
    // The thesis engine writes {"direction":"up","magnitude_rough":"..."}
    // — for v1 we treat "up" as 0.7 prob, "down" as 0.3, "neutral" as 0.5.
    // (When the engine starts emitting an explicit prob_up field we'll use that.)
    let pred_prob_up = claim
        .get("prob_up")
        .and_then(|v| v.as_f64())
        .unwrap_or_else(|| match claim.get("direction").and_then(|v| v.as_str()) {
            Some("up") => 0.7,
            Some("down") => 0.3,
            _ => 0.5,
        });
    let brier = scoring::brier(pred_prob_up, realised_up);
    let now = Utc::now();
    let lead_time = scoring::lead_time_days(alert_at, now);

    let observed = serde_json::json!({
        "realised_up": realised_up,
        "consensus_score": event.score,
        "exit_crossed": event.exit_crossed,
        "lead_time_days": lead_time,
    });
    sqlx::query(
        r#"INSERT INTO outcome (prediction_id, observed, observed_at, score)
           VALUES ($1, $2, $3, $4)"#,
    )
    .bind(prediction_id)
    .bind(observed) // sqlx encodes serde_json::Value as jsonb directly
    .bind(now)
    .bind(brier)
    .execute(pool)
    .await
    .context("insert outcome")?;
    info!(
        prediction_id = %prediction_id,
        brier = brier,
        lead_time_days = lead_time,
        realised_up = realised_up,
        "outcome recorded"
    );
    Ok(())
}

/// Compute rolling calibration summary over the last `lookback_days`. Used
/// by /api/calibration in the gateway.
pub async fn calibration_summary(pool: &PgPool, lookback_days: i64) -> Result<CalibrationSummary> {
    let pred_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM prediction WHERE at > now() - ($1 || ' days')::interval",
    )
    .bind(lookback_days.to_string())
    .fetch_one(pool)
    .await
    .unwrap_or(0);

    let scored: Vec<(f64, f64)> = sqlx::query(
        r#"SELECT o.score::float8 AS brier,
                  COALESCE((o.observed->>'lead_time_days')::float8, 0.0) AS lead_time
             FROM outcome o
             JOIN prediction p ON p.prediction_id = o.prediction_id
            WHERE o.observed_at > now() - ($1 || ' days')::interval"#,
    )
    .bind(lookback_days.to_string())
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|r| (r.try_get("brier").unwrap_or(0.0), r.try_get("lead_time").unwrap_or(0.0)))
    .collect();

    let briers: Vec<f64> = scored.iter().map(|(b, _)| *b).collect();
    let lead_times: Vec<f64> = scored.iter().map(|(_, l)| *l).collect();

    Ok(CalibrationSummary {
        predictions_total: pred_count,
        outcomes_scored: scored.len() as i64,
        mean_brier: scoring::mean_brier(&briers),
        mean_lead_time_days: if lead_times.is_empty() {
            None
        } else {
            Some(lead_times.iter().sum::<f64>() / lead_times.len() as f64)
        },
        median_lead_time_days: median(&lead_times),
    })
}

fn median(xs: &[f64]) -> Option<f64> {
    if xs.is_empty() {
        return None;
    }
    let mut s = xs.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = s.len() / 2;
    Some(if s.len() % 2 == 0 { (s[mid - 1] + s[mid]) / 2.0 } else { s[mid] })
}

/// Long-running entry point.
pub async fn run(pool: PgPool, bus: Bus) -> Result<()> {
    let svc = Service { pool, bus };
    let _handles = svc.start().await?;
    // Hold consumers alive until ctrl-c.
    let _ = tokio::signal::ctrl_c().await;
    Ok(())
}
