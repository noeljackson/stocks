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
use serde_json::{Value, json};
use sqlx::{Row, postgres::PgPool};
use tracing::{info, warn};

use super::scoring::{
    self, CalibrationSummary, ParentThemeCalibration, TechnicalTimingCalibration,
};
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
        let h_actionable = self
            .bus
            .consume(
                subjects::STREAM_THESIS,
                "reflection-actionable",
                subjects::THESIS_ACTIONABLE,
                move |msg| {
                    let pool = pool_a.clone();
                    async move { record_prediction(&pool, msg).await }
                },
            )
            .await?;

        let pool_f = self.pool.clone();
        let h_fulfilled = self
            .bus
            .consume(
                subjects::STREAM_THESIS,
                "reflection-fulfilled",
                subjects::THESIS_FULFILLED,
                move |msg| {
                    let pool = pool_f.clone();
                    async move { record_outcome(&pool, msg, true).await }
                },
            )
            .await?;

        let pool_i = self.pool.clone();
        let h_invalidated = self
            .bus
            .consume(
                subjects::STREAM_THESIS,
                "reflection-invalidated",
                subjects::THESIS_INVALIDATED,
                move |msg| {
                    let pool = pool_i.clone();
                    async move { record_outcome(&pool, msg, false).await }
                },
            )
            .await?;

        info!("reflection: 3 durable consumers bound");
        Ok(vec![h_actionable, h_fulfilled, h_invalidated])
    }
}

async fn record_prediction(pool: &PgPool, msg: jetstream::Message) -> Result<()> {
    let Ok(event) = serde_json::from_slice::<ThesisActionable>(&msg.payload) else {
        warn!("dropping malformed thesis.actionable");
        return Ok(());
    };
    let parsed_uuid = event
        .thesis_id
        .as_deref()
        .and_then(|s| uuid::Uuid::parse_str(s).ok());
    // Look up the thesis row when we have a parseable id. Source-of-truth for
    // forecast + horizon is the thesis, not the actionable payload (which
    // historically didn't carry forecast at all).
    let (thesis_uuid, forecast_from_db, horizon_days) = match parsed_uuid {
        Some(u) => {
            let row = sqlx::query("SELECT forecast FROM thesis WHERE thesis_id = $1")
                .bind(u)
                .fetch_optional(pool)
                .await
                .unwrap_or(None);
            match row {
                Some(r) => {
                    let f: serde_json::Value =
                        r.try_get("forecast").unwrap_or(serde_json::Value::Null);
                    let days = f.get("horizon_days").and_then(|v| v.as_i64());
                    (Some(u), Some(f), days)
                }
                None => (None, None, None),
            }
        }
        None => (None, None, None),
    };
    // Prefer DB forecast (authoritative); fall back to event-supplied forecast
    // for smoke tests / replay that publish a synthetic actionable.
    let forecast = forecast_from_db
        .or(event.forecast)
        .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
    let parent_brain_links = match event.symbol.as_deref() {
        Some(symbol) => load_parent_brain_links(pool, symbol)
            .await
            .context("load parent brain links")?,
        None => Vec::new(),
    };
    let claim = claim_with_parent_brain_links(forecast, parent_brain_links);
    let horizon_at = horizon_days.map(|d| Utc::now() + chrono::Duration::days(d));
    if let Err(e) = sqlx::query(
        r#"INSERT INTO prediction (thesis_id, symbol, kind, claim, horizon_at)
           VALUES ($1, $2, 'direction', $3, $4)"#,
    )
    .bind(thesis_uuid)
    .bind(event.symbol.as_deref())
    .bind(&claim)
    .bind(horizon_at)
    .execute(pool)
    .await
    {
        warn!(error = %e, thesis = ?event.thesis_id, "insert prediction failed");
        return Err(e).context("insert prediction");
    }
    info!(thesis_id = ?event.thesis_id, horizon_at = ?horizon_at, "prediction recorded");
    Ok(())
}

fn claim_with_parent_brain_links(mut claim: Value, parent_brain_links: Vec<Value>) -> Value {
    if parent_brain_links.is_empty() {
        return claim;
    }
    if let Some(obj) = claim.as_object_mut() {
        obj.insert(
            "parent_brain_theses".to_string(),
            Value::Array(parent_brain_links),
        );
        return claim;
    }
    json!({
        "forecast": claim,
        "parent_brain_theses": parent_brain_links,
    })
}

