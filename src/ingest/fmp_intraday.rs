//! FMP intraday price adapter for chart intervals.
//!
//! FMP exposes native 1min, 5min, 15min, 30min, 1hour, and 4hour bars at
//! `/stable/historical-chart/{interval}`. The chart route can aggregate native
//! 1min bars into 3m and native 1hour bars into 2h.

use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{NaiveDateTime, TimeZone, Utc};
use reqwest::Client;
use serde::Deserialize;

use super::rate_limit;

#[derive(Debug, Clone)]
pub struct IntradayPriceBarRow {
    pub symbol: String,
    pub interval: String,
    pub ts: chrono::DateTime<Utc>,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
}

pub struct FmpIntradayAdapter {
    api_key: String,
    base_url: String,
    client: Client,
}

impl FmpIntradayAdapter {
    pub fn new(api_key: &str, base_url: &str) -> Self {
        Self {
            api_key: api_key.to_string(),
            base_url: base_url.trim_end_matches('/').to_string(),
            client: Client::builder()
                .timeout(Duration::from_secs(20))
                .build()
                .expect("reqwest client"),
        }
    }

    #[must_use]
    pub fn configured(&self) -> bool {
        !self.api_key.is_empty()
    }

    pub async fn fetch_one(
        &self,
        symbol: &str,
        native_interval: &str,
        lookback_days: i64,
    ) -> Result<Vec<IntradayPriceBarRow>> {
        if self.api_key.is_empty() {
            return Ok(Vec::new());
        }
        let today = Utc::now().date_naive();
        let from = today - chrono::Duration::days(lookback_days);
        let url = format!(
            "{}/stable/historical-chart/{native_interval}?symbol={symbol}&from={from}&to={today}&apikey={key}",
            self.base_url,
            from = from.format("%Y-%m-%d"),
            today = today.format("%Y-%m-%d"),
            key = self.api_key,
        );
        rate_limit::fmp().wait().await;
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .with_context(|| format!("fmp intraday fetch {symbol} {native_interval}"))?;
        let status = resp.status();
        let retry_after = rate_limit::retry_after(resp.headers());
        rate_limit::fmp().observe_status(status, retry_after).await;
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "fmp intraday {symbol} {native_interval} {}: {}",
                status.as_u16(),
                &body[..body.len().min(256)]
            );
        }
        let parsed: Vec<FmpIntradayBar> = resp
            .json()
            .await
            .with_context(|| format!("fmp intraday decode {symbol} {native_interval}"))?;
        Ok(to_rows(symbol, native_interval, &parsed))
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct FmpIntradayBar {
    pub date: String,
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

#[must_use]
pub fn to_rows(
    symbol: &str,
    native_interval: &str,
    bars: &[FmpIntradayBar],
) -> Vec<IntradayPriceBarRow> {
    bars.iter()
        .filter_map(|b| {
            let ndt = NaiveDateTime::parse_from_str(&b.date, "%Y-%m-%d %H:%M:%S").ok()?;
            Some(IntradayPriceBarRow {
                symbol: symbol.to_string(),
                interval: native_interval.to_string(),
                ts: Utc.from_utc_datetime(&ndt),
                open: b.open,
                high: b.high,
                low: b.low,
                close: b.close,
                volume: b.volume,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_rows_decodes_fmp_intraday_shape() {
        let bars: Vec<FmpIntradayBar> = serde_json::from_value(serde_json::json!([
            {"date":"2026-05-29 15:59:00","open":100.0,"high":101.0,"low":99.5,"close":100.5,"volume":12345},
            {"date":"bad","open":200.0,"high":201.0,"low":199.0,"close":200.5,"volume":1}
        ]))
        .unwrap();
        let rows = to_rows("NVDA", "1min", &bars);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].symbol, "NVDA");
        assert_eq!(rows[0].interval, "1min");
        assert_eq!(rows[0].close, 100.5);
        assert_eq!(rows[0].volume, 12345.0);
        assert_eq!(rows[0].ts.to_rfc3339(), "2026-05-29T15:59:00+00:00");
    }
}
