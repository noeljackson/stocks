//! Condition evaluator service (#14) — walks v_condition every
//! EVAL_INTERVAL_SECS (default 60), resolves each pending condition's metric,
//! and updates status.

use std::time::Duration;

use anyhow::Result;
use stocks::platform::{config::Config, logging, store::Store};
use stocks::thesis::evaluator;

#[tokio::main]
async fn main() -> Result<()> {
    logging::init("evaluator");
    let cfg = Config::load();
    let interval = std::env::var("EVAL_INTERVAL_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or_else(|| Duration::from_secs(60));

    let store = Store::connect(&cfg.database_url).await?;
    evaluator::run(store.pool, interval).await
}
