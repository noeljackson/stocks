//! FMP per-ticker news adapter (#19). `/stable/news/stock?symbols=`
//! returns articles per symbol. **No sentiment field** — these articles get
//! scored by our universal `sentiment::score_one` classifier post-ingest.

use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use reqwest::Client;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct FmpNewsRow {
    pub symbol: String,
    #[serde(rename = "publishedDate")]
    pub published_date: String, // "2026-05-31 05:05:00"
    #[serde(default)]
    pub publisher: Option<String>,
    pub title: String,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewsArticle {
    pub symbol: String,
    pub title: String,
    pub body: Option<String>,
    pub url: Option<String>,
    pub publisher: Option<String>,
    pub published_at: DateTime<Utc>,
    pub source: &'static str,
}

/// Pure: normalize the FMP shape into our `news_article`-ready type. Drops
/// rows whose `publishedDate` doesn't parse — we'd rather skip than fail.
#[must_use]
pub fn normalize(rows: &[FmpNewsRow]) -> Vec<NewsArticle> {
    rows.iter()
        .filter_map(|r| {
            let naive =
                NaiveDateTime::parse_from_str(&r.published_date, "%Y-%m-%d %H:%M:%S").ok()?;
            let published_at = Utc.from_utc_datetime(&naive);
            Some(NewsArticle {
                symbol: r.symbol.clone(),
                title: r.title.clone(),
                body: r.text.clone(),
                url: r.url.clone(),
                publisher: r.publisher.clone(),
                published_at,
                source: "fmp",
            })
        })
        .collect()
}

pub struct FmpNewsAdapter {
    api_key: String,
    base_url: String,
    client: Client,
}

impl FmpNewsAdapter {
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

    /// Fetch the most-recent `limit` articles for one symbol.
    pub async fn fetch_one(&self, symbol: &str, limit: u32) -> Result<Vec<NewsArticle>> {
        if self.api_key.is_empty() {
            return Ok(Vec::new());
        }
        let url = format!(
            "{}/stable/news/stock?symbols={symbol}&limit={limit}&apikey={key}",
            self.base_url,
            key = self.api_key,
        );
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .with_context(|| format!("fmp news fetch {symbol}"))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("fmp news {symbol} {}: {}", status.as_u16(), &body[..body.len().min(256)]);
        }
        let rows: Vec<FmpNewsRow> = resp
            .json()
            .await
            .with_context(|| format!("fmp news decode {symbol}"))?;
        Ok(normalize(&rows))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_response() -> Vec<FmpNewsRow> {
        // Verbatim from a live probe on 2026-05-31.
        serde_json::from_value(serde_json::json!([
            {
                "symbol":"MU",
                "publishedDate":"2026-05-31 05:05:00",
                "publisher":"Fool - Investing News",
                "title":"Micron Just Entered the Trillion-Dollar Club. Is It Too Late to Buy the Stock?",
                "image":"https://images.financialmodelingprep.com/news/x.jpg",
                "site":"fool.com",
                "text":"Micron joins players including Nvidia and Microsoft in this exclusive group.",
                "url":"https://www.fool.com/investing/2026/05/31/micron-just-entered-the-trillion-dollar-club-is-it/"
            }
        ])).unwrap()
    }

    #[test]
    fn normalize_decodes_real_fmp_row() {
        let rows = normalize(&sample_response());
        assert_eq!(rows.len(), 1);
        let a = &rows[0];
        assert_eq!(a.symbol, "MU");
        assert_eq!(a.source, "fmp");
        assert!(a.title.contains("Trillion-Dollar"));
        assert_eq!(a.body.as_deref().unwrap(), "Micron joins players including Nvidia and Microsoft in this exclusive group.");
        assert_eq!(a.publisher.as_deref().unwrap(), "Fool - Investing News");
    }

    #[test]
    fn normalize_parses_published_date_as_utc() {
        let rows = normalize(&sample_response());
        assert_eq!(rows[0].published_at.to_rfc3339(), "2026-05-31T05:05:00+00:00");
    }

    #[test]
    fn normalize_drops_bad_dates_keeps_good() {
        let mut rows = sample_response();
        rows[0].published_date = "yesterday".into();
        let out = normalize(&rows);
        assert!(out.is_empty(), "bad date drops the row");
    }
}
