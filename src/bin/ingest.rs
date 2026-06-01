//! Ingestion runner: EDGAR + FRED + XBRL adapters.
//!
//! EDGAR + FRED follow the per-event Adapter trait (one Event = one
//! ingest_event row, published to NATS). XBRL (#32) is bulk: ~thousands of
//! company_fact rows per company per poll. It runs as a parallel task with
//! its own interval and persists directly to `company_fact`, bypassing the
//! per-event store-and-publish path.

use anyhow::Result;
use std::sync::Arc;
use stocks::ingest::cboe::CboeAdapter;
use stocks::ingest::crowd_sentiment_service;
use stocks::ingest::edgar::EdgarAdapter;
use stocks::ingest::fmp::FmpPriceAdapter;
use stocks::ingest::fmp_estimates::FmpEstimatesAdapter;
use stocks::ingest::fmp_estimates_service;
use stocks::ingest::fmp_news::FmpNewsAdapter;
use stocks::ingest::fmp_opinion::FmpOpinionAdapter;
use stocks::ingest::fmp_opinion_service;
use stocks::ingest::fred::FredAdapter;
use stocks::ingest::massive_news::MassiveNewsAdapter;
use stocks::ingest::news_service::{self, NewsIngestService, ScorerFn};
use stocks::ingest::xbrl::XbrlAdapter;
use stocks::ingest::{self, rate_limit, source_health};
use stocks::llm::prompts::load;
use stocks::llm::{self};
use stocks::platform::{bus::Bus, config::Config, logging, store::Store, subjects};
use stocks::sentiment;
use tracing::{error, info};

