//! Massive (Polygon-compat) news adapter (#19). `/v2/reference/news?ticker=`
//! returns articles plus `insights[]` per ticker, each insight carrying a
//! pre-scored sentiment from Massive's own classifier.

use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Deserialize;

use super::fmp_news::NewsArticle;

#[derive(Debug, Clone, Deserialize)]
pub struct MassiveInsight {
    pub ticker: String,
    /// "positive" | "neutral" | "negative" per Massive's docs.
    #[serde(default)]
    pub sentiment: Option<String>,
    #[serde(default)]
    pub sentiment_reasoning: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MassiveArticle {
    pub id: String,
    pub published_utc: DateTime<Utc>,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub article_url: Option<String>,
    #[serde(default)]
    pub publisher: Option<MassivePublisher>,
    #[serde(default)]
    pub insights: Vec<MassiveInsight>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MassivePublisher {
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MassiveNewsResponse {
    #[serde(default)]
    pub results: Vec<MassiveArticle>,
}

/// One scored article ready for `news_article` upsert. Sentiment fields are
/// populated from the upstream insight when present; left None if the vendor
/// didn't score it (the news service will pass it through the LLM scorer).
#[derive(Debug, Clone, PartialEq)]
pub struct ScoredArticle {
    pub article: NewsArticle,
    pub upstream_sentiment: Option<String>,
    pub upstream_rationale: Option<String>,
}

/// Pure: flatten Massive's response into one ScoredArticle per (article, ticker)
/// insight. An article that mentions 3 tickers becomes 3 rows.
#[must_use]
pub fn normalize(resp: &MassiveNewsResponse, universe: &[&str]) -> Vec<ScoredArticle> {
    let mut out = Vec::new();
    let universe: std::collections::HashSet<&str> = universe.iter().copied().collect();
    for art in &resp.results {
        for insight in &art.insights {
            if !universe.contains(insight.ticker.as_str()) {
                continue;
            }
            out.push(ScoredArticle {
                article: NewsArticle {
                    symbol: insight.ticker.clone(),
                    title: art.title.clone(),
                    body: art.description.clone(),
                    url: art.article_url.clone(),
                    publisher: art.publisher.as_ref().and_then(|p| p.name.clone()),
                    published_at: art.published_utc,
                    source: "massive",
                },
                upstream_sentiment: insight.sentiment.clone(),
                upstream_rationale: insight.sentiment_reasoning.clone(),
            });
        }
    }
    out
}

pub struct MassiveNewsAdapter {
    api_key: String,
    base_url: String,
    client: Client,
}

impl MassiveNewsAdapter {
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

    /// Fetch the most-recent `limit` Massive news rows tagged for `symbol`.
    /// Returns rows flattened to per-ticker articles, filtered to `universe`.
    pub async fn fetch_one(
        &self,
        symbol: &str,
        limit: u32,
        universe: &[&str],
    ) -> Result<Vec<ScoredArticle>> {
        if self.api_key.is_empty() {
            return Ok(Vec::new());
        }
        let url = format!(
            "{}/v2/reference/news?ticker={symbol}&order=desc&limit={limit}&apiKey={key}",
            self.base_url,
            key = self.api_key,
        );
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .with_context(|| format!("massive news fetch {symbol}"))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "massive news {symbol} {}: {}",
                status.as_u16(),
                &body[..body.len().min(256)]
            );
        }
        let parsed: MassiveNewsResponse = resp
            .json()
            .await
            .with_context(|| format!("massive news decode {symbol}"))?;
        Ok(normalize(&parsed, universe))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_resp() -> MassiveNewsResponse {
        // Massive's documented shape; the article describes a single multi-
        // ticker piece with insights for MU + NVDA (so dedup logic is exercised).
        serde_json::from_value(serde_json::json!({
            "results": [{
                "id":"abc123",
                "published_utc":"2026-05-31T05:05:00Z",
                "title":"AI capex tailwind drives memory + GPU names",
                "description":"Hyperscaler procurement signals continued strength through 2027.",
                "article_url":"https://example.com/article1",
                "publisher":{"name":"ExampleWire"},
                "insights":[
                    {"ticker":"MU","sentiment":"positive","sentiment_reasoning":"capex tailwind"},
                    {"ticker":"NVDA","sentiment":"positive","sentiment_reasoning":"primary beneficiary"},
                    {"ticker":"NOT_IN_UNIVERSE","sentiment":"neutral"}
                ]
            }]
        })).unwrap()
    }

    #[test]
    fn normalize_yields_one_row_per_insight_in_universe() {
        let universe = &["MU", "NVDA", "AMD"];
        let rows = normalize(&sample_resp(), universe);
        assert_eq!(rows.len(), 2, "non-universe insights filtered out");
        assert!(rows.iter().any(|r| r.article.symbol == "MU"));
        assert!(rows.iter().any(|r| r.article.symbol == "NVDA"));
    }

    #[test]
    fn normalize_carries_upstream_sentiment_through() {
        let rows = normalize(&sample_resp(), &["MU"]);
        assert_eq!(rows[0].upstream_sentiment.as_deref(), Some("positive"));
        assert_eq!(
            rows[0].upstream_rationale.as_deref(),
            Some("capex tailwind")
        );
        assert_eq!(rows[0].article.source, "massive");
    }

    #[test]
    fn normalize_handles_missing_insights_array() {
        let mut resp = sample_resp();
        resp.results[0].insights.clear();
        let rows = normalize(&resp, &["MU"]);
        assert!(rows.is_empty());
    }
}
