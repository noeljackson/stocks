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
use stocks::ingest::cboe::CboeAdapter;
use stocks::ingest::crowd_sentiment_service;
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

    // FMP price ingest (#60, #88). Per-ticker incremental backfill: 5y on
    // first sight, 30d on subsequent polls. Pool = discovery_pool ∪ ticker
    // (so both scan-pool members AND active universe get bars).
    {
        let store = store.clone();
        let key = cfg.fmp_api_key.clone();
        let base = cfg.fmp_base_url.clone();
        tokio::spawn(async move {
            let adapter = FmpPriceAdapter::new(&key, &base);
            let interval = Duration::from_secs(6 * 3600);
            loop {
                // Pool ∪ active tickers ∪ benchmarks
                let mut symbols: std::collections::BTreeSet<String> = Default::default();
                if let Ok(p) = store.discovery_pool_symbols().await {
                    symbols.extend(p);
                }
                for bench in ["SPY", "QQQ", "SMH", "VIXY"] {
                    symbols.insert(bench.to_string());
                }
                let symbols: Vec<String> = symbols.into_iter().collect();
                if !symbols.is_empty() {
                    let oldest = store.oldest_bar_per_symbol(&symbols).await.unwrap_or_default();
                    match adapter.poll_symbols(&symbols, &oldest).await {
                        Ok(rows) if rows.is_empty() => {}
                        Ok(rows) => match store.upsert_price_bars(&rows).await {
                            Ok(inserted) => info!(symbols = symbols.len(), rows = rows.len(), inserted, "fmp price pass complete"),
                            Err(e) => error!(error = %e, "fmp price persist failed"),
                        },
                        Err(e) => error!(error = %e, "fmp price poll failed"),
                    }
                }
                tokio::time::sleep(interval).await;
            }
        });
    }

    // FMP screener (#88): refresh discovery_pool nightly so the scanner has
    // a broad investible candidate set, not just our hand-curated universe.
    {
        let pool = store.pool.clone();
        let key = cfg.fmp_api_key.clone();
        let base = cfg.fmp_base_url.clone();
        tokio::spawn(async move {
            let adapter = stocks::ingest::fmp_screener::FmpScreenerAdapter::new(&key, &base);
            let interval = Duration::from_secs(24 * 3600);
            if let Err(e) = stocks::ingest::discovery_pool_service::run(pool, adapter, interval).await {
                error!(error = %e, "discovery_pool service exited");
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

    // Crowd sentiment (#20): CBOE put/call + VIX daily CSV → crowd_sentiment.
    // Feeds the consensus retail_attention component.
    {
        let pool = store.pool.clone();
        tokio::spawn(async move {
            let adapter = CboeAdapter::new();
            let interval = Duration::from_secs(6 * 3600);
            if let Err(e) = crowd_sentiment_service::run(pool, adapter, interval).await {
                error!(error = %e, "crowd_sentiment service exited");
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