async fn load_parent_brain_links(pool: &PgPool, symbol: &str) -> Result<Vec<Value>> {
    let rows = sqlx::query(
        r#"SELECT bt.id, bt.scope, bt.key, bt.name, bt.state, bt.direction,
                  bt.version, bt.last_evaluated_at,
                  btt.role, btt.rationale, btt.conviction
             FROM brain_thesis bt
        LEFT JOIN brain_thesis_ticker btt
               ON btt.brain_thesis_id = bt.id
              AND btt.symbol = $1
            WHERE bt.active = true
              AND (bt.scope = 'macro' OR btt.symbol IS NOT NULL)
         ORDER BY CASE bt.scope WHEN 'macro' THEN 0 WHEN 'sector' THEN 1 ELSE 2 END,
                  COALESCE(btt.conviction, 0) DESC,
                  bt.name
            LIMIT 12"#,
    )
    .bind(symbol)
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(|r| {
            let id: uuid::Uuid = r.try_get("id")?;
            let scope: String = r.try_get("scope")?;
            let role = r.try_get::<Option<String>, _>("role")?.unwrap_or_else(|| {
                if scope == "macro" {
                    "macro_context"
                } else {
                    "linked"
                }
                .to_string()
            });
            let last_evaluated_at: Option<DateTime<Utc>> = r.try_get("last_evaluated_at")?;
            Ok::<Value, sqlx::Error>(json!({
                "id": id,
                "scope": scope,
                "key": r.try_get::<String, _>("key")?,
                "name": r.try_get::<String, _>("name")?,
                "state": r.try_get::<String, _>("state")?,
                "direction": r.try_get::<String, _>("direction")?,
                "version": r.try_get::<i32, _>("version")?,
                "last_evaluated_at": last_evaluated_at,
                "symbol": symbol,
                "role": role,
                "rationale": r.try_get::<Option<String>, _>("rationale")?,
                "conviction": r.try_get::<Option<i32>, _>("conviction")?,
            }))
        })
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("decode parent brain links")
}

async fn record_outcome(pool: &PgPool, msg: jetstream::Message, realised_up: bool) -> Result<()> {
    let Ok(event) = serde_json::from_slice::<ThesisOutcome>(&msg.payload) else {
        warn!("dropping malformed thesis outcome");
        return Ok(());
    };
    // Mirror the record_prediction FK check: only attempt thesis_id match if
    // the row actually exists. Otherwise fall back to symbol-based lookup so
    // smoke / replay flows still record outcomes.
    let parsed_uuid = event
        .thesis_id
        .as_deref()
        .and_then(|s| uuid::Uuid::parse_str(s).ok());
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
    .map(|r| {
        (
            r.try_get("brier").unwrap_or(0.0),
            r.try_get("lead_time").unwrap_or(0.0),
        )
    })
    .collect();

    let briers: Vec<f64> = scored.iter().map(|(b, _)| *b).collect();
    let lead_times: Vec<f64> = scored.iter().map(|(_, l)| *l).collect();
    let parent_themes = parent_theme_calibration(pool, lookback_days).await?;
    let technical_timing = technical_timing_calibration(pool, lookback_days).await?;

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
        parent_themes,
        technical_timing,
    })
}

async fn parent_theme_calibration(
    pool: &PgPool,
    lookback_days: i64,
) -> Result<Vec<ParentThemeCalibration>> {
    sqlx::query(
        r#"SELECT link->>'key' AS key,
                  COALESCE(link->>'name', link->>'key') AS name,
                  COALESCE(link->>'scope', 'theme') AS scope,
                  COALESCE(link->>'role', 'linked') AS role,
                  COUNT(*) AS predictions_total,
                  COUNT(o.outcome_id) AS outcomes_scored,
                  AVG(o.score::float8) AS mean_brier,
                  AVG(COALESCE((o.observed->>'lead_time_days')::float8, 0.0))
                      FILTER (WHERE o.outcome_id IS NOT NULL) AS mean_lead_time_days
             FROM prediction p
       CROSS JOIN LATERAL jsonb_array_elements(
                    CASE
                      WHEN jsonb_typeof(p.claim->'parent_brain_theses') = 'array'
                      THEN p.claim->'parent_brain_theses'
                      ELSE '[]'::jsonb
                    END
                  ) AS link
        LEFT JOIN outcome o ON o.prediction_id = p.prediction_id
            WHERE p.at > now() - ($1 || ' days')::interval
         GROUP BY key, name, scope, role
         ORDER BY COUNT(o.outcome_id) DESC, COUNT(*) DESC, name, role
            LIMIT 20"#,
    )
    .bind(lookback_days.to_string())
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|r| {
        Ok::<ParentThemeCalibration, sqlx::Error>(ParentThemeCalibration {
            key: r.try_get("key")?,
            name: r.try_get("name")?,
            scope: r.try_get("scope")?,
            role: r.try_get("role")?,
            predictions_total: r.try_get("predictions_total")?,
            outcomes_scored: r.try_get("outcomes_scored")?,
            mean_brier: r.try_get("mean_brier").ok(),
            mean_lead_time_days: r.try_get("mean_lead_time_days").ok(),
        })
    })
    .collect::<std::result::Result<Vec<_>, _>>()
    .context("decode parent theme calibration")
}

