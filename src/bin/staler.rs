//! Staleness service (#11) — wakes every STALER_INTERVAL_SECS (default 300),
//! finds pending conditions past their deadline, marks them stale, emits
//! risk.warning for each.

use std::time::Duration;

use anyhow::Result;
use stocks::platform::{bus::Bus, config::Config, logging, store::Store};
use stocks::thesis::staleness;

#[tokio::main]
async fn main() -> Result<()> {
    logging::init("staler");
    let cfg = Config::load();
    let interval = std::env::var("STALER_INTERVAL_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or_else(|| Duration::from_secs(300));

    let store = Store::connect(&cfg.database_url).await?;
    let bus = Bus::connect(&cfg.nats_url).await?;

    staleness::run(store.pool, bus, interval).await
}
