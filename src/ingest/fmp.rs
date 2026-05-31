//! Financial Modeling Prep adapter — replaces Massive as the OHLCV source
//! (#60 / #18). Uses `/stable/historical-price-eod/full` which returns
//! adjusted OHLCV plus VWAP, going back 30+ years.
//!
//! Auth via `?apikey=` query param. Rate limit on Starter is 300 req/min
//! (~5/sec); we pace 200ms between symbols to stay safe and well under
//! anything Finnhub-style adapters might hit later.
//!
//! Without `FMP_API_KEY` set the adapter no-ops with a one-shot warning,
//! same pattern as the FRED and Massive adapters.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{Duration as ChronoDuration, NaiveDate, TimeZone, Utc};
use reqwest::Client;
use serde::Deserialize;
use tracing::warn;

use super::massive::PriceBarRow;
use super::sec;

/// Benchmarks pulled alongside the seeded universe — same set we pulled
/// from Massive so consensus.price_extension and the regime classifier
/// keep working unchanged.
const BENCHMARKS: &[&str] = &["SPY", "QQQ", "SMH", "VIXY"];

pub struct FmpPriceAdapter {
    api_key: String,
    base_url: String,
    client: Client,
    warned_no_key: AtomicBool,
}

impl FmpPriceAdapter {
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

/// One bar in FMP's shape. Fields beyond OHLCV (change, changePercent, vwap)
/// are present but we don't persist them — `price_bar` columns are fixed
/// across vendors.
#[derive(Debug, Clone, Deserialize)]
pub struct FmpBar {
    pub symbol: String,
    pub date: String, // "YYYY-MM-DD"
    #[serde(default)]
    pub open: f64,
    #[serde(default)]
    pub high: f64,
    #[serde(default)]
    pub low: f64,
    #[serde(default)]
    pub close: f64,
    #[serde(default)]
    pub volume: f64,
}

/// Pure function: convert FMP response into normalized `PriceBarRow`s.
/// Drops rows whose date doesn't parse — we'd rather lose one bar than
/// fail the whole batch.
///
/// Timestamps are anchored at **04:00 UTC** (= 00:00 ET during EDT) to match
/// the convention Polygon/Massive served (its `t` ms-since-epoch field
/// landed at the start of the US trading day in ET). Keeping this convention
/// means existing rows ingested via Massive dedup correctly against new
/// FMP rows on the `(symbol, ts)` primary key.
#[must_use]
pub fn to_rows(symbol: &str, bars: &[FmpBar]) -> Vec<PriceBarRow> {
    bars.iter()
        .filter_map(|b| {
            let nd = NaiveDate::parse_from_str(&b.date, "%Y-%m-%d").ok()?;
            let ts = Utc.from_utc_datetime(&nd.and_hms_opt(4, 0, 0)?);
            Some(PriceBarRow {
                symbol: symbol.to_string(),
                ts,
                open: b.open,
                high: b.high,
                low: b.low,
                close: b.close,
                volume: b.volume,
            })
        })
        .collect()
}

impl FmpPriceAdapter {
    /// Fetch the last `lookback_days` of daily bars for one symbol.
    pub async fn fetch_one(&self, symbol: &str, lookback_days: i64) -> Result<Vec<PriceBarRow>> {
        if self.api_key.is_empty() {
            return Ok(Vec::new());
        }
        let today = Utc::now().date_naive();
        let from = today - ChronoDuration::days(lookback_days);
        let url = format!(
            "{}/stable/historical-price-eod/full?symbol={symbol}&from={from}&to={today}&apikey={key}",
            self.base_url,
            from = from.format("%Y-%m-%d"),
            today = today.format("%Y-%m-%d"),
            key = self.api_key,
        );
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .with_context(|| format!("fmp price fetch {symbol}"))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("fmp {symbol} {}: {}", status.as_u16(), &body[..body.len().min(256)]);
        }
        let parsed: Vec<FmpBar> = resp
            .json()
            .await
            .with_context(|| format!("fmp decode {symbol}"))?;
        Ok(to_rows(symbol, &parsed))
    }

