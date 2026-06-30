//! Scores due technical timing observations (#288).

use std::time::Duration;

use anyhow::Result;
use stocks::platform::{config::Config, logging, store::Store};

#[tokio::main]
async fn main() -> Result<()> {
    logging::init("technical-timing-validator");
    let cfg = Config::load();
    let interval = std::env::var("TECHNICAL_TIMING_VALIDATOR_INTERVAL_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or_else(|| Duration::from_secs(300));
    let limit = std::env::var("TECHNICAL_TIMING_VALIDATOR_MAX_PER_PASS")
        .ok()
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(500)
        .clamp(1, 1000);

    let store = Store::connect(&cfg.database_url).await?;
    if matches!(
        std::env::var("TECHNICAL_TIMING_VALIDATOR_ONCE").as_deref(),
        Ok("1" | "true" | "yes")
    ) {
        let scored = store.score_due_technical_timing_observations(limit).await?;
        println!("{scored}");
        return Ok(());
    }

    loop {
        let scored = store.score_due_technical_timing_observations(limit).await?;
        if scored > 0 {
            tracing::info!(scored, "technical timing observations scored");
        }
        tokio::time::sleep(interval).await;
    }
}
