//! Scores automation strategy readiness and applies approved lifecycle gates.

use std::time::Duration;

use anyhow::Result;
use stocks::automation::builtin_strategy_definitions;
use stocks::platform::{config::Config, logging, store::Store};

#[tokio::main]
async fn main() -> Result<()> {
    logging::init("automation-readiness");
    let cfg = Config::load();
    let interval = std::env::var("AUTOMATION_READINESS_INTERVAL_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or_else(|| Duration::from_secs(300));
    let lookback_days = std::env::var("AUTOMATION_READINESS_LOOKBACK_DAYS")
        .ok()
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(90)
        .clamp(1, 3660);

    let store = Store::connect(&cfg.database_url).await?;
    store
        .ensure_builtin_automation_strategies(&builtin_strategy_definitions())
        .await?;
    if matches!(
        std::env::var("AUTOMATION_READINESS_ONCE").as_deref(),
        Ok("1" | "true" | "yes")
    ) {
        let evaluated = store.evaluate_automation_readiness(lookback_days).await?;
        println!("{evaluated}");
        return Ok(());
    }

    loop {
        let evaluated = store.evaluate_automation_readiness(lookback_days).await?;
        tracing::info!(
            evaluated,
            lookback_days,
            "automation readiness pass complete"
        );
        tokio::time::sleep(interval).await;
    }
}
