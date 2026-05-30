//! Risk overlay (SPEC §7).

use anyhow::Result;
use stocks::platform::{bus::Bus, config::Config, logging, store::Store};
use stocks::risk;
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    logging::init("risk");
    let cfg = Config::load();
    let store = Store::connect(&cfg.database_url).await?;
    let bus = Bus::connect(&cfg.nats_url).await?;
    let _handle = risk::run(store, bus).await?;
    info!("risk running");
    let _ = tokio::signal::ctrl_c().await;
    Ok(())
}
