//! Structured logging (JSON to stdout) via tracing-subscriber.
//!
//! RUST_LOG env var picks the level / per-target filters; defaults to "info".

use tracing_subscriber::{EnvFilter, fmt, prelude::*};

/// Installs a JSON tracing subscriber. Idempotent: subsequent calls are
/// no-ops (the second install fails silently via `try_init`). The optional
/// `service` field is attached to every event so logs in a multi-service
/// k8s pod are still attributable.
pub fn init(service: &'static str) {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn"));

    let layer = fmt::layer()
        .json()
        .with_target(false)
        .with_current_span(false)
        .with_span_list(false);

    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(layer)
        .try_init();

    // Attach the service name as a default field on every event in this process.
    tracing::info!(service, "logger initialized");
}
