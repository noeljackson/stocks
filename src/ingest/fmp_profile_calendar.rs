//! FMP company profile and earnings calendar adapter (#260).
//!
//! `/stable/profile?symbol=` provides issuer metadata used for classification
//! and operator context. `/stable/earnings?symbol=&limit=` provides upcoming
//! and recent earnings dates with EPS/revenue actuals and estimates.

use std::time::Duration;

use anyhow::{Context, Result};
use chrono::NaiveDate;
use reqwest::Client;
use serde::Deserialize;

use super::rate_limit;

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct CompanyProfileRow {
    pub symbol: String,
    #[serde(default, rename = "companyName")]
    pub company_name: Option<String>,
    #[serde(default)]
    pub currency: Option<String>,
    #[serde(default, rename = "marketCap")]
    pub market_cap: Option<f64>,
    #[serde(default)]
    pub beta: Option<f64>,
    #[serde(default)]
    pub exchange: Option<String>,
    #[serde(default, rename = "exchangeFullName")]
    pub exchange_full_name: Option<String>,
    #[serde(default)]
    pub industry: Option<String>,
    #[serde(default)]
    pub sector: Option<String>,
    #[serde(default)]
    pub country: Option<String>,
    #[serde(default)]
    pub website: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub ceo: Option<String>,
    #[serde(default, rename = "fullTimeEmployees")]
    pub full_time_employees: Option<String>,
    #[serde(default, rename = "ipoDate")]
    pub ipo_date: Option<String>,
    #[serde(default, rename = "isEtf")]
    pub is_etf: Option<bool>,
    #[serde(default, rename = "isAdr")]
    pub is_adr: Option<bool>,
    #[serde(default, rename = "isFund")]
    pub is_fund: Option<bool>,
    #[serde(default, rename = "isActivelyTrading")]
    pub is_actively_trading: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct EarningsCalendarRow {
    pub symbol: String,
    pub date: String,
    #[serde(default, rename = "epsActual")]
    pub eps_actual: Option<f64>,
    #[serde(default, rename = "epsEstimated")]
    pub eps_estimated: Option<f64>,
    #[serde(default, rename = "revenueActual")]
    pub revenue_actual: Option<f64>,
    #[serde(default, rename = "revenueEstimated")]
    pub revenue_estimated: Option<f64>,
    #[serde(default, rename = "lastUpdated")]
    pub last_updated: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NormalizedCompanyProfile {
    pub symbol: String,
    pub company_name: Option<String>,
    pub currency: Option<String>,
    pub market_cap: Option<f64>,
    pub beta: Option<f64>,
    pub exchange: Option<String>,
    pub exchange_full_name: Option<String>,
    pub industry: Option<String>,
    pub sector: Option<String>,
    pub country: Option<String>,
    pub website: Option<String>,
    pub description: Option<String>,
    pub ceo: Option<String>,
    pub full_time_employees: Option<i64>,
    pub ipo_date: Option<NaiveDate>,
    pub is_etf: Option<bool>,
    pub is_adr: Option<bool>,
    pub is_fund: Option<bool>,
    pub is_actively_trading: Option<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NormalizedEarningsEvent {
    pub symbol: String,
    pub report_date: NaiveDate,
    pub eps_actual: Option<f64>,
    pub eps_estimated: Option<f64>,
    pub revenue_actual: Option<f64>,
    pub revenue_estimated: Option<f64>,
    pub last_updated: Option<NaiveDate>,
}

pub fn decode_profile(json: &serde_json::Value) -> Result<Vec<CompanyProfileRow>> {
    serde_json::from_value::<Vec<CompanyProfileRow>>(json.clone())
        .context("decode fmp profile response")
}

pub fn decode_earnings(json: &serde_json::Value) -> Result<Vec<EarningsCalendarRow>> {
    serde_json::from_value::<Vec<EarningsCalendarRow>>(json.clone())
        .context("decode fmp earnings response")
}

#[must_use]
pub fn normalize_profile(row: &CompanyProfileRow) -> NormalizedCompanyProfile {
    NormalizedCompanyProfile {
        symbol: row.symbol.clone(),
        company_name: row.company_name.clone(),
        currency: row.currency.clone(),
        market_cap: row.market_cap,
        beta: row.beta,
        exchange: row.exchange.clone(),
        exchange_full_name: row.exchange_full_name.clone(),
        industry: row.industry.clone(),
        sector: row.sector.clone(),
        country: row.country.clone(),
        website: row.website.clone(),
        description: row.description.clone(),
        ceo: row.ceo.clone(),
        full_time_employees: row.full_time_employees.as_deref().and_then(parse_i64),
        ipo_date: row.ipo_date.as_deref().and_then(parse_date),
        is_etf: row.is_etf,
        is_adr: row.is_adr,
        is_fund: row.is_fund,
        is_actively_trading: row.is_actively_trading,
    }
}

#[must_use]
pub fn normalize_earnings(rows: &[EarningsCalendarRow]) -> Vec<NormalizedEarningsEvent> {
    rows.iter()
        .filter_map(|row| {
            let report_date = parse_date(&row.date)?;
            Some(NormalizedEarningsEvent {
                symbol: row.symbol.clone(),
                report_date,
                eps_actual: row.eps_actual,
                eps_estimated: row.eps_estimated,
                revenue_actual: row.revenue_actual,
                revenue_estimated: row.revenue_estimated,
                last_updated: row.last_updated.as_deref().and_then(parse_date),
            })
        })
        .collect()
}

fn parse_date(raw: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(raw, "%Y-%m-%d").ok()
}

fn parse_i64(raw: &str) -> Option<i64> {
    raw.trim().replace(',', "").parse::<i64>().ok()
}

pub struct FmpProfileCalendarAdapter {
    api_key: String,
    base_url: String,
    client: Client,
}

impl FmpProfileCalendarAdapter {
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

    async fn fetch_json(&self, symbol: &str, path: &str) -> Result<serde_json::Value> {
        if self.api_key.is_empty() {
            return Ok(serde_json::Value::Array(vec![]));
        }
        let sep = if path.contains('?') { '&' } else { '?' };
        let url = format!(
            "{}{path}{sep}apikey={key}",
            self.base_url,
            sep = sep,
            key = self.api_key,
        );
        rate_limit::fmp().wait().await;
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .with_context(|| format!("fmp profile/calendar fetch {symbol} {path}"))?;
        let status = resp.status();
        let retry_after = rate_limit::retry_after(resp.headers());
        rate_limit::fmp().observe_status(status, retry_after).await;
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "fmp profile/calendar {symbol} {}: {}",
                status.as_u16(),
                &body[..body.len().min(256)]
            );
        }
        resp.json()
            .await
            .with_context(|| format!("fmp profile/calendar decode {symbol} {path}"))
    }

    pub async fn fetch_profile(&self, symbol: &str) -> Result<serde_json::Value> {
        self.fetch_json(symbol, &format!("/stable/profile?symbol={symbol}"))
            .await
    }

    pub async fn fetch_earnings(
        &self,
        symbol: &str,
        earnings_limit: usize,
    ) -> Result<serde_json::Value> {
        self.fetch_json(
            symbol,
            &format!(
                "/stable/earnings?symbol={symbol}&limit={}",
                earnings_limit.max(1)
            ),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_and_normalizes_company_profile_shape() {
        let rows = decode_profile(&serde_json::json!([{
            "symbol": "2454.TW",
            "marketCap": 7254301383675_f64,
            "companyName": "MediaTek Inc.",
            "currency": "TWD",
            "exchangeFullName": "Taiwan Stock Exchange",
            "exchange": "TAI",
            "industry": "Semiconductors",
            "sector": "Technology",
            "country": "TW",
            "fullTimeEmployees": "6999",
            "ipoDate": "2001-07-23",
            "isEtf": false,
            "isActivelyTrading": true
        }]))
        .unwrap();

        let normalized = normalize_profile(&rows[0]);
        assert_eq!(normalized.symbol, "2454.TW");
        assert_eq!(normalized.company_name.as_deref(), Some("MediaTek Inc."));
        assert_eq!(normalized.full_time_employees, Some(6999));
        assert_eq!(
            normalized.ipo_date,
            Some(NaiveDate::from_ymd_opt(2001, 7, 23).unwrap())
        );
        assert_eq!(normalized.sector.as_deref(), Some("Technology"));
    }

    #[test]
    fn decodes_and_normalizes_earnings_shape() {
        let rows = decode_earnings(&serde_json::json!([{
            "symbol": "NVDA",
            "date": "2026-08-26",
            "epsActual": null,
            "epsEstimated": 2.07,
            "revenueActual": null,
            "revenueEstimated": 91572710000_f64,
            "lastUpdated": "2026-06-02"
        }]))
        .unwrap();

        let normalized = normalize_earnings(&rows);
        assert_eq!(normalized.len(), 1);
        assert_eq!(
            normalized[0].report_date,
            NaiveDate::from_ymd_opt(2026, 8, 26).unwrap()
        );
        assert_eq!(normalized[0].eps_estimated, Some(2.07));
        assert_eq!(
            normalized[0].last_updated,
            Some(NaiveDate::from_ymd_opt(2026, 6, 2).unwrap())
        );
    }

    #[test]
    fn normalize_earnings_drops_unparseable_dates() {
        let rows = vec![EarningsCalendarRow {
            symbol: "NVDA".into(),
            date: "soon".into(),
            eps_actual: None,
            eps_estimated: Some(2.07),
            revenue_actual: None,
            revenue_estimated: None,
            last_updated: None,
        }];

        assert!(normalize_earnings(&rows).is_empty());
    }
}
