//! NATS + JetStream wrapper (SPEC §3: durable, replayable streams).
//!
//! Publish is JetStream-only — every message is persisted to the matching
//! stream. Consume binds a *durable* JetStream consumer with explicit ack
//! and bounded redelivery; the cursor lives server-side so restarts replay
//! anything unacked. Plain core NATS is exposed for the rare fire-and-forget
//! case but production paths should go through [`Bus::consume`].

use std::time::Duration;

use anyhow::{Context, Result};
use async_nats::{
    Client,
    jetstream::{
        self, Context as JsContext,
        consumer::{DeliverPolicy, PullConsumer, pull},
        stream::{Config as StreamConfig, RetentionPolicy, StorageType},
    },
};
use futures::StreamExt;
use tokio::sync::oneshot;
use tracing::{error, info};

#[derive(Clone)]
pub struct Bus {
    pub nc: Client,
    pub js: JsContext,
}

/// Returned by [`Bus::consume`]; dropping it stops the consumer task at
/// the next batch boundary.
pub struct ConsumerHandle {
    cancel: Option<oneshot::Sender<()>>,
}

impl ConsumerHandle {
    /// Signals the consumer task to drain and stop.
    pub fn stop(mut self) {
        if let Some(tx) = self.cancel.take() {
            let _ = tx.send(());
        }
    }
}

impl Drop for ConsumerHandle {
    fn drop(&mut self) {
        if let Some(tx) = self.cancel.take() {
            let _ = tx.send(());
        }
    }
}

impl Bus {
    /// Dials NATS and initializes the JetStream context.
    pub async fn connect(url: &str) -> Result<Self> {
        let nc = async_nats::ConnectOptions::new()
            .name("stocks")
            .connect(url)
            .await
            .with_context(|| format!("nats connect {url}"))?;
        let js = jetstream::new(nc.clone());
        Ok(Self { nc, js })
    }

    /// Idempotently creates/updates a file-backed stream over `subjects`.
    /// Both producers and consumers should call this for the streams they
    /// touch — calls are cheap and prevent startup races.
    pub async fn ensure_stream(&self, name: &str, subjects: &[&str]) -> Result<()> {
        let cfg = StreamConfig {
            name: name.to_string(),
            subjects: subjects.iter().map(|&s| s.to_string()).collect(),
            storage: StorageType::File,
            retention: RetentionPolicy::Limits,
            ..Default::default()
        };
        self.js
            .get_or_create_stream(cfg)
            .await
            .map_err(|e| anyhow::anyhow!("ensure_stream {name}: {e}"))?;
        Ok(())
    }

    /// Publishes a message to JetStream. Errors if no stream covers `subject`.
    pub async fn publish(&self, subject: &str, payload: &[u8]) -> Result<()> {
        self.js
            .publish(subject.to_string(), payload.to_vec().into())
            .await
            .map_err(|e| anyhow::anyhow!("publish {subject}: {e}"))?
            .await
            .map_err(|e| anyhow::anyhow!("publish ack {subject}: {e}"))?;
        Ok(())
    }

    /// Creates (or updates) a durable JetStream consumer on the named stream,
    /// filtered to `filter_subject`, and spawns a pull-based loop dispatching
    /// to `handler`. Ack policy:
    ///
    /// - `Ok(())` from handler → ack (mark delivered).
    /// - `Err(_)` from handler → nak (redeliver up to `MaxDeliver=5`).
    pub async fn consume<F, Fut>(
        &self,
        stream: &str,
        durable: &str,
        filter_subject: &str,
        handler: F,
    ) -> Result<ConsumerHandle>
    where
        F: Fn(jetstream::Message) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<()>> + Send + 'static,
    {
        let s = self
            .js
            .get_stream(stream)
            .await
            .map_err(|e| anyhow::anyhow!("get_stream {stream}: {e}"))?;
        let consumer: PullConsumer = s
            .get_or_create_consumer(
                durable,
                pull::Config {
                    durable_name: Some(durable.to_string()),
                    filter_subject: filter_subject.to_string(),
                    ack_policy: jetstream::consumer::AckPolicy::Explicit,
                    max_deliver: 5,
                    deliver_policy: DeliverPolicy::All,
                    ..Default::default()
                },
            )
            .await
            .map_err(|e| anyhow::anyhow!("create consumer {stream}/{durable}: {e}"))?;

        let (tx, mut rx) = oneshot::channel::<()>();
        let durable_name = durable.to_string();
        let stream_name = stream.to_string();

        tokio::spawn(async move {
            loop {
                // Cancellation check.
                match rx.try_recv() {
                    Ok(()) | Err(oneshot::error::TryRecvError::Closed) => {
                        info!(stream = %stream_name, durable = %durable_name, "consumer stopped");
                        return;
                    }
                    Err(oneshot::error::TryRecvError::Empty) => {}
                }

                let mut batch = match consumer
                    .batch()
                    .max_messages(16)
                    .expires(Duration::from_secs(5))
                    .messages()
                    .await
                {
                    Ok(b) => b,
                    Err(e) => {
                        error!(stream = %stream_name, durable = %durable_name, error = %e, "batch fetch failed");
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        continue;
                    }
                };

                while let Some(item) = batch.next().await {
                    let msg = match item {
                        Ok(m) => m,
                        Err(e) => {
                            error!(stream = %stream_name, durable = %durable_name, error = %e, "message error");
                            continue;
                        }
                    };
                    match handler(msg.clone()).await {
                        Ok(()) => {
                            if let Err(e) = msg.ack().await {
                                error!(error = %e, "ack failed");
                            }
                        }
                        Err(e) => {
                            error!(error = %e, "handler error; nacking");
                            if let Err(e) = msg
                                .ack_with(async_nats::jetstream::AckKind::Nak(None))
                                .await
                            {
                                error!(error = %e, "nak failed");
                            }
                        }
                    }
                }
            }
        });

        Ok(ConsumerHandle { cancel: Some(tx) })
    }
}
