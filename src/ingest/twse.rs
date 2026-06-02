//! Taiwan Stock Exchange daily OHLCV fallback.
//!
//! FMP search knows symbols such as 2454.TW, but the configured EOD endpoint
//! can be entitlement-gated for Taiwan listings. TWSE's public STOCK_DAY
//! endpoint provides official daily bars by month with no API key.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{Datelike, Duration as ChronoDuration, NaiveDate, TimeZone, Utc};
use reqwest::Client;
use serde::Deserialize;
use tracing::warn;

use super::massive::PriceBarRow;

pub struct TwseDailyAdapter {
    base_url: String,
    client: Client,
}

impl TwseDailyAdapter {
    pub fn new(base_url: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client: Client::builder()
                .timeout(Duration::from_secs(20))
                .build()
                .expect("reqwest client"),
        }
    }

    #[must_use]
    pub fn supports_symbol(symbol: &str) -> bool {
        twse_stock_no(symbol).is_some()
    }

    pub async fn fetch_one(&self, symbol: &str, lookback_days: i64) -> Result<Vec<PriceBarRow>> {
        let Some(stock_no) = twse_stock_no(symbol) else {
            return Ok(Vec::new());
        };
        let today = Utc::now().date_naive();
        let from = today - ChronoDuration::days(lookback_days);
        let mut all = Vec::new();
        let months = month_starts_between(from, today);
        let month_count = months.len();
        for (idx, month) in months.into_iter().enumerate() {
            let rows = self.fetch_month(symbol, &stock_no, month).await?;
            all.extend(
                rows.into_iter()
                    .filter(|r| r.ts.date_naive() >= from && r.ts.date_naive() <= today),
            );
            if idx + 1 < month_count {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
        all.sort_by_key(|r| r.ts);
        Ok(all)
    }

    pub async fn poll_symbols(
        &self,
        symbols: &[String],
        existing_min_ts_by_symbol: &HashMap<String, Option<chrono::DateTime<Utc>>>,
    ) -> Result<Vec<PriceBarRow>> {
        let mut all = Vec::new();
        let five_years = 365 * 5;
        for sym in symbols {
            let cold = !matches!(existing_min_ts_by_symbol.get(sym), Some(Some(_)));
            let lookback_days = if cold { five_years } else { 30 };
            match self.fetch_one(sym, lookback_days).await {
                Ok(rows) => {
                    tracing::info!(
                        symbol = %sym,
                        bars = rows.len(),
                        cold,
                        lookback_days,
                        "twse bars fetched"
                    );
                    all.extend(rows);
                }
                Err(e) => {
                    warn!(symbol = %sym, error = %e, "twse fetch failed; continuing");
                }
            }
        }
        Ok(all)
    }

    async fn fetch_month(
        &self,
        symbol: &str,
        stock_no: &str,
        month: NaiveDate,
    ) -> Result<Vec<PriceBarRow>> {
        let date = month.format("%Y%m01");
        let url = format!(
            "{}/exchangeReport/STOCK_DAY?response=json&date={date}&stockNo={stock_no}",
            self.base_url
        );
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .with_context(|| format!("twse stock day fetch {symbol} {date}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "twse {symbol} {}: {}",
                status.as_u16(),
                &body[..body.len().min(256)]
            );
        }
        let parsed: TwseStockDayResponse = resp
            .json()
            .await
            .with_context(|| format!("twse stock day decode {symbol} {date}"))?;
        Ok(to_rows(symbol, &parsed))
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct TwseStockDayResponse {
    pub stat: String,
    #[serde(default)]
    pub data: Vec<Vec<String>>,
}

#[must_use]
pub fn twse_stock_no(symbol: &str) -> Option<String> {
    let symbol = symbol.trim().to_ascii_uppercase();
    let (code, suffix) = symbol.split_once('.')?;
    if suffix == "TW" && code.len() == 4 && code.chars().all(|c| c.is_ascii_digit()) {
        Some(code.to_string())
    } else {
        None
    }
}

#[must_use]
pub fn to_rows(symbol: &str, response: &TwseStockDayResponse) -> Vec<PriceBarRow> {
    if response.stat != "OK" {
        return Vec::new();
    }
    response
        .data
        .iter()
        .filter_map(|row| {
            if row.len() < 7 {
                return None;
            }
            let nd = parse_roc_date(&row[0])?;
            Some(PriceBarRow {
                symbol: symbol.to_string(),
                ts: Utc.from_utc_datetime(&nd.and_hms_opt(4, 0, 0)?),
                open: parse_number(&row[3])?,
                high: parse_number(&row[4])?,
                low: parse_number(&row[5])?,
                close: parse_number(&row[6])?,
                volume: parse_number(&row[1])?,
            })
        })
        .collect()
}

fn parse_roc_date(value: &str) -> Option<NaiveDate> {
    let mut parts = value.split('/');
    let year = parts.next()?.parse::<i32>().ok()? + 1911;
    let month = parts.next()?.parse::<u32>().ok()?;
    let day = parts.next()?.parse::<u32>().ok()?;
    NaiveDate::from_ymd_opt(year, month, day)
}

fn parse_number(value: &str) -> Option<f64> {
    value.replace(',', "").parse::<f64>().ok()
}

fn month_starts_between(from: NaiveDate, to: NaiveDate) -> Vec<NaiveDate> {
    let mut year = from.year();
    let mut month = from.month();
    let mut out = Vec::new();
    loop {
        let current = NaiveDate::from_ymd_opt(year, month, 1).expect("valid month");
        out.push(current);
        if year == to.year() && month == to.month() {
            break;
        }
        if month == 12 {
            year += 1;
            month = 1;
        } else {
            month += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_response() -> TwseStockDayResponse {
        serde_json::from_value(serde_json::json!({
            "stat": "OK",
            "data": [[
                "115/05/04",
                "4,019,865",
                "11,537,012,550",
                "2,870.00",
                "2,870.00",
                "2,870.00",
                "2,870.00",
                "+260.00",
                "15,261",
                ""
            ]]
        }))
        .unwrap()
    }

    #[test]
    fn supports_numeric_taiwan_exchange_suffix() {
        assert_eq!(twse_stock_no("2454.TW"), Some("2454".to_string()));
        assert_eq!(twse_stock_no(" 0050.tw "), Some("0050".to_string()));
        assert_eq!(twse_stock_no("2454.T"), None);
        assert_eq!(twse_stock_no("NVDA"), None);
    }

    #[test]
    fn to_rows_decodes_roc_dates_and_numeric_fields() {
        let rows = to_rows("2454.TW", &sample_response());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].symbol, "2454.TW");
        assert_eq!(
            rows[0].ts.date_naive(),
            NaiveDate::from_ymd_opt(2026, 5, 4).unwrap()
        );
        assert_eq!(rows[0].open, 2870.0);
        assert_eq!(rows[0].close, 2870.0);
        assert_eq!(rows[0].volume, 4_019_865.0);
    }

    #[test]
    fn non_ok_response_yields_no_rows() {
        let rows = to_rows(
            "2454.TW",
            &TwseStockDayResponse {
                stat: "NO_DATA".to_string(),
                data: vec![],
            },
        );
        assert!(rows.is_empty());
    }

    #[test]
    fn month_range_includes_start_and_end_months() {
        let months = month_starts_between(
            NaiveDate::from_ymd_opt(2026, 5, 15).unwrap(),
            NaiveDate::from_ymd_opt(2026, 7, 2).unwrap(),
        );
        assert_eq!(
            months,
            vec![
                NaiveDate::from_ymd_opt(2026, 5, 1).unwrap(),
                NaiveDate::from_ymd_opt(2026, 6, 1).unwrap(),
                NaiveDate::from_ymd_opt(2026, 7, 1).unwrap(),
            ]
        );
    }
}
