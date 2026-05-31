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
use stocks::ingest::fmp::FmpPriceAdapter;
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

    // FMP (primary price source, #60). Replaces Massive as of 2026-05-31.
    // Massive adapter is still in the codebase but no longer wired here —
    // it's reserved for the news+sentiment work (#19) where it's the only
    // vendor we have with per-article sentiment.
    {
        let store = store.clone();
        let key = cfg.fmp_api_key.clone();
        let base = cfg.fmp_base_url.clone();
        tokio::spawn(async move {
            let adapter = FmpPriceAdapter::new(&key, &base);
            let interval = Duration::from_secs(6 * 3600);
            loop {
                // 2y lookback: covers Donchian-55 base_breakout (#22), full
                // SMA-200 (consensus #21 price_extension), and gives the
                // 52w-high baseline. FMP Starter supports 5y+; (symbol, ts)
                // PK + ON CONFLICT means each subsequent poll is idempotent.
                match adapter.poll_all(730).await {
                    Ok(rows) if rows.is_empty() => {}
                    Ok(rows) => match store.upsert_price_bars(&rows).await {
                        Ok(inserted) => info!(rows = rows.len(), inserted, "fmp price pass complete"),
                        Err(e) => error!(error = %e, "fmp price persist failed"),
                    },
                    Err(e) => error!(error = %e, "fmp price poll failed"),
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
