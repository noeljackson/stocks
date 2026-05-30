//! Goalpost detector service: subscribes to thesis.updated, loads
//! immutable_original + current invalidation_conditions from Postgres,
//! persists the diff into thesis_version_history, emits risk.warning
//! (kind=goalpost_moved) when weakened or needs_review.

use std::sync::Arc;

use anyhow::Result;
use chrono::Utc;
use serde::Deserialize;
use sqlx::Row;
use tracing::{info, warn};
use uuid::Uuid;

use super::{Condition, analyze};
use crate::platform::bus::{Bus, ConsumerHandle};
use crate::platform::store::Store;
use crate::platform::subjects;

#[derive(Deserialize)]
struct UpdatedEvent {
    thesis_id: String,
    #[serde(default)]
    version: i32,
    #[serde(default)]
    rationale: String,
}

pub async fn run(store: Store, bus: Bus) -> Result<ConsumerHandle> {
    bus.ensure_stream(subjects::STREAM_THESIS, &["thesis.*"])
        .await?;
    let store = Arc::new(store);
    let bus = Arc::new(bus);
    let bus_consume = bus.clone();
    let handle = bus_consume
        .consume(
            subjects::STREAM_THESIS,
            "goalpost-detector",
            subjects::THESIS_UPDATED,
            {
                let store = store.clone();
                let bus = bus.clone();
                move |msg| {
                    let store = store.clone();
                    let bus = bus.clone();
                    async move { on_updated(&store, &bus, &msg.payload).await }
                }
            },
        )
        .await?;
    info!(
        stream = subjects::STREAM_THESIS,
        filter = subjects::THESIS_UPDATED,
        "goalpost detector consuming"
    );
    Ok(handle)
}

async fn on_updated(store: &Store, bus: &Bus, data: &[u8]) -> Result<()> {
    let Ok(ev) = serde_json::from_slice::<UpdatedEvent>(data) else {
        warn!("dropping malformed thesis.updated");
        return Ok(());
    };
    let Ok(thesis_uuid) = Uuid::parse_str(&ev.thesis_id) else {
        warn!(thesis_id = %ev.thesis_id, "thesis_id is not a UUID; dropping");
        return Ok(());
    };
    let (orig, curr) = load_conditions(store, thesis_uuid).await?;
    let report = analyze(&orig, &curr);

    let diff_body = serde_json::json!({
        "dropped":     report.dropped,
        "loosened":    report.loosened,
        "added":       report.added,
        "needs_review": report.needs_review,
        "reasons":     report.reasons,
    });
    record_version(
        store,
        thesis_uuid,
        ev.version,
        &diff_body,
        &ev.rationale,
        report.weakened,
    )
    .await?;

    if report.weakened || report.needs_review {
        let payload = serde_json::json!({
            "thesis_id":    ev.thesis_id,
            "version":      ev.version,
            "kind":         "goalpost_moved",
            "weakened":     report.weakened,
            "needs_review": report.needs_review,
            "dropped":      report.dropped,
            "loosened":     report.loosened,
            "added":        report.added,
            "reasons":      report.reasons,
            "at":           Utc::now(),
        });
        bus.publish(subjects::RISK_WARNING, payload.to_string().as_bytes())
            .await?;
        warn!(
            thesis_id = %ev.thesis_id,
            version = ev.version,
            weakened = report.weakened,
            needs_review = report.needs_review,
            "goalpost moved"
        );
    } else {
        info!(thesis_id = %ev.thesis_id, version = ev.version, "goalpost clean");
    }
    Ok(())
}

async fn load_conditions(
    store: &Store,
    thesis_id: Uuid,
) -> Result<(Vec<Condition>, Vec<Condition>)> {
    let row = sqlx::query(
        r#"SELECT COALESCE(immutable_original -> 'invalidation_conditions', '[]'::jsonb)
                  AS original,
                  invalidation_conditions AS current
             FROM thesis WHERE thesis_id = $1"#,
    )
    .bind(thesis_id)
    .fetch_optional(&store.pool)
    .await?
    .ok_or_else(|| anyhow::anyhow!("thesis {thesis_id} not found"))?;

    let orig_json: serde_json::Value = row.try_get("original")?;
    let curr_json: serde_json::Value = row.try_get("current")?;
    let orig: Vec<Condition> = serde_json::from_value(orig_json)?;
    let curr: Vec<Condition> = serde_json::from_value(curr_json)?;
    Ok((orig, curr))
}

async fn record_version(
    store: &Store,
    thesis_id: Uuid,
    version: i32,
    diff: &serde_json::Value,
    rationale: &str,
    weakens: bool,
) -> Result<()> {
    let rationale_opt: Option<&str> = if rationale.is_empty() { None } else { Some(rationale) };
    sqlx::query(
        r#"INSERT INTO thesis_version_history
             (thesis_id, version, diff, rationale, weakens_invalidation)
             VALUES ($1, $2, $3, $4, $5)"#,
    )
    .bind(thesis_id)
    .bind(version)
    .bind(diff)
    .bind(rationale_opt)
    .bind(weakens)
    .execute(&store.pool)
    .await?;
    Ok(())
}
