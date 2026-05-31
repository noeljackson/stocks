//! Risk overlay service: durable consumer on thesis.actionable, loads
//! positions from Postgres, publishes risk.veto / risk.warning.

use std::sync::Arc;

use anyhow::Result;
use chrono::Utc;
use serde::Deserialize;
use tracing::{info, warn};

use super::{Config, Intent, Portfolio, derive_portfolio, evaluate};
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

// Honest fallback when the operator hasn't set `portfolio_settings` yet.
// Marked explicitly so risk verdicts emitted during this state can be
// recognized as demo-mode (we also log a warn! at every evaluation).
const UNCONFIGURED_PORTFOLIO: Portfolio = Portfolio {
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
    let positions = store.open_positions_for_risk().await?;
    let settings = store.portfolio_settings().await?;
    let realized_pnl = store.realized_pnl_total().await.unwrap_or(0.0);

    let (portfolio, is_demo) = match derive_portfolio(settings, &positions, realized_pnl) {
        Some(p) => (p, false),
        None => {
            warn!(
                "portfolio_settings.account_size_usd unset — risk overlay running on \
                 DEMO portfolio (100k notional, 50% cash). Set via PUT /api/portfolio."
            );
            (UNCONFIGURED_PORTFOLIO, true)
        }
    };

    let intent = Intent {
        symbol: a.symbol.clone(),
        cluster: a.cluster.clone(),
        instrument: a.instrument.clone(),
        delta_notional: a.delta_notional,
        premium_at_risk: a.premium_at_risk,
    };
    let decision = evaluate(&intent, &positions, portfolio, &cfg);

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
        "portfolio_demo": is_demo,
        "portfolio": {
            "total_value": portfolio.total_value,
            "cash_pct":    portfolio.cash_pct,
            "drawdown_pct": portfolio.drawdown_pct,
        },
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