#[tokio::main]
async fn main() -> Result<()> {
    logging::init("ingest");
    let cfg = Config::load();

    let store = Store::connect(&cfg.database_url).await?;
    let bus = Bus::connect(&cfg.nats_url).await?;
    bus.ensure_stream(subjects::STREAM_INGEST, &["ingest.*"])
        .await?;

    // FMP price ingest (#60, #88). Per-ticker incremental backfill: 5y on
    // first sight, 30d on subsequent polls. This follows the tiered deep
    // universe so active names refresh first and top proposed candidates get
    // enough data to graduate into research.
    {
        let store = store.clone();
        let key = cfg.fmp_api_key.clone();
        let base = cfg.fmp_base_url.clone();
        tokio::spawn(async move {
            let adapter = FmpPriceAdapter::new(&key, &base);
            let interval = ingest::interval_secs_from_env("FMP_PRICE_INTERVAL_SECS", 30 * 60);
            loop {
                // Tiered deep universe ∪ benchmarks.
                let mut symbols: std::collections::BTreeSet<String> = Default::default();
                let max_symbols =
                    ingest::max_symbols_from_env("FMP_PRICE_MAX_SYMBOLS_PER_PASS", 125);
                if let Ok(p) = store.priority_scan_symbols(max_symbols).await {
                    symbols.extend(p);
                }
                for bench in ["SPY", "QQQ", "SMH", "VIXY"] {
                    symbols.insert(bench.to_string());
                }
                let symbols: Vec<String> = symbols.into_iter().collect();
                if !symbols.is_empty() {
                    if let Err(e) = store
                        .mark_source_started("fmp_price", symbols.len() as i32)
                        .await
                    {
                        error!(error = %e, "fmp_price source health start record failed");
                    }
                    let oldest = store
                        .oldest_bar_per_symbol(&symbols)
                        .await
                        .unwrap_or_default();
                    match adapter.poll_symbols(&symbols, &oldest).await {
                        Ok(rows) if rows.is_empty() => {
                            if let Err(e) = store
                                .record_source_success("fmp_price", 0, 0, symbols.len() as i32, 0)
                                .await
                            {
                                error!(error = %e, "fmp_price source health success record failed");
                            }
                        }
                        Ok(rows) => match store.upsert_price_bars(&rows).await {
                            Ok(inserted) => {
                                if let Err(e) = store
                                    .record_source_success(
                                        "fmp_price",
                                        rows.len() as i64,
                                        inserted as i64,
                                        symbols.len() as i32,
                                        0,
                                    )
                                    .await
                                {
                                    error!(error = %e, "fmp_price source health success record failed");
                                }
                                info!(
                                    symbols = symbols.len(),
                                    rows = rows.len(),
                                    inserted,
                                    "fmp price pass complete"
                                )
                            }
                            Err(e) => {
                                let message = e.to_string();
                                if let Err(record_err) = store
                                    .record_source_failure(
                                        "fmp_price",
                                        source_health::failure_kind(&message),
                                        &message,
                                        None,
                                    )
                                    .await
                                {
                                    error!(error = %record_err, "fmp_price source health failure record failed");
                                }
                                error!(error = %e, "fmp price persist failed")
                            }
                        },
                        Err(e) => {
                            let message = e.to_string();
                            let retry_after_at =
                                if source_health::failure_kind(&message) == "rate_limited" {
                                    rate_limit::fmp().retry_after_at().await
                                } else {
                                    None
                                };
                            if let Err(record_err) = store
                                .record_source_failure(
                                    "fmp_price",
                                    source_health::failure_kind(&message),
                                    &message,
                                    retry_after_at,
                                )
                                .await
                            {
                                error!(error = %record_err, "fmp_price source health failure record failed");
                            }
                            error!(error = %e, "fmp price poll failed")
                        }
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
            let interval = ingest::interval_secs_from_env("FMP_SCREENER_INTERVAL_SECS", 24 * 3600);
            if let Err(e) =
                stocks::ingest::discovery_pool_service::run(pool, adapter, interval).await
            {
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
            let interval = ingest::interval_secs_from_env("FMP_ESTIMATES_INTERVAL_SECS", 30 * 60);
            if let Err(e) = fmp_estimates_service::run(pool, adapter, interval).await {
                error!(error = %e, "fmp_estimates service exited");
            }
        });
    }

    // FMP analyst opinion (#116): price target consensus, recommendation mix,
    // and recent target events. This is separate from estimate revisions; it
    // tells cognition what the visible sell-side opinion already says.
    {
        let pool = store.pool.clone();
        let key = cfg.fmp_api_key.clone();
        let base = cfg.fmp_base_url.clone();
        tokio::spawn(async move {
            let adapter = FmpOpinionAdapter::new(&key, &base);
            let interval = ingest::interval_secs_from_env("FMP_OPINION_INTERVAL_SECS", 30 * 60);
            if let Err(e) = fmp_opinion_service::run(pool, adapter, interval).await {
                error!(error = %e, "fmp_analyst_opinion service exited");
            }
        });
    }

    // Crowd sentiment (#20): CBOE put/call + VIX daily CSV → crowd_sentiment.
    // Feeds the consensus retail_attention component.
    {
        let pool = store.pool.clone();
        tokio::spawn(async move {
            let adapter = CboeAdapter::new();
            let interval = ingest::interval_secs_from_env("CBOE_INTERVAL_SECS", 30 * 60);
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
                error!(
                    "news_service: prompts/score-sentiment.md missing; sentiment scoring disabled"
                );
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
            let interval = ingest::interval_secs_from_env("NEWS_INTERVAL_SECS", 30 * 60);
            if let Err(e) = news_service::run(svc, interval).await {
                error!(error = %e, "news_service exited");
            }
        });
    }

    // Spawn the XBRL bulk loop in parallel with the per-event adapter runner.
    // This is capped to the tiered deep universe; broad pool membership alone
    // is not enough to spend SEC/XBRL quota every pass.
    {
        let store = store.clone();
        let ua = cfg.sec_user_agent.clone();
        tokio::spawn(async move {
            let adapter = XbrlAdapter::new(&ua);
            let interval = ingest::interval_secs_from_env("XBRL_INTERVAL_SECS", 6 * 3600);
            // First-run fires immediately so a fresh deploy populates company_fact.
            loop {
                let max_symbols = ingest::max_symbols_from_env("XBRL_MAX_SYMBOLS_PER_PASS", 100);
                let symbols = store
                    .priority_scan_symbols(max_symbols)
                    .await
                    .unwrap_or_default();
                let attempted = symbols.len() as i32;
                if let Err(e) = store.mark_source_started("xbrl", attempted).await {
                    error!(error = %e, "xbrl source health start record failed");
                }
                match adapter.poll_symbols(&symbols).await {
                    Ok((rows, missing_cik_count, failed_fetch_count)) => {
                        match store.upsert_company_facts(&rows).await {
                            Ok(inserted) => {
                                let failed = (missing_cik_count + failed_fetch_count) as i32;
                                if let Err(e) = store
                                    .record_source_success(
                                        "xbrl",
                                        rows.len() as i64,
                                        inserted as i64,
                                        attempted,
                                        failed,
                                    )
                                    .await
                                {
                                    error!(error = %e, "xbrl source health success record failed");
                                }
                                info!(
                                    symbols = symbols.len(),
                                    rows = rows.len(),
                                    inserted,
                                    missing_cik_count,
                                    failed_fetch_count,
                                    "xbrl pass complete"
                                )
                            }
                            Err(e) => {
                                let message = e.to_string();
                                if let Err(record_err) = store
                                    .record_source_failure(
                                        "xbrl",
                                        source_health::failure_kind(&message),
                                        &message,
                                        None,
                                    )
                                    .await
                                {
                                    error!(error = %record_err, "xbrl source health failure record failed");
                                }
                                error!(error = %e, "xbrl persist failed")
                            }
                        }
                    }
                    Err(e) => {
                        let message = e.to_string();
                        if let Err(record_err) = store
                            .record_source_failure(
                                "xbrl",
                                source_health::failure_kind(&message),
                                &message,
                                None,
                            )
                            .await
                        {
                            error!(error = %record_err, "xbrl source health failure record failed");
                        }
                        error!(error = %e, "xbrl poll failed")
                    }
                }
                tokio::time::sleep(interval).await;
            }
        });
    }

    // EDGAR filings follow the same tiered deep universe as XBRL. The broad
    // screener pool should nominate first; SEC loops should not spend every
    // 30-minute pass walking 1,000+ low-rank names.
    {
        let store = store.clone();
        let bus = bus.clone();
        let ua = cfg.sec_user_agent.clone();
        tokio::spawn(async move {
            let adapter = EdgarAdapter::new(&ua);
            let interval = ingest::interval_secs_from_env("EDGAR_INTERVAL_SECS", 30 * 60);
            loop {
                let max_symbols = ingest::max_symbols_from_env("EDGAR_MAX_SYMBOLS_PER_PASS", 100);
                let symbols = store
                    .priority_scan_symbols(max_symbols)
                    .await
                    .unwrap_or_default();
                let attempted = symbols.len() as i32;
                if let Err(e) = store.mark_source_started("edgar", attempted).await {
                    error!(error = %e, "edgar source health start record failed");
                }
                match adapter.poll_symbols(&symbols).await {
                    Ok((events, missing_cik_count, failed_fetch_count)) => {
                        let rows_seen = events.len() as i64;
                        match persist_events(&store, &bus, "edgar", events).await {
                            Ok((stored, published)) => {
                                let failed = (missing_cik_count + failed_fetch_count) as i32;
                                if let Err(e) = store
                                    .record_source_success(
                                        "edgar",
                                        rows_seen,
                                        stored as i64,
                                        attempted,
                                        failed,
                                    )
                                    .await
                                {
                                    error!(error = %e, "edgar source health success record failed");
                                }
                                if stored > 0 {
                                    info!(
                                        symbols = symbols.len(),
                                        new = stored,
                                        published,
                                        missing_cik_count,
                                        failed_fetch_count,
                                        "edgar filings pass complete"
                                    );
                                }
                            }
                            Err(e) => {
                                let message = e.to_string();
                                if let Err(record_err) = store
                                    .record_source_failure(
                                        "edgar",
                                        source_health::failure_kind(&message),
                                        &message,
                                        None,
                                    )
                                    .await
                                {
                                    error!(error = %record_err, "edgar source health failure record failed");
                                }
                                error!(error = %e, "edgar filings persist failed");
                            }
                        }
                    }
                    Err(e) => {
                        let message = e.to_string();
                        if let Err(record_err) = store
                            .record_source_failure(
                                "edgar",
                                source_health::failure_kind(&message),
                                &message,
                                None,
                            )
                            .await
                        {
                            error!(error = %record_err, "edgar source health failure record failed");
                        }
                        error!(error = %e, "edgar filings poll failed");
                    }
                }
                tokio::time::sleep(interval).await;
            }
        });
    }

    let adapters: Vec<Box<dyn ingest::Adapter>> =
        vec![Box::new(FredAdapter::new(&cfg.fred_api_key))];

    info!("ingestion started");
    ingest::run(store, bus, adapters, async {
        let _ = tokio::signal::ctrl_c().await;
        info!("shutdown signal received");
    })
    .await
}

async fn persist_events(
    store: &Store,
    bus: &Bus,
    adapter_name: &str,
    events: Vec<ingest::Event>,
) -> Result<(u32, u32)> {
    let mut stored = 0u32;
    let mut published = 0u32;
    for ev in events {
        let symbol_opt = if ev.symbol.is_empty() {
            None
        } else {
            Some(ev.symbol.as_str())
        };
        let inserted = store
            .append_ingest_event(
                &ev.source,
                &ev.kind,
                symbol_opt,
                &ev.payload,
                &ev.content_hash(),
                ev.source_ts,
            )
            .await?;
        if !inserted {
            continue;
        }
        stored += 1;
        if !ev.subject.is_empty() {
            bus.publish(&ev.subject, &ev.payload).await?;
            published += 1;
        }
    }
    if stored > 0 {
        info!(adapter = adapter_name, new = stored, published, "ingested");
    }
    Ok((stored, published))
}
