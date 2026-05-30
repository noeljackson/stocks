//! HTTP + SSE + embedded SPA gateway (SPEC §3 + §11).

use std::sync::Arc;

use anyhow::Result;
use stocks::gateway::Gateway;
use stocks::platform::{bus::Bus, config::Config, logging, store::Store};
use tracing::{error, info};

#[tokio::main]
async fn main() -> Result<()> {
    logging::init("gateway");
    let cfg = Config::load();

    let store = Store::connect(&cfg.database_url).await?;
    let bus = Bus::connect(&cfg.nats_url).await?;
    let dev_redirect = if cfg.dev_mode { Some(cfg.dev_ui_url.clone()) } else { None };
    if let Some(t) = dev_redirect.as_deref() {
        info!(target = t, "dev mode: SPA fallback will redirect to vite");
    }
    let gw = Arc::new(Gateway::new(store, bus, dev_redirect));

    let _consumers = gw.start_subscriptions().await?;
    let app = gw.clone().router();

    let addr = cfg.gateway_addr.trim_start_matches(':');
    let bind = format!("0.0.0.0:{addr}");
    info!(addr = %bind, "gateway listening");
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    if let Err(e) = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
    {
        error!(error = %e, "axum serve");
    }
    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    info!("shutdown signal received");
}
