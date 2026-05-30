//! Risk overlay service: durable consumer on thesis.actionable, loads
//! positions from Postgres, publishes risk.veto / risk.warning.

use std::sync::Arc;

use anyhow::Result;
use chrono::Utc;
use serde::Deserialize;
use sqlx::Row;
use tracing::{info, warn};

use super::{Config, Intent, Portfolio, Position, evaluate};
use crate::platform::bus::{Bus, ConsumerHandle};
use crate::platform::store::Store;
use crate::platform::subjects;

#[derive(Deserialize, Default)]
struct Actionable {
    #[serde(default)]
    thesis_id: String,
    #[serde(default)]
    symbol: String,
    #[serde(default)]
    cluster: String,
    #[serde(default)]
    instrument: String,
    #[serde(default)]
    delta_notional: f64,
    #[serde(default)]
    premium_at_risk: f64,
}

// v0 portfolio defaults — replaced with live IBKR integration in a later phase.
const DEFAULT_PORTFOLIO: Portfolio = Portfolio {
    total_value: 100_000.0,
    cash_pct: 50.0,
    drawdown_pct: 0.0,
};

pub async fn run(store: Store, bus: Bus) -> Result<ConsumerHandle> {
    bus.ensure_stream(subjects::STREAM_DECISIONS, &["risk.*", "decision.*"])
        .await?;

    let store = Arc::new(store);
    let bus = Arc::new(bus);
    let bus_consume = bus.clone();
    let handle = bus_consume
        .consume(
            subjects::STREAM_THESIS,
            "risk-overlay",
            subjects::THESIS_ACTIONABLE,
            {
                let store = store.clone();
                let bus = bus.clone();
                move |msg| {
                    let store = store.clone();
                    let bus = bus.clone();
                    async move { on_actionable(&store, &bus, &msg.payload).await }
                }
            },
        )
        .await?;
    info!(
        stream = subjects::STREAM_THESIS,
        filter = subjects::THESIS_ACTIONABLE,
        "risk overlay consuming"
    );
    Ok(handle)
}

async fn on_actionable(store: &Store, bus: &Bus, data: &[u8]) -> Result<()> {
    let Ok(a) = serde_json::from_slice::<Actionable>(data) else {
        warn!("dropping malformed thesis.actionable");
        return Ok(());
    };
    let (cfg_json, cfg_ver) = store.active_config("risk").await?;
    let cfg: Config = serde_json::from_value(cfg_json)?;
    let positions = load_open_positions(store).await?;

    let intent = Intent {
        symbol: a.symbol.clone(),
        cluster: a.cluster.clone(),
        instrument: a.instrument.clone(),
        delta_notional: a.delta_notional,
        premium_at_risk: a.premium_at_risk,
    };
    let decision = evaluate(&intent, &positions, DEFAULT_PORTFOLIO, &cfg);

    if !decision.veto && decision.warnings.is_empty() {
        return Ok(());
    }

    let payload = serde_json::json!({
        "thesis_id": a.thesis_id,
        "symbol":    a.symbol,
        "veto":      decision.veto,
        "reasons":   decision.reasons,
        "warnings":  decision.warnings,
        "size_mult": decision.size_mult,
        "config_version": cfg_ver,
        "at":        Utc::now(),
    });
    let subject = if decision.veto { subjects::RISK_VETO } else { subjects::RISK_WARNING };
    bus.publish(subject, payload.to_string().as_bytes()).await?;
    info!(
        subject = subject,
        symbol = %a.symbol,
        veto = decision.veto,
        "risk verdict"
    );
    Ok(())
}

async fn load_open_positions(store: &Store) -> Result<Vec<Position>> {
    // Cast NUMERIC → float8 in SQL so sqlx hands us a primitive f64 directly,
    // avoiding the `bigdecimal` feature pull-in.
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
    .fetch_all(&store.pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(Position {
                symbol: row.try_get("symbol")?,
                cluster: row.try_get("cluster")?,
                instrument: row.try_get("instrument")?,
                delta_notional: row.try_get::<f64, _>("delta_notional")?,
                premium_at_risk: row.try_get::<f64, _>("premium_at_risk")?,
            })
        })
        .collect()
}
