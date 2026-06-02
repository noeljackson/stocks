//! Event router: ingest.* → route.ticker.>

use anyhow::Result;
use stocks::platform::{bus::Bus, config::Config, logging};
use stocks::router;
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    logging::init("router");
    let cfg = Config::load();
    let bus = Bus::connect(&cfg.nats_url).await?;
    let _handle = router::run(bus).await?;
    info!("router running");
    let _ = tokio::signal::ctrl_c().await;
    Ok(())
}
