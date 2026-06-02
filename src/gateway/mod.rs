//! Decision/alert + UI gateway (SPEC §3 + §11):
//!
//! - REST: `/healthz`, `GET /api/alerts`, `POST /api/decisions`
//! - SSE:  `GET /api/stream` (NATS thesis.* + risk.* → SSE hub fan-out)
//! - SPA:  `/*` falls back to the embedded Svelte bundle with index.html
//!   fallback for client-side routing.

mod routes;
mod sse;

use std::sync::Arc;

use anyhow::Result;
use tracing::info;

use crate::ingest::fmp_intraday::FmpIntradayAdapter;
use crate::llm::{Provider, prompts};
use crate::platform::bus::{Bus, ConsumerHandle};
use crate::platform::store::Store;
use crate::platform::subjects;

use self::sse::Hub;

pub struct Gateway {
    pub store: Arc<Store>,
    pub bus: Arc<Bus>,
    pub hub: Arc<Hub>,
    pub fmp_intraday: Arc<FmpIntradayAdapter>,
    pub llm: Arc<dyn Provider>,
    pub llm_provider_name: String,
    pub llm_model: String,
    pub prompts: prompts::Registry,
    /// When Some, the SPA fallback returns a 302 to this URL instead of
    /// serving the rust-embed'd snapshot. Set by `make dev` so :8080 is
    /// API-only and the live SPA lives at :5173.
    pub dev_redirect: Option<String>,
}

impl Gateway {
    pub fn new(
        store: Store,
        bus: Bus,
        fmp_api_key: String,
        fmp_base_url: String,
        llm: Box<dyn Provider>,
        llm_provider_name: String,
        llm_model: String,
        prompts: prompts::Registry,
        dev_redirect: Option<String>,
    ) -> Self {
        Self {
            store: Arc::new(store),
            bus: Arc::new(bus),
            hub: Arc::new(Hub::new()),
            fmp_intraday: Arc::new(FmpIntradayAdapter::new(&fmp_api_key, &fmp_base_url)),
            llm: Arc::from(llm),
            llm_provider_name,
            llm_model,
            prompts,
            dev_redirect,
        }
    }

    /// Binds durable JetStream consumers that persist alerts and feed the
    /// SSE hub. Returns the [`ConsumerHandle`]s — drop to stop.
    pub async fn start_subscriptions(&self) -> Result<Vec<ConsumerHandle>> {
        self.bus
            .ensure_stream(subjects::STREAM_THESIS, &["thesis.*"])
            .await?;
        self.bus
            .ensure_stream(subjects::STREAM_DECISIONS, &["risk.*", "decision.*"])
            .await?;

        let thesis = self
            .bind_consumer(
                subjects::STREAM_THESIS,
                "gateway-thesis-alerts",
                "thesis.*",
                "state_transition",
            )
            .await?;
        let risk = self
            .bind_consumer(
                subjects::STREAM_DECISIONS,
                "gateway-risk-alerts",
                "risk.*",
                "risk",
            )
            .await?;
        Ok(vec![thesis, risk])
    }

    async fn bind_consumer(
        &self,
        stream: &str,
        durable: &str,
        filter: &str,
        kind: &'static str,
    ) -> Result<ConsumerHandle> {
        let store = self.store.clone();
        let hub = self.hub.clone();
        self.bus
            .consume(stream, durable, filter, move |msg| {
                let store = store.clone();
                let hub = hub.clone();
                async move {
                    let kind_enum = match kind {
                        "state_transition" => crate::platform::domain::AlertKind::StateTransition,
                        "risk" => crate::platform::domain::AlertKind::Risk,
                        _ => crate::platform::domain::AlertKind::StateTransition,
                    };
                    if let Err(e) = store.insert_alert(kind_enum, None, &msg.payload).await {
                        return Err(e);
                    }
                    let payload_json: serde_json::Value =
                        serde_json::from_slice(&msg.payload).unwrap_or(serde_json::Value::Null);
                    let env = serde_json::json!({
                        "subject": msg.subject.as_str(),
                        "kind": kind,
                        "payload": payload_json,
                    });
                    hub.broadcast(env.to_string());
                    Ok(())
                }
            })
            .await
    }

    /// Builds the axum router. Caller is responsible for `tokio::net::TcpListener::bind` + `axum::serve`.
    pub fn router(self: Arc<Self>) -> axum::Router {
        info!("gateway router built");
        routes::build(self)
    }
}