    /// Poll a list of symbols using per-ticker incremental backfill:
    /// - First-ever sight (no existing bars) → 5y backfill
    /// - Subsequent polls → 30d window (idempotent on (symbol, ts) PK)
    ///
    /// `existing_min_ts_by_symbol` lets the caller pass the oldest bar we
    /// already have per symbol (cheap one-shot query). When the entry is
    /// absent OR `None`, we treat the symbol as cold and pull 5y.
    pub async fn poll_symbols(
        &self,
        symbols: &[String],
        existing_min_ts_by_symbol: &std::collections::HashMap<String, Option<chrono::DateTime<Utc>>>,
    ) -> Result<Vec<PriceBarRow>> {
        if self.api_key.is_empty() {
            if !self.warned_no_key.swap(true, Ordering::Relaxed) {
                warn!("fmp: FMP_API_KEY not set; skipping price ingest");
            }
            return Ok(Vec::new());
        }
        let mut all = Vec::new();
        let five_years = 365 * 5;
        for sym in symbols {
            let cold = !matches!(existing_min_ts_by_symbol.get(sym), Some(Some(_)));
            let lookback_days = if cold { five_years } else { 30 };
            match self.fetch_one(sym, lookback_days).await {
                Ok(rows) => {
                    tracing::info!(
                        symbol = %sym, bars = rows.len(), cold,
                        lookback_days,
                        "fmp bars fetched"
                    );
                    all.extend(rows);
                }
                Err(e) => {
                    tracing::warn!(symbol = %sym, error = %e, "fmp fetch failed; continuing");
                }
            }
            // 200ms = 5 req/s. Safe under FMP Starter's 300/min cap.
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        Ok(all)
    }

    /// Legacy seeded-list path (used by integration tests). Prefer poll_symbols.
    pub async fn poll_all(&self, lookback_days: i64) -> Result<Vec<PriceBarRow>> {
        if self.api_key.is_empty() {
            if !self.warned_no_key.swap(true, Ordering::Relaxed) {
                warn!("fmp: FMP_API_KEY not set; skipping price ingest");
            }
            return Ok(Vec::new());
        }
        let mut all = Vec::new();
        let symbols: Vec<&str> = sec::all_seeded()
            .map(|(s, _)| s)
            .chain(BENCHMARKS.iter().copied())
            .collect();
        for sym in symbols {
            if let Ok(rows) = self.fetch_one(sym, lookback_days).await {
                all.extend(rows);
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        Ok(all)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Datelike;

    fn sample_bars() -> Vec<FmpBar> {
        // Verbatim shape from a live FMP /stable/historical-price-eod/full
        // probe against MU on 2026-05-31. Real wire format, real numbers.
        serde_json::from_value(serde_json::json!([
            {"symbol":"MU","date":"2024-01-10","open":82.96,"high":83.02,"low":81.66,"close":82.38,"volume":10748625,"change":-0.575,"changePercent":-0.69913,"vwap":82.505},
            {"symbol":"MU","date":"2024-01-09","open":83.13,"high":84.19,"low":82.9, "close":83.33,"volume":12138742,"change":0.2,   "changePercent":0.24059, "vwap":83.3875},
            {"symbol":"MU","date":"2024-01-08","open":83.89,"high":85.51,"low":83.83,"close":84.95,"volume":16219817,"change":1.06,  "changePercent":1.26,    "vwap":84.555}
        ])).unwrap()
    }

    #[test]
    fn to_rows_decodes_a_real_response() {
        let rows = to_rows("MU", &sample_bars());
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].symbol, "MU");
        assert_eq!(rows[0].open, 82.96);
        assert_eq!(rows[0].close, 82.38);
        assert_eq!(rows[2].volume, 16219817.0);
    }

    #[test]
    fn to_rows_anchors_bars_to_market_open_et() {
        // 04:00 UTC = 00:00 ET (during EDT). Matches the convention Polygon
        // served when we were on Massive, so existing rows dedup on the
        // (symbol, ts) PK instead of duplicating.
        let rows = to_rows("MU", &sample_bars());
        let d = rows[0].ts.date_naive();
        assert_eq!(d.year(), 2024);
        assert_eq!(d.month(), 1);
        assert_eq!(d.day(), 10);
        assert_eq!(rows[0].ts.time().to_string(), "04:00:00");
    }

    #[test]
    fn to_rows_drops_unparseable_date() {
        let mut bars = sample_bars();
        bars[1].date = "not-a-date".into();
        let rows = to_rows("MU", &bars);
        assert_eq!(rows.len(), 2, "bad date dropped, the other two kept");
    }

    #[test]
    fn to_rows_handles_empty() {
        let rows = to_rows("X", &[]);
        assert!(rows.is_empty());
    }

    #[test]
    fn ordering_is_preserved_from_api_response() {
        // FMP returns newest-first by default; we don't sort. Caller upserts
        // with ON CONFLICT so order doesn't matter for correctness, but the
        // sequence is preserved here so downstream consumers (e.g. log lines)
        // see the same order the API gave.
        let rows = to_rows("MU", &sample_bars());
        assert!(rows[0].ts > rows[2].ts, "first row is most-recent");
    }
}