async fn technical_timing_calibration(
    pool: &PgPool,
    lookback_days: i64,
) -> Result<Vec<TechnicalTimingCalibration>> {
    sqlx::query(
        r#"SELECT technical_state,
                  setup_kind,
                  entry_stance,
                  benchmark_symbol,
                  COUNT(*) AS observations_total,
                  COUNT(evaluated_at) AS outcomes_scored,
                  AVG(forward_return_pct::float8)
                      FILTER (WHERE evaluated_at IS NOT NULL) AS mean_forward_return_pct,
                  AVG(max_drawdown_pct::float8)
                      FILTER (WHERE evaluated_at IS NOT NULL) AS mean_max_drawdown_pct,
                  AVG(benchmark_return_pct::float8)
                      FILTER (WHERE evaluated_at IS NOT NULL) AS mean_benchmark_return_pct,
                  AVG(excess_return_pct::float8)
                      FILTER (WHERE evaluated_at IS NOT NULL) AS mean_excess_return_pct,
                  AVG(CASE WHEN forward_return_pct > 0 THEN 1.0 ELSE 0.0 END)
                      FILTER (WHERE evaluated_at IS NOT NULL) AS positive_return_rate,
                  AVG(CASE WHEN excess_return_pct > 0 THEN 1.0 ELSE 0.0 END)
                      FILTER (WHERE evaluated_at IS NOT NULL AND excess_return_pct IS NOT NULL)
                      AS outperform_rate
             FROM technical_timing_observation
            WHERE observed_at > now() - ($1 || ' days')::interval
         GROUP BY technical_state, setup_kind, entry_stance, benchmark_symbol
         ORDER BY COUNT(evaluated_at) DESC,
                  COUNT(*) DESC,
                  technical_state,
                  benchmark_symbol
            LIMIT 50"#,
    )
    .bind(lookback_days.to_string())
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|r| {
        Ok::<TechnicalTimingCalibration, sqlx::Error>(TechnicalTimingCalibration {
            technical_state: r.try_get("technical_state")?,
            setup_kind: r.try_get("setup_kind")?,
            entry_stance: r.try_get("entry_stance")?,
            benchmark_symbol: r.try_get("benchmark_symbol")?,
            observations_total: r.try_get("observations_total")?,
            outcomes_scored: r.try_get("outcomes_scored")?,
            mean_forward_return_pct: r.try_get("mean_forward_return_pct").ok(),
            mean_max_drawdown_pct: r.try_get("mean_max_drawdown_pct").ok(),
            mean_benchmark_return_pct: r.try_get("mean_benchmark_return_pct").ok(),
            mean_excess_return_pct: r.try_get("mean_excess_return_pct").ok(),
            positive_return_rate: r.try_get("positive_return_rate").ok(),
            outperform_rate: r.try_get("outperform_rate").ok(),
        })
    })
    .collect::<std::result::Result<Vec<_>, _>>()
    .context("decode technical timing calibration")
}

fn median(xs: &[f64]) -> Option<f64> {
    if xs.is_empty() {
        return None;
    }
    let mut s = xs.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = s.len() / 2;
    Some(if s.len() % 2 == 0 {
        (s[mid - 1] + s[mid]) / 2.0
    } else {
        s[mid]
    })
}

/// Long-running entry point.
pub async fn run(pool: PgPool, bus: Bus) -> Result<()> {
    let svc = Service { pool, bus };
    let _handles = svc.start().await?;
    // Hold consumers alive until ctrl-c.
    let _ = tokio::signal::ctrl_c().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parent_brain_links_are_added_without_losing_forecast_fields() {
        let claim = claim_with_parent_brain_links(
            json!({"direction": "up", "prob_up": 0.7}),
            vec![json!({"key": "ai_compute_infrastructure", "role": "supplier"})],
        );

        assert_eq!(claim["direction"], "up");
        assert_eq!(claim["prob_up"], 0.7);
        assert_eq!(
            claim["parent_brain_theses"][0]["key"],
            "ai_compute_infrastructure"
        );
        assert_eq!(claim["parent_brain_theses"][0]["role"], "supplier");
    }

    #[test]
    fn non_object_claims_are_wrapped_with_parent_brain_links() {
        let claim = claim_with_parent_brain_links(
            json!("synthetic"),
            vec![json!({"key": "macro_regime", "role": "macro_context"})],
        );

        assert_eq!(claim["forecast"], "synthetic");
        assert_eq!(claim["parent_brain_theses"][0]["key"], "macro_regime");
    }
}
