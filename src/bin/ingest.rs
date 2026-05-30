//! Ingestion runner: EDGAR + FRED + XBRL adapters.
//!
//! EDGAR + FRED follow the per-event Adapter trait (one Event = one
//! ingest_event row, published to NATS). XBRL (#32) is bulk: ~thousands of
//! company_fact rows per company per poll. It runs as a parallel task with
//! its own interval and persists directly to `company_fact`, bypassing the
//! per-event store-and-publish path.

use std::time::Duration;

use anyhow::Result;
use stocks::ingest;
use stocks::ingest::edgar::EdgarAdapter;
use stocks::ingest::fred::FredAdapter;
use stocks::ingest::massive::MassiveAdapter;
use stocks::ingest::xbrl::XbrlAdapter;
use stocks::platform::{bus::Bus, config::Config, logging, store::Store, subjects};
use tracing::{error, info};

#[tokio::main]
async fn main() -> Result<()> {
    logging::init("ingest");
    let cfg = Config::load();

    let store = Store::connect(&cfg.database_url).await?;
    let bus = Bus::connect(&cfg.nats_url).await?;
    bus.ensure_stream(subjects::STREAM_INGEST, &["ingest.*"]).await?;

    // Spawn the Massive (price) bulk loop. Daily lookback of 30 days on
    // each poll keeps re-sync cheap; the table primary key dedups.
    {
        let store = store.clone();
        let key = cfg.massive_api_key.clone();
        let base = cfg.massive_base_url.clone();
        tokio::spawn(async move {
            let adapter = MassiveAdapter::new(&key, &base);
            let interval = Duration::from_secs(6 * 3600);
            loop {
                match adapter.poll_all(30).await {
                    Ok(rows) if rows.is_empty() => {
                        // already warned if key missing; otherwise no-op
                    }
                    Ok(rows) => match store.upsert_price_bars(&rows).await {
                        Ok(inserted) => info!(rows = rows.len(), inserted, "massive pass complete"),
                        Err(e) => error!(error = %e, "massive persist failed"),
                    },
                    Err(e) => error!(error = %e, "massive poll failed"),
                }
                tokio::time::sleep(interval).await;
            }
        });
    }

    // Spawn the XBRL bulk loop in parallel with the per-event adapter runner.
    {
        let store = store.clone();
        let ua = cfg.sec_user_agent.clone();
        tokio::spawn(async move {
            let adapter = XbrlAdapter::new(&ua);
            let interval = Duration::from_secs(6 * 3600);
            // First-run fires immediately so a fresh deploy populates company_fact.
            loop {
                match adapter.poll_all().await {
                    Ok(rows) => match store.upsert_company_facts(&rows).await {
                        Ok(inserted) => info!(rows = rows.len(), inserted, "xbrl pass complete"),
                        Err(e) => error!(error = %e, "xbrl persist failed"),
                    },
                    Err(e) => error!(error = %e, "xbrl poll failed"),
                }
                tokio::time::sleep(interval).await;
            }
        });
    }

    let adapters: Vec<Box<dyn ingest::Adapter>> = vec![
        Box::new(EdgarAdapter::new(&cfg.sec_user_agent)),
        Box::new(FredAdapter::new(&cfg.fred_api_key)),
    ];

    info!("ingestion started");
    ingest::run(store, bus, adapters, async {
        let _ = tokio::signal::ctrl_c().await;
        info!("shutdown signal received");
    })
    .await
}
