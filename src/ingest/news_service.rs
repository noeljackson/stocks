//! News ingest service (#19).
//!
//! Per-pass flow per universe ticker:
//!   1. Fetch from FMP (`/stable/news/stock`) — no upstream sentiment
//!   2. Fetch from Massive (`/v2/reference/news`) — upstream sentiment per insight
//!   3. Upsert each article into `news_article` (dedup by URL, fall back to
//!      `(symbol, title, published_at)`)
//!   4. For rows that landed with NULL sentiment, queue an LLM-scoring call
//!      via `sentiment::score_one` and patch the row when it returns
//!
//! Sentiment scoring is opt-in via the `scorer` callback so callers without
//! a configured LLM provider still get news ingest (sentiment=null, scored
//! later when scorer becomes available). Keeps the wiring decoupled from
//! `crate::llm::*` setup which lives in the gateway binary.

use std::time::Duration;

use anyhow::{Context, Result};
use sqlx::Row;
use sqlx::postgres::PgPool;
use tracing::{info, warn};

use super::fmp_news::{FmpNewsAdapter, NewsArticle};
use super::massive_news::MassiveNewsAdapter;
use super::{max_symbols_from_env, rate_limit, source_health};
use crate::sentiment::SentimentScore;

/// Identifies a Sentiment-scorer callback. Returns the scored result OR
/// an error (caller will log + skip that article, NOT fail the pass).
pub type ScorerFn = std::sync::Arc<
    dyn for<'a> Fn(
            &'a str, // ticker
            &'a str, // title
            &'a str, // body
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<SentimentScore>> + Send + 'a>,
        > + Send
        + Sync,
>;

pub struct NewsIngestService {
    pub pool: PgPool,
    pub fmp: FmpNewsAdapter,
    pub massive: MassiveNewsAdapter,
    /// Optional LLM scorer. If `None`, articles without upstream sentiment
    /// stay with `sentiment IS NULL` and will be picked up by a later pass
    /// once a scorer is wired in.
    pub scorer: Option<ScorerFn>,
    /// Audit metadata for LLM-scored rows.
    pub prompt_name: String,
    pub prompt_hash: String,
    /// Max articles per ticker per pass.
    pub per_ticker_limit: u32,
}

impl NewsIngestService {
    /// One pass over the tiered deep-research universe. Returns (inserted, scored).
    pub async fn run_once(&self) -> Result<(usize, usize)> {
        let store = crate::platform::store::Store {
            pool: self.pool.clone(),
        };
        let max_symbols = max_symbols_from_env("NEWS_MAX_SYMBOLS_PER_PASS", 100);
        let universe_owned: Vec<String> = store
            .priority_scan_symbols(max_symbols)
            .await
            .unwrap_or_default();
        let universe: Vec<&str> = universe_owned.iter().map(String::as_str).collect();
        source_health::mark_started(&self.pool, "fmp_news", universe.len() as i32).await?;
        source_health::mark_started(&self.pool, "massive_news", universe.len() as i32).await?;

        let mut inserted_total = 0;
        let mut inserted_fmp = 0;
        let mut inserted_massive = 0;
        let mut fmp_rows_seen = 0;
        let mut massive_rows_seen = 0;
        let mut fmp_failed = 0;
        let mut massive_failed = 0;
        let mut saw_fmp_rate_limit = false;
        let mut scored_total = 0;

        for symbol in &universe {
            // --- 1+2 fetch from both vendors ---
            let fmp_rows = match self.fmp.fetch_one(symbol, self.per_ticker_limit).await {
                Ok(r) => r,
                Err(e) => {
                    fmp_failed += 1;
                    if source_health::failure_kind(&e.to_string()) == "rate_limited" {
                        saw_fmp_rate_limit = true;
                    }
                    warn!(symbol = symbol, error = %e, "fmp news fetch failed");
                    Vec::new()
                }
            };
            fmp_rows_seen += fmp_rows.len();
            let massive_rows = match self
                .massive
                .fetch_one(symbol, self.per_ticker_limit, &universe)
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    massive_failed += 1;
                    warn!(symbol = symbol, error = %e, "massive news fetch failed");
                    Vec::new()
                }
            };
            massive_rows_seen += massive_rows.len();

