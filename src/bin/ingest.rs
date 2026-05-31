//! Ingestion runner: EDGAR + FRED + XBRL adapters.
//!
//! EDGAR + FRED follow the per-event Adapter trait (one Event = one
//! ingest_event row, published to NATS). XBRL (#32) is bulk: ~thousands of
//! company_fact rows per company per poll. It runs as a parallel task with
//! its own interval and persists directly to `company_fact`, bypassing the
//! per-event store-and-publish path.

use std::time::Duration;

use anyhow::Result;
use stocks::ingest;
use stocks::ingest::edgar::EdgarAdapter;
use stocks::ingest::fred::FredAdapter;
use stocks::ingest::fmp::FmpPriceAdapter;
use stocks::ingest::fmp_estimates::FmpEstimatesAdapter;
use stocks::ingest::fmp_estimates_service;
use stocks::ingest::fmp_news::FmpNewsAdapter;
use stocks::ingest::massive_news::MassiveNewsAdapter;
use stocks::ingest::news_service::{self, NewsIngestService, ScorerFn};
use stocks::ingest::xbrl::XbrlAdapter;
use stocks::llm::prompts::load;
use stocks::llm::{self};
use stocks::sentiment;
use std::sync::Arc;
use stocks::platform::{bus::Bus, config::Config, logging, store::Store, subjects};
use tracing::{error, info};

#[tokio::main]
async fn main() -> Result<()> {
    logging::init("ingest");
    let cfg = Config::load();

    let store = Store::connect(&cfg.database_url).await?;
    let bus = Bus::connect(&cfg.nats_url).await?;
    bus.ensure_stream(subjects::STREAM_INGEST, &["ingest.*"]).await?;

    // FMP (primary price source, #60). Replaces Massive as of 2026-05-31.
    // Massive adapter is still in the codebase but no longer wired here —
    // it's reserved for the news+sentiment work (#19) where it's the only
    // vendor we have with per-article sentiment.
    {
        let store = store.clone();
        let key = cfg.fmp_api_key.clone();
        let base = cfg.fmp_base_url.clone();
        tokio::spawn(async move {
            let adapter = FmpPriceAdapter::new(&key, &base);
            let interval = Duration::from_secs(6 * 3600);
            loop {
                // 2y lookback: covers Donchian-55 base_breakout (#22), full
                // SMA-200 (consensus #21 price_extension), and gives the
                // 52w-high baseline. FMP Starter supports 5y+; (symbol, ts)
                // PK + ON CONFLICT means each subsequent poll is idempotent.
                match adapter.poll_all(730).await {
                    Ok(rows) if rows.is_empty() => {}
                    Ok(rows) => match store.upsert_price_bars(&rows).await {
                        Ok(inserted) => info!(rows = rows.len(), inserted, "fmp price pass complete"),
                        Err(e) => error!(error = %e, "fmp price persist failed"),
                    },
                    Err(e) => error!(error = %e, "fmp price poll failed"),
                }
                tokio::time::sleep(interval).await;
            }
        });
    }

    // FMP analyst-estimates snapshot + diff service (#18). Daily snapshot per
    // universe ticker → estimate_snapshot; diff vs prior → estimate_revision.
    // 6h interval matches the price loop; the diff is a no-op on quiet days.
    {
        let pool = store.pool.clone();
        let key = cfg.fmp_api_key.clone();
        let base = cfg.fmp_base_url.clone();
        tokio::spawn(async move {
            let adapter = FmpEstimatesAdapter::new(&key, &base);
            let interval = Duration::from_secs(6 * 3600);
            if let Err(e) = fmp_estimates_service::run(pool, adapter, interval).await {
                error!(error = %e, "fmp_estimates service exited");
            }
        });
    }

    // News ingest (#19): poll FMP + Massive news endpoints, dedupe, sentiment
    // from upstream when present (Massive) else from our LLM classifier (FMP).
    {
        let pool = store.pool.clone();
        let fmp_key = cfg.fmp_api_key.clone();
        let fmp_base = cfg.fmp_base_url.clone();
        let m_key = cfg.massive_api_key.clone();
        let m_base = cfg.massive_base_url.clone();
        let llm_cfg = cfg.llm();
        let prompts_dir = std::path::PathBuf::from("prompts");
        let store_for_recorder = store.clone();
        tokio::spawn(async move {
            // Build the scorer closure lazily; only fires if we have an LLM
            // provider AND the prompt file is readable.
            let registry = match load(&prompts_dir) {
                Ok(r) => r,
                Err(e) => {
                    error!(error = %e, "news_service: prompts dir unreadable; sentiment scoring disabled");
                    return;
                }
            };
            let Some(prompt) = registry.get("score-sentiment").cloned() else {
                error!("news_service: prompts/score-sentiment.md missing; sentiment scoring disabled");
                return;
            };
            let provider: Arc<dyn llm::Provider> = Arc::from(llm::new(&llm_cfg));
            let provider_name = if llm_cfg.provider.is_empty() {
                llm::detect(&llm_cfg).to_string()
            } else {
                llm_cfg.provider.clone()
            };

            let scorer: ScorerFn = {
                let provider = provider.clone();
                let store = store_for_recorder.clone();
                let prompt = prompt.clone();
                let pn = provider_name.clone();
                Arc::new(move |ticker: &str, title: &str, body: &str| {
                    let provider = provider.clone();
                    let store = store.clone();
                    let prompt = prompt.clone();
                    let pn = pn.clone();
                    let ticker = ticker.to_string();
                    let title = title.to_string();
                    let body = body.to_string();
                    Box::pin(async move {
                        sentiment::score_one(
                            provider.as_ref(),
                            Some(&store),
                            &prompt,
                            &pn,
                            &ticker,
                            &title,
                            &body,
                            None,
                        )
                        .await
                    })
                })
            };

            let svc = NewsIngestService {
                pool,
                fmp: FmpNewsAdapter::new(&fmp_key, &fmp_base),
                massive: MassiveNewsAdapter::new(&m_key, &m_base),
                scorer: Some(scorer),
                prompt_name: prompt.name.clone(),
                prompt_hash: prompt.hash.clone(),
                per_ticker_limit: 20,
            };
            let interval = Duration::from_secs(2 * 3600);
            if let Err(e) = news_service::run(svc, interval).await {
                error!(error = %e, "news_service exited");
            }
        });
    }

    // Spawn the XBRL bulk loop in parallel with the per-event adapter runner.
    {
        let store = store.clone();
        let ua = cfg.sec_user_agent.clone();
        tokio::spawn(async move {
            let adapter = XbrlAdapter::new(&ua);
            let interval = Duration::from_secs(6 * 3600);
            // First-run fires immediately so a fresh deploy populates company_fact.
            loop {
                match adapter.poll_all().await {
                    Ok(rows) => match store.upsert_company_facts(&rows).await {
                        Ok(inserted) => info!(rows = rows.len(), inserted, "xbrl pass complete"),
                        Err(e) => error!(error = %e, "xbrl persist failed"),
                    },
                    Err(e) => error!(error = %e, "xbrl poll failed"),
                }
                tokio::time::sleep(interval).await;
            }
        });
    }

    let adapters: Vec<Box<dyn ingest::Adapter>> = vec![
        Box::new(EdgarAdapter::new(&cfg.sec_user_agent)),
        Box::new(FredAdapter::new(&cfg.fred_api_key)),
    ];

    info!("ingestion started");
    ingest::run(store, bus, adapters, async {
        let _ = tokio::signal::ctrl_c().await;
        info!("shutdown signal received");
    })
    .await
}
