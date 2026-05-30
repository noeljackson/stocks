//! Massive (= Polygon) price-bar adapter (#17). Polls daily OHLCV for the
//! seeded ticker universe + benchmark tickers (SPY/QQQ/SMH) and upserts
//! into `price_bar`. Auth via `?apiKey=` query param (Polygon-compat).
//!
//! Rate limits: Starter tier is unlimited; Free is 5 req/min. The adapter
//! paces itself at 200ms between symbols → 5 req/sec, safe for any paid
//! tier and well under the free 5/min when N ≤ 12 tickers (we have 8 Tier-1
//! + 4 benchmarks → 12 reqs, 1 minute-worth on free, sub-second on paid).
//!
//! Without `MASSIVE_API_KEY` set the adapter no-ops with a one-shot warning,
//! same pattern as the FRED adapter.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{Duration as ChronoDuration, TimeZone, Utc};
use reqwest::Client;
use serde::Deserialize;
use tracing::warn;

use super::sec; // shared seeded-ticker list
use super::{Adapter, Event};

/// Benchmark tickers we always pull alongside the Tier-1 universe so the
/// regime classifier has SPX/QQQ/SMH inputs and the consensus calculator
/// can compute price-extension vs SMA / RSI.
const BENCHMARKS: &[&str] = &["SPY", "QQQ", "SMH", "VIXY"];

pub struct MassiveAdapter {
    api_key: String,
    base_url: String,
    client: Client,
    warned_no_key: AtomicBool,
}

impl MassiveAdapter {
    pub fn new(api_key: &str, base_url: &str) -> Self {
        Self {
            api_key: api_key.to_string(),
            base_url: base_url.trim_end_matches('/').to_string(),
            client: Client::builder()
                .timeout(Duration::from_secs(20))
                .build()
                .expect("reqwest client"),
            warned_no_key: AtomicBool::new(false),
        }
    }
}

/// One bar in the Polygon-compat shape. Field names are single letters
/// because that's what the API returns — we expand them when persisting
/// to `price_bar`.
#[derive(Debug, Clone, Deserialize)]
pub struct Bar {
    /// open
    #[serde(default)]
    pub o: f64,
    /// high
    #[serde(default)]
    pub h: f64,
    /// low
    #[serde(default)]
    pub l: f64,
    /// close
    #[serde(default)]
    pub c: f64,
    /// volume
    #[serde(default)]
    pub v: f64,
    /// volume-weighted average price
    #[serde(default)]
    pub vw: Option<f64>,
    /// unix-millis timestamp at the start of the bar
    pub t: i64,
    /// transaction count
    #[serde(default)]
    pub n: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AggsResponse {
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub ticker: String,
    #[serde(default)]
    pub results: Vec<Bar>,
    #[serde(default)]
    #[serde(rename = "queryCount")]
    pub query_count: Option<i64>,
    #[serde(default)]
    #[serde(rename = "resultsCount")]
    pub results_count: Option<i64>,
}

/// Normalized bar ready for `price_bar` UPSERT.
#[derive(Debug, Clone, PartialEq)]
pub struct PriceBarRow {
    pub symbol: String,
    pub ts: chrono::DateTime<Utc>,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
}

/// Pure function: convert one API response into normalized rows.
#[must_use]
pub fn to_rows(symbol: &str, resp: &AggsResponse) -> Vec<PriceBarRow> {
    resp.results
        .iter()
        .filter_map(|b| {
            let ts = Utc.timestamp_millis_opt(b.t).single()?;
            Some(PriceBarRow {
                symbol: symbol.to_string(),
                ts,
                open: b.o,
                high: b.h,
                low: b.l,
                close: b.c,
                volume: b.v,
            })
        })
        .collect()
}

#[async_trait]
impl Adapter for MassiveAdapter {
    fn name(&self) -> &str {
        "massive"
    }
    fn interval(&self) -> Duration {
        // Daily bars; poll once per day after market close is plenty. We
        // poll every 6h so dev iteration sees fresh data sooner.
        Duration::from_secs(6 * 3600)
    }
    async fn poll(&self) -> Result<Vec<Event>> {
        // The MassiveAdapter writes directly to `price_bar` via the runner's
        // tee path (see cmd/ingest), bypassing per-event publish. Returning
        // empty here — the actual fetch+persist happens through poll_all().
        Ok(Vec::new())
    }
}

impl MassiveAdapter {
    /// Fetch the last `lookback_days` of daily bars for one symbol.
    pub async fn fetch_one(&self, symbol: &str, lookback_days: i64) -> Result<Vec<PriceBarRow>> {
        if self.api_key.is_empty() {
            return Ok(Vec::new()); // caller already warned
        }
        let today = Utc::now().date_naive();
        let from = today - ChronoDuration::days(lookback_days);
        let url = format!(
            "{}/v2/aggs/ticker/{symbol}/range/1/day/{from}/{today}?adjusted=true&sort=asc&limit=50000&apiKey={key}",
            self.base_url,
            symbol = symbol,
            from = from.format("%Y-%m-%d"),
            today = today.format("%Y-%m-%d"),
            key = self.api_key,
        );
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .with_context(|| format!("massive fetch {symbol}"))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("massive {symbol} {}: {}", status.as_u16(), &body[..body.len().min(256)]);
        }
        let parsed: AggsResponse = resp
            .json()
            .await
            .with_context(|| format!("massive decode {symbol}"))?;
        Ok(to_rows(symbol, &parsed))
    }