            // --- 3 upsert ---
            for a in &fmp_rows {
                match upsert_article(&self.pool, a, None, None).await {
                    Ok(true) => {
                        inserted_total += 1;
                        inserted_fmp += 1;
                    }
                    Ok(false) => {}
                    Err(e) => warn!(symbol = symbol, error = ?e, "upsert fmp article failed"),
                }
            }
            for s in &massive_rows {
                match upsert_article(
                    &self.pool,
                    &s.article,
                    s.upstream_sentiment.as_deref(),
                    s.upstream_rationale.as_deref(),
                )
                .await
                {
                    Ok(true) => {
                        inserted_total += 1;
                        inserted_massive += 1;
                    }
                    Ok(false) => {}
                    Err(e) => warn!(symbol = symbol, error = ?e, "upsert massive article failed"),
                }
            }
            // Pace between symbols to stay under rate limits on either vendor.
            tokio::time::sleep(Duration::from_millis(150)).await;
        }

        // --- 4 LLM-score everything still NULL ---
        if let Some(scorer) = self.scorer.as_ref() {
            scored_total = self.score_pending(scorer).await?;
        }

        source_health::record_success(
            &self.pool,
            "fmp_news",
            fmp_rows_seen as i64,
            inserted_fmp as i64,
            universe.len() as i32,
            fmp_failed,
        )
        .await?;
        source_health::record_success(
            &self.pool,
            "massive_news",
            massive_rows_seen as i64,
            inserted_massive as i64,
            universe.len() as i32,
            massive_failed,
        )
        .await?;
        if saw_fmp_rate_limit {
            source_health::record_failure(
                &self.pool,
                "fmp_news",
                "rate_limited",
                "one or more FMP news requests were rate limited",
                rate_limit::fmp().retry_after_at().await,
            )
            .await?;
        }
        Ok((inserted_total, scored_total))
    }

    async fn score_pending(&self, scorer: &ScorerFn) -> Result<usize> {
        let rows = sqlx::query(
            r#"SELECT id, symbol, title, COALESCE(body, '') AS body
                 FROM news_article
                WHERE sentiment IS NULL
             ORDER BY id DESC LIMIT 50"#,
        )
        .fetch_all(&self.pool)
        .await
        .context("load unscored news")?;
        let mut n = 0;
        for row in rows {
            let id: i64 = row.try_get("id")?;
            let symbol: String = row.try_get("symbol")?;
            let title: String = row.try_get("title")?;
            let body: String = row.try_get("body")?;
            let score = match scorer(&symbol, &title, &body).await {
                Ok(s) if s.is_valid() => s,
                Ok(s) => {
                    warn!(id, ?s, "sentiment score failed validation");
                    continue;
                }
                Err(e) => {
                    warn!(id, error = %e, "sentiment scorer errored");
                    continue;
                }
            };
            if let Err(e) = sqlx::query(
                r#"UPDATE news_article
                      SET sentiment = $1, sentiment_polarity = $2,
                          sentiment_confidence = $3, sentiment_source = 'llm',
                          sentiment_rationale = $4,
                          prompt_name = $5, prompt_hash = $6, scored_at = now()
                    WHERE id = $7 AND sentiment IS NULL"#,
            )
            .bind(&score.sentiment)
            .bind(score.polarity)
            .bind(&score.confidence)
            .bind(&score.rationale)
            .bind(&self.prompt_name)
            .bind(&self.prompt_hash)
            .bind(id)
            .execute(&self.pool)
            .await
            {
                warn!(id, error = %e, "patch sentiment failed");
                continue;
            }
            n += 1;
        }
        Ok(n)
    }
}

/// Returns `true` if a fresh row was inserted, `false` if an existing row
/// matched on the dedup index. When `upstream_sentiment` is provided we
/// also populate the sentiment columns so the scorer doesn't re-process it.
async fn upsert_article(
    pool: &PgPool,
    a: &NewsArticle,
    upstream_sentiment: Option<&str>,
    upstream_rationale: Option<&str>,
) -> Result<bool> {
    // Coerce to the DB CHECK constraint set; null out anything we don't recognise
    // (Massive occasionally emits "" / "unknown"), so the LLM scorer fills it in.
    let upstream_sentiment = upstream_sentiment.and_then(|s| match s {
        "positive" | "neutral" | "negative" => Some(s),
        _ => None,
    });
    let polarity: Option<f64> = upstream_sentiment.map(|s| match s {
        "positive" => 0.5,
        "negative" => -0.5,
        _ => 0.0,
    });
    let sentiment_source: Option<&str> = upstream_sentiment.map(|_| "upstream");
    let res = sqlx::query(
        r#"INSERT INTO news_article
             (symbol, title, body, url, publisher, published_at, source,
              sentiment, sentiment_polarity, sentiment_source, sentiment_rationale)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
           ON CONFLICT DO NOTHING"#,
    )
    .bind(&a.symbol)
    .bind(&a.title)
    .bind(&a.body)
    .bind(&a.url)
    .bind(&a.publisher)
    .bind(a.published_at)
    .bind(a.source)
    .bind(upstream_sentiment)
    .bind(polarity)
    .bind(sentiment_source)
    .bind(upstream_rationale)
    .execute(pool)
    .await
    .context("insert news_article")?;
    Ok(res.rows_affected() > 0)
}

/// Long-running service loop.
pub async fn run(service: NewsIngestService, interval: Duration) -> Result<()> {
    info!(
        interval_secs = interval.as_secs(),
        "news ingest service started"
    );
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        match service.run_once().await {
            Ok((ins, scored)) if ins > 0 || scored > 0 => {
                info!(inserted = ins, scored, "news pass complete");
            }
            Ok(_) => {}
            Err(e) => {
                let message = e.to_string();
                let retry_after_at = if source_health::failure_kind(&message) == "rate_limited" {
                    rate_limit::fmp().retry_after_at().await
                } else {
                    None
                };
                if let Err(record_err) = source_health::record_failure(
                    &service.pool,
                    "fmp_news",
                    source_health::failure_kind(&message),
                    &message,
                    retry_after_at,
                )
                .await
                {
                    warn!(error = %record_err, "news source health failure record failed");
                }
                warn!(error = %e, "news pass failed");
            }
        }
    }
}
