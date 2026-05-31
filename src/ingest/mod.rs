//! Adapter framework: fetch → normalize → append-only store + emit (SPEC §3).
//!
//! Each adapter is a small async object with `name`, `interval`, `poll`. The
//! [`run`] runner spawns each on its own ticker and dedups via `content_hash`
//! (so re-polling old data is harmless).

pub mod cboe;
pub mod crowd_sentiment_service;
pub mod discovery_pool_service;
pub mod edgar;
pub mod fmp;
pub mod fmp_estimates;
pub mod fmp_estimates_service;
pub mod fmp_news;
pub mod fmp_screener;
pub mod fred;
pub mod massive;
pub mod massive_news;
pub mod news_service;
pub mod sec;
pub mod xbrl;

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use tokio::sync::Notify;
use tokio::task::JoinSet;
use tracing::{error, info};

use crate::platform::bus::Bus;
use crate::platform::store::Store;

/// Normalized item produced by an adapter.
#[derive(Debug, Clone)]
pub struct Event {
    pub source: String,
    pub kind: String,
    pub symbol: String,    // "" for market-wide
    pub subject: String,   // NATS subject to publish on
    pub payload: Vec<u8>,  // canonical JSON
    pub source_ts: Option<DateTime<Utc>>,
}

impl Event {
    /// Stable dedup key over source + kind + symbol + payload.
    #[must_use]
    pub fn content_hash(&self) -> String {
        let mut h = Sha256::new();
        for s in [&self.source, &self.kind, &self.symbol] {
            h.update(s.as_bytes());
            h.update([0]);
        }
        h.update(&self.payload);
        hex::encode(h.finalize())
    }
}

#[async_trait]
pub trait Adapter: Send + Sync {
    fn name(&self) -> &str;
    fn interval(&self) -> Duration;
    async fn poll(&self) -> Result<Vec<Event>>;
}

/// Runs adapters concurrently until `shutdown` resolves.
pub async fn run<F>(
    store: Store,
    bus: Bus,
    adapters: Vec<Box<dyn Adapter>>,
    shutdown: F,
) -> Result<()>
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    let store = Arc::new(store);
    let bus = Arc::new(bus);
    let stop = Arc::new(Notify::new());
    let mut set: JoinSet<()> = JoinSet::new();

    for adapter in adapters {
        let store = store.clone();
        let bus = bus.clone();
        let stop = stop.clone();
        set.spawn(async move { adapter_loop(adapter, store, bus, stop).await });
    }

    // Wait for the caller's shutdown signal, then notify every adapter.
    shutdown.await;
    info!("ingest run: shutdown signaled");
    stop.notify_waiters();
    while set.join_next().await.is_some() {}
    Ok(())
}

async fn adapter_loop(
    adapter: Box<dyn Adapter>,
    store: Arc<Store>,
    bus: Arc<Bus>,
    stop: Arc<Notify>,
) {
    let interval = adapter.interval();
    let name = adapter.name().to_string();
    loop {
        run_once(&*adapter, &store, &bus, &name).await;
        tokio::select! {
            () = tokio::time::sleep(interval) => {},
            () = stop.notified() => {
                info!(adapter = %name, "adapter stopping");
                return;
            }
        }
    }
}

async fn run_once(adapter: &dyn Adapter, store: &Store, bus: &Bus, name: &str) {
    let events = match adapter.poll().await {
        Ok(e) => e,
        Err(e) => {
            error!(adapter = name, error = %e, "poll failed");
            return;
        }
    };
    let mut stored = 0u32;
    let mut published = 0u32;
    for ev in events {
        let symbol_opt = if ev.symbol.is_empty() { None } else { Some(ev.symbol.as_str()) };
        let inserted = match store
            .append_ingest_event(
                &ev.source,
                &ev.kind,
                symbol_opt,
                &ev.payload,
                &ev.content_hash(),
                ev.source_ts,
            )
            .await
        {
            Ok(b) => b,
            Err(e) => {
                error!(adapter = name, error = %e, "store failed");
                continue;
            }
        };
        if !inserted {
            continue; // already seen
        }
        stored += 1;
        if !ev.subject.is_empty() {
            if let Err(e) = bus.publish(&ev.subject, &ev.payload).await {
                error!(adapter = name, subject = %ev.subject, error = %e, "publish failed");
                continue;
            }
            published += 1;
        }
    }
    if stored > 0 {
        info!(adapter = name, new = stored, published, "ingested");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_hash_dedups_across_runs() {
        let e1 = Event {
            source: "edgar".into(),
            kind: "10-K".into(),
            symbol: "NVDA".into(),
            subject: "ingest.filing".into(),
            payload: br#"{"x":1}"#.to_vec(),
            source_ts: None,
        };
        let mut e2 = e1.clone();
        e2.source_ts = Some(Utc::now()); // source_ts is NOT in the hash
        assert_eq!(e1.content_hash(), e2.content_hash());
    }

    #[test]
    fn content_hash_differs_by_symbol() {
        let mut a = Event {
            source: "edgar".into(),
            kind: "10-K".into(),
            symbol: "NVDA".into(),
            subject: String::new(),
            payload: b"{}".to_vec(),
            source_ts: None,
        };
        let original = a.content_hash();
        a.symbol = "MU".into();
        assert_ne!(a.content_hash(), original);
    }
}
