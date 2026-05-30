//! Regime classifier service: subscribes to ingest.macro via a durable
//! consumer, maintains an in-memory snapshot, persists market_state on every
//! update, publishes regime.state.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use chrono::Utc;
use serde::Deserialize;
use sqlx::Row;
use tokio::sync::Mutex;
use tracing::{error, info, warn};

use super::{Config, Outcome, classify, fred_series_to_indicator};
use crate::platform::bus::{Bus, ConsumerHandle};
use crate::platform::domain::Regime;
use crate::platform::store::Store;
use crate::platform::subjects;

#[derive(Deserialize)]
struct MacroObs {
    series: String,
    date: String,
    value: String,
}

#[derive(Default)]
struct State {
    snap: HashMap<String, f64>,
    last_regime: Option<Regime>,
    last_capitulation: bool,
}

/// Run the regime classifier service.
pub async fn run(store: Store, bus: Bus) -> Result<ConsumerHandle> {
    bus.ensure_stream(subjects::STREAM_MARKET, &["regime.*", "discovery.*"])
        .await?;

    let state = Arc::new(Mutex::new(State::default()));
    warm_start(&store, &state).await.unwrap_or_else(|e| {
        warn!(error = %e, "warm-start failed; starting empty");
    });

    let store = Arc::new(store);
    let bus = Arc::new(bus);
    let bus_consume = bus.clone();

    let handle = bus_consume
        .consume(subjects::STREAM_INGEST, "regime-classifier", subjects::INGEST_MACRO, {
            let store = store.clone();
            let bus = bus.clone();
            let state = state.clone();
            move |msg| {
                let store = store.clone();
                let bus = bus.clone();
                let state = state.clone();
                async move { on_macro(&store, &bus, &state, &msg.payload).await }
            }
        })
        .await?;
    info!(
        stream = subjects::STREAM_INGEST,
        filter = subjects::INGEST_MACRO,
        "regime classifier consuming"
    );
    Ok(handle)
}

async fn on_macro(
    store: &Store,
    bus: &Bus,
    state: &Mutex<State>,
    data: &[u8],
) -> Result<()> {
    let Ok(obs) = serde_json::from_slice::<MacroObs>(data) else {
        warn!("dropping malformed macro message");
        return Ok(());
    };
    if obs.series.is_empty() {
        warn!("macro missing series; dropping");
        return Ok(());
    }
    let Ok(val) = obs.value.parse::<f64>() else {
        warn!(series = %obs.series, value = %obs.value, "non-numeric macro value; dropping");
        return Ok(());
    };
    let name = fred_series_to_indicator(&obs.series).to_string();

    // Load config every message (cheap; lets config bumps take effect immediately).
    let (cfg_json, cfg_ver) = store.active_config("regime").await?;
    let cfg: Config = serde_json::from_value(cfg_json)?;

    let outcome = {
        let mut s = state.lock().await;
        s.snap.insert(name.clone(), val);
        let snap = s.snap.clone();
        let prev_regime = s.last_regime;
        let prev_cap = s.last_capitulation;
        let outcome: Outcome = classify(&cfg, &snap);
        s.last_regime = Some(outcome.regime);
        s.last_capitulation = outcome.capitulation;
        OutcomeWithChange {
            outcome,
            changed: prev_regime != Some(s.last_regime.unwrap())
                || prev_cap != s.last_capitulation,
        }
    };

    let as_of = Utc::now();
    let indicators_json = serde_json::to_value(&outcome.outcome.indicators)?;
    if let Err(e) = store
        .upsert_market_state(
            as_of,
            outcome.outcome.regime.as_str(),
            outcome.outcome.capitulation,
            &indicators_json,
            cfg_ver,
        )
        .await
    {
        error!(error = %e, "persist market_state");
        return Err(e);
    }
    let publish_body = serde_json::json!({
        "as_of": as_of,
        "regime": outcome.outcome.regime,
        "capitulation": outcome.outcome.capitulation,
        "matched": outcome.outcome.matched,
        "config_version": cfg_ver,
        "trigger": {
            "series": obs.series, "name": name, "value": val, "date": obs.date,
        },
    });
    bus.publish(subjects::REGIME_STATE, publish_body.to_string().as_bytes())
        .await?;
    if outcome.changed {
        info!(
            regime = %outcome.outcome.regime.as_str(),
            capitulation = outcome.outcome.capitulation,
            "regime change"
        );
        if outcome.outcome.capitulation {
            bus.publish(
                subjects::REGIME_CAPITULATION,
                publish_body.to_string().as_bytes(),
            )
            .await?;
        }
    }
    Ok(())
}

struct OutcomeWithChange {
    outcome: Outcome,
    changed: bool,
}

/// Rebuilds the snapshot from the latest FRED observation per series already
/// in `ingest_event` so a restart doesn't lose state until the next poll.
async fn warm_start(store: &Store, state: &Mutex<State>) -> Result<()> {
    let rows = sqlx::query(
        r#"SELECT DISTINCT ON (payload->>'series') payload
             FROM ingest_event
            WHERE source = 'fred'
         ORDER BY payload->>'series', ingested_at DESC"#,
    )
    .fetch_all(&store.pool)
    .await?;
    let mut s = state.lock().await;
    for row in rows {
        let payload: serde_json::Value = row.try_get("payload")?;
        let Ok(obs) = serde_json::from_value::<MacroObs>(payload) else {
            continue;
        };
        let Ok(v) = obs.value.parse::<f64>() else {
            continue;
        };
        let name = fred_series_to_indicator(&obs.series).to_string();
        s.snap.insert(name, v);
    }
    Ok(())
}
