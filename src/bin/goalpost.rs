//! Goalpost detector (SPEC §5.3).

use anyhow::Result;
use stocks::goalpost;
use stocks::platform::{bus::Bus, config::Config, logging, store::Store};
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    logging::init("goalpost");
    let cfg = Config::load();
    let store = Store::connect(&cfg.database_url).await?;
    let bus = Bus::connect(&cfg.nats_url).await?;
    let _handle = goalpost::run(store, bus).await?;
    info!("goalpost running");
    let _ = tokio::signal::ctrl_c().await;
    Ok(())
}
