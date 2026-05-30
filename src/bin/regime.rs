//! Macro regime classifier (SPEC §4).

use anyhow::Result;
use stocks::platform::{bus::Bus, config::Config, logging, store::Store};
use stocks::regime;
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    logging::init("regime");
    let cfg = Config::load();

    let store = Store::connect(&cfg.database_url).await?;
    let bus = Bus::connect(&cfg.nats_url).await?;
    let _handle = regime::run(store, bus).await?;
    info!("regime running");

    let _ = tokio::signal::ctrl_c().await;
    Ok(())
}
