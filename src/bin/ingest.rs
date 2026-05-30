//! Ingestion runner: EDGAR + FRED adapters → NATS + append-only store.

use anyhow::Result;
use stocks::ingest;
use stocks::ingest::edgar::EdgarAdapter;
use stocks::ingest::fred::FredAdapter;
use stocks::platform::{bus::Bus, config::Config, logging, store::Store, subjects};
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    logging::init("ingest");
    let cfg = Config::load();

    let store = Store::connect(&cfg.database_url).await?;
    let bus = Bus::connect(&cfg.nats_url).await?;
    bus.ensure_stream(subjects::STREAM_INGEST, &["ingest.*"]).await?;

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
