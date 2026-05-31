//! Consensus computation service (#21). Walks the active universe every
//! CONSENSUS_INTERVAL_SECS (default 300 prod, 30 dev), scores each symbol,
//! persists to consensus_score, emits thesis.fulfilled / thesis.updated on
//! fresh threshold crossings.

use std::time::Duration;

use anyhow::Result;
use stocks::consensus;
use stocks::platform::{bus::Bus, config::Config, logging, store::Store};

#[tokio::main]
async fn main() -> Result<()> {
    logging::init("consensus");
    let cfg = Config::load();
    let interval = std::env::var("CONSENSUS_INTERVAL_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or_else(|| Duration::from_secs(300));

    let store = Store::connect(&cfg.database_url).await?;
    let bus = Bus::connect(&cfg.nats_url).await?;
    consensus::service::run(store.pool, bus, interval).await
}
