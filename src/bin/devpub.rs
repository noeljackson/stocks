//! Dev helper: publish a synthetic event for local smoke testing.
//!
//! Usage: `devpub <subject> <json-payload>`
//! Example: `devpub thesis.actionable '{"ticker":"NVDA","conviction":0.72}'`

use anyhow::Result;
use stocks::platform::{bus::Bus, config::Config};

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: devpub <subject> <json-payload>");
        std::process::exit(2);
    }
    let subject = &args[1];
    let payload = &args[2];
    let cfg = Config::load();
    let bus = Bus::connect(&cfg.nats_url).await?;
    bus.publish(subject, payload.as_bytes()).await?;
    println!("published {subject} {} bytes", payload.len());
    Ok(())
}
