//! FMP analyst-opinion adapter (#116).
//!
//! Analyst estimates are financial forecasts. This adapter captures the
//! separate sell-side opinion surface: price target consensus, buy/hold/sell
//! mix, and recent price-target events.

use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDate, Utc};
use reqwest::Client;
use serde::Deserialize;

use super::rate_limit;

#[derive(Debug, Clone)]
pub struct FmpOpinionRaw {
    pub consensus: serde_json::Value,
    pub recommendations: serde_json::Value,
    pub price_target_events: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct PriceTargetConsensusRow {
    pub symbol: String,
    #[serde(default, alias = "targetHigh")]
    pub target_high: Option<f64>,
    #[serde(default, alias = "targetLow")]
    pub target_low: Option<f64>,
    #[serde(default, alias = "targetConsensus")]
    pub target_consensus: Option<f64>,
    #[serde(default, alias = "targetMedian")]
    pub target_median: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct RecommendationRow {
    pub symbol: String,
    #[serde(default)]
    pub date: Option<String>,
    #[serde(default, alias = "analystRatingsStrongBuy")]
    pub strong_buy: Option<i32>,
    #[serde(default, alias = "analystRatingsBuy")]
    pub buy: Option<i32>,
    #[serde(default, alias = "analystRatingsHold")]
    pub hold: Option<i32>,
    #[serde(default, alias = "analystRatingsSell")]
    pub sell: Option<i32>,
    #[serde(default, alias = "analystRatingsStrongSell")]
    pub strong_sell: Option<i32>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct PriceTargetEventRow {
    pub symbol: String,
    #[serde(rename = "publishedDate")]
    pub published_date: String,
    #[serde(default, rename = "newsURL")]
    pub news_url: Option<String>,
    #[serde(rename = "newsTitle")]
    pub news_title: String,
    #[serde(default, rename = "analystName")]
    pub analyst_name: Option<String>,
    #[serde(default, rename = "priceTarget")]
    pub price_target: Option<f64>,
    #[serde(default, rename = "adjPriceTarget")]
    pub adj_price_target: Option<f64>,
    #[serde(default, rename = "priceWhenPosted")]
    pub price_when_posted: Option<f64>,
    #[serde(default, rename = "newsPublisher")]
    pub news_publisher: Option<String>,
    #[serde(default, rename = "newsBaseURL")]
    pub news_base_url: Option<String>,
    #[serde(default, rename = "analystCompany")]
    pub analyst_company: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NormalizedRecommendation {
    pub symbol: String,
    pub as_of_date: Option<NaiveDate>,
    pub strong_buy: Option<i32>,
    pub buy: Option<i32>,
    pub hold: Option<i32>,
    pub sell: Option<i32>,
    pub strong_sell: Option<i32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NormalizedPriceTargetEvent {
    pub symbol: String,
    pub published_at: DateTime<Utc>,
    pub news_url: Option<String>,
    pub news_title: String,
    pub analyst_name: Option<String>,
    pub analyst_company: Option<String>,
    pub price_target: Option<f64>,
    pub adj_price_target: Option<f64>,
    pub price_when_posted: Option<f64>,
    pub news_publisher: Option<String>,
    pub news_base_url: Option<String>,
}

pub fn decode_consensus(json: &serde_json::Value) -> Result<Vec<PriceTargetConsensusRow>> {
    serde_json::from_value::<Vec<PriceTargetConsensusRow>>(json.clone())
        .context("decode fmp price-target-consensus response")
}

pub fn decode_recommendations(json: &serde_json::Value) -> Result<Vec<RecommendationRow>> {
    serde_json::from_value::<Vec<RecommendationRow>>(json.clone())
        .context("decode fmp grades-historical response")
}

pub fn decode_price_target_events(json: &serde_json::Value) -> Result<Vec<PriceTargetEventRow>> {
    serde_json::from_value::<Vec<PriceTargetEventRow>>(json.clone())
        .context("decode fmp price-target-news response")
}

#[must_use]
pub fn normalize_recommendations(rows: &[RecommendationRow]) -> Vec<NormalizedRecommendation> {
    rows.iter()
        .map(|r| NormalizedRecommendation {
            symbol: r.symbol.clone(),
            as_of_date: r
                .date
                .as_deref()
                .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()),
            strong_buy: r.strong_buy,
            buy: r.buy,
            hold: r.hold,
            sell: r.sell,
            strong_sell: r.strong_sell,
        })
        .collect()
}

#[must_use]
pub fn normalize_price_target_events(
    rows: &[PriceTargetEventRow],
) -> Vec<NormalizedPriceTargetEvent> {
    rows.iter()
        .filter_map(|r| {
            let published_at = DateTime::parse_from_rfc3339(&r.published_date)
                .ok()?
                .with_timezone(&Utc);
            Some(NormalizedPriceTargetEvent {
                symbol: r.symbol.clone(),
                published_at,
                news_url: r.news_url.clone(),
                news_title: r.news_title.clone(),
                analyst_name: r.analyst_name.clone(),
                analyst_company: r.analyst_company.clone(),
                price_target: r.price_target,
                adj_price_target: r.adj_price_target,
                price_when_posted: r.price_when_posted,
                news_publisher: r.news_publisher.clone(),
                news_base_url: r.news_base_url.clone(),
            })
        })
        .collect()
}

pub struct FmpOpinionAdapter {
    api_key: String,
    base_url: String,
    client: Client,
}

impl FmpOpinionAdapter {
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
            .with_context(|| format!("fmp opinion fetch {symbol} {path}"))?;
        let status = resp.status();
        let retry_after = rate_limit::retry_after(resp.headers());
        rate_limit::fmp().observe_status(status, retry_after).await;
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "fmp opinion {symbol} {}: {}",
                status.as_u16(),
                &body[..body.len().min(256)]
            );
        }
        resp.json()
            .await
            .with_context(|| format!("fmp opinion decode {symbol} {path}"))
    }

    pub async fn fetch_one(&self, symbol: &str) -> Result<FmpOpinionRaw> {
        let consensus = self
            .fetch_json(
                symbol,
                &format!("/stable/price-target-consensus?symbol={symbol}"),
            )
            .await?;
        let recommendations = self
            .fetch_json(
                symbol,
                &format!("/stable/grades-historical?symbol={symbol}&limit=1"),
            )
            .await?;
        let price_target_events = self
            .fetch_json(
                symbol,
                &format!("/stable/price-target-news?symbol={symbol}&limit=10"),
            )
            .await?;
        Ok(FmpOpinionRaw {
            consensus,
            recommendations,
            price_target_events,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_price_target_consensus_shape() {
        let rows = decode_consensus(&serde_json::json!([{
            "symbol": "NVDA",
            "targetHigh": 500,
            "targetLow": 218,
            "targetConsensus": 316.79,
            "targetMedian": 300
        }]))
        .unwrap();
        assert_eq!(rows[0].target_consensus, Some(316.79));
        assert_eq!(rows[0].target_median, Some(300.0));
    }

    #[test]
    fn normalizes_recommendation_mix_date() {
        let rows = decode_recommendations(&serde_json::json!([{
            "symbol": "NVDA",
            "date": "2026-06-01",
            "analystRatingsStrongBuy": 10,
            "analystRatingsBuy": 48,
            "analystRatingsHold": 2,
            "analystRatingsSell": 1,
            "analystRatingsStrongSell": 0
        }]))
        .unwrap();
        let normalized = normalize_recommendations(&rows);
        assert_eq!(
            normalized[0].as_of_date.unwrap(),
            NaiveDate::from_ymd_opt(2026, 6, 1).unwrap()
        );
        assert_eq!(normalized[0].buy, Some(48));
    }

    #[test]
    fn normalizes_price_target_event_timestamp() {
        let rows = decode_price_target_events(&serde_json::json!([{
            "symbol": "NVDA",
            "publishedDate": "2026-05-27T14:19:54.000Z",
            "newsURL": "https://thefly.com/ajax/news_get.php?id=4361526",
            "newsTitle": "Nvidia price target raised to $425 from $360 at Tigress Financial",
            "analystName": "",
            "priceTarget": 425,
            "adjPriceTarget": 425,
            "priceWhenPosted": 210.24,
            "newsPublisher": "TheFly",
            "newsBaseURL": "thefly.com",
            "analystCompany": "Tigress Financial"
        }]))
        .unwrap();
        let normalized = normalize_price_target_events(&rows);
        assert_eq!(
            normalized[0].published_at.to_rfc3339(),
            "2026-05-27T14:19:54+00:00"
        );
        assert_eq!(normalized[0].price_target, Some(425.0));
    }
}
