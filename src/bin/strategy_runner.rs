//! Shadow automation strategy runner (#293).

use std::time::Duration;

use anyhow::Result;
use stocks::platform::{config::Config, logging, store::Store};

#[tokio::main]
async fn main() -> Result<()> {
    logging::init("strategy-runner");
    let cfg = Config::load();
    let interval = std::env::var("AUTOMATION_STRATEGY_INTERVAL_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or_else(|| Duration::from_secs(60));
    let limit = std::env::var("AUTOMATION_STRATEGY_MAX_PER_PASS")
        .ok()
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(100)
        .clamp(1, 500);

    let store = Store::connect(&cfg.database_url).await?;
    if matches!(
        std::env::var("AUTOMATION_STRATEGY_ONCE").as_deref(),
        Ok("1" | "true" | "yes")
    ) {
        let summary = stocks::automation::run_once(&store, limit).await?;
        println!("{}", serde_json::to_string_pretty(&summary)?);
        return Ok(());
    }

    stocks::automation::run(store, interval, limit).await
}
