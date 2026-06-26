//! Price alert evaluator — manual and AI-generated price levels.

use std::time::Duration;

use anyhow::Result;
use stocks::platform::{bus::Bus, config::Config, logging, store::Store};

#[tokio::main]
async fn main() -> Result<()> {
    logging::init("price-alerts");
    let cfg = Config::load();
    let interval = std::env::var("PRICE_ALERT_INTERVAL_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or_else(|| Duration::from_secs(60));

    let store = Store::connect(&cfg.database_url).await?;
    let bus = Bus::connect(&cfg.nats_url).await?;
    stocks::price_alerts::run(store, bus, interval).await
}