    /// Poll every seeded Tier-1 ticker + benchmarks. Caller persists.
    pub async fn poll_all(&self, lookback_days: i64) -> Result<Vec<PriceBarRow>> {
        if self.api_key.is_empty() {
            if !self.warned_no_key.swap(true, Ordering::Relaxed) {
                warn!("massive: MASSIVE_API_KEY not set; skipping price ingest");
            }
            return Ok(Vec::new());
        }
        let mut all = Vec::new();
        // Universe: Tier-1 seeded names + market benchmarks.
        let symbols: Vec<&str> = sec::all_seeded()
            .map(|(s, _)| s)
            .chain(BENCHMARKS.iter().copied())
            .collect();
        for sym in symbols {
            match self.fetch_one(sym, lookback_days).await {
                Ok(rows) => {
                    tracing::info!(symbol = sym, bars = rows.len(), "massive bars fetched");
                    all.extend(rows);
                }
                Err(e) => {
                    tracing::warn!(symbol = sym, error = %e, "massive fetch failed; continuing");
                }
            }
            // 200ms = 5 req/s. Safe for any paid tier; free tier (5/min) will
            // still throttle but the per-symbol error is non-fatal.
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        Ok(all)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Datelike;

    fn sample_response() -> AggsResponse {
        serde_json::from_value(serde_json::json!({
            "ticker": "NVDA",
            "queryCount": 2,
            "resultsCount": 2,
            "adjusted": true,
            "results": [
                {"o":140.0,"h":145.0,"l":139.5,"c":143.2,"v":120000000.0,"vw":142.1,"t":1748563200000_i64,"n":987654},
                {"o":143.5,"h":146.8,"l":143.0,"c":145.7,"v":98000000.0,"vw":145.0,"t":1748649600000_i64,"n":876543}
            ],
            "status": "OK",
            "request_id": "abc"
        })).unwrap()
    }

    #[test]
    fn to_rows_decodes_bars_in_chronological_order() {
        let resp = sample_response();
        let rows = to_rows("NVDA", &resp);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].symbol, "NVDA");
        assert_eq!(rows[0].open, 140.0);
        assert_eq!(rows[1].close, 145.7);
        // First bar should be older than second.
        assert!(rows[0].ts < rows[1].ts);
    }

    #[test]
    fn to_rows_skips_invalid_timestamps() {
        let mut resp = sample_response();
        // i64::MAX as millis is out of chrono's range → single() returns None.
        resp.results[0].t = i64::MAX;
        let rows = to_rows("NVDA", &resp);
        assert_eq!(rows.len(), 1, "bad timestamp dropped, valid one kept");
    }

    #[test]
    fn to_rows_handles_empty_results() {
        let mut resp = sample_response();
        resp.results.clear();
        assert!(to_rows("X", &resp).is_empty());
    }

    #[test]
    fn timestamp_decodes_to_known_date() {
        let resp = sample_response();
        let rows = to_rows("NVDA", &resp);
        // 1748563200000 ms = 2025-05-30 00:00:00 UTC
        let date = rows[0].ts.date_naive();
        assert_eq!(date.year(), 2025);
        assert_eq!(date.month(), 5);
        assert_eq!(date.day(), 30);
    }
}
