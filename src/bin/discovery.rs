//! Discovery scanner service (#22). Walks active tickers every
//! DISCOVERY_INTERVAL_SECS (default 300 prod, 60 dev), runs enabled signal
//! detectors, persists hits + publishes discovery.candidate per hit.

use std::time::Duration;

use anyhow::Result;
use stocks::discovery;
use stocks::platform::{bus::Bus, config::Config, logging, store::Store};

#[tokio::main]
async fn main() -> Result<()> {
    logging::init("discovery");
    let cfg = Config::load();
    let interval = std::env::var("DISCOVERY_INTERVAL_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or_else(|| Duration::from_secs(300));

    let store = Store::connect(&cfg.database_url).await?;
    let bus = Bus::connect(&cfg.nats_url).await?;
    discovery::service::run(store.pool, bus, interval).await
}
