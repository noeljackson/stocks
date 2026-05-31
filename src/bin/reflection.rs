//! Reflection service (#23). Binds 3 durable consumers on THESIS:
//! actionable → write prediction; fulfilled/invalidated → write outcome.

use anyhow::Result;
use stocks::platform::{bus::Bus, config::Config, logging, store::Store};
use stocks::reflection;

#[tokio::main]
async fn main() -> Result<()> {
    logging::init("reflection");
    let cfg = Config::load();
    let store = Store::connect(&cfg.database_url).await?;
    let bus = Bus::connect(&cfg.nats_url).await?;
    reflection::service::run(store.pool, bus).await
}
