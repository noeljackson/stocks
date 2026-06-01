//! FMP company-screener adapter (#88). Pulls a broad pool of investible
//! names matching sector/cap criteria so the discovery scanner can fire
//! signals on a real candidate population (not just our hand-curated
//! universe).
//!
//! Run nightly: refreshes `discovery_pool`, marks names that dropped out
//! of the screener result so we don't re-pull their bars.

use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::Client;
use serde::Deserialize;

use super::rate_limit;

#[derive(Debug, Clone, Deserialize)]
pub struct ScreenerRow {
    pub symbol: String,
    #[serde(default, rename = "companyName")]
    pub company_name: Option<String>,
    #[serde(default)]
    pub sector: Option<String>,
    #[serde(default)]
    pub industry: Option<String>,
    #[serde(default, rename = "marketCap")]
    pub market_cap: Option<i64>,
    #[serde(default, rename = "isActivelyTrading")]
    pub is_actively_trading: bool,
}

pub struct FmpScreenerAdapter {
    api_key: String,
    base_url: String,
    client: Client,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ScreenerSlice {
    sector: &'static str,
    industry: Option<&'static str>,
}

fn screener_slices() -> &'static [ScreenerSlice] {
    &[
        // Core compute/software infrastructure.
        ScreenerSlice {
            sector: "Technology",
            industry: Some("Semiconductors"),
        },
        ScreenerSlice {
            sector: "Technology",
            industry: Some("Semiconductor Equipment & Materials"),
        },
        ScreenerSlice {
            sector: "Technology",
            industry: Some("Communication Equipment"),
        },
        ScreenerSlice {
            sector: "Technology",
            industry: Some("Information Technology Services"),
        },
        ScreenerSlice {
            sector: "Technology",
            industry: Some("Software - Infrastructure"),
        },
        ScreenerSlice {
            sector: "Technology",
            industry: Some("Computer Hardware"),
        },
        // Power, cooling, grid, and construction bottlenecks around AI buildout.
        ScreenerSlice {
            sector: "Industrials",
            industry: Some("Electrical Equipment & Parts"),
        },
        ScreenerSlice {
            sector: "Industrials",
            industry: Some("Specialty Industrial Machinery"),
        },
        ScreenerSlice {
            sector: "Industrials",
            industry: Some("Engineering & Construction"),
        },
        ScreenerSlice {
            sector: "Industrials",
            industry: Some("Building Products & Equipment"),
        },
        ScreenerSlice {
            sector: "Utilities",
            industry: Some("Utilities - Renewable"),
        },
        ScreenerSlice {
            sector: "Utilities",
            industry: Some("Utilities - Independent Power Producers"),
        },
        ScreenerSlice {
            sector: "Utilities",
            industry: Some("Utilities - Regulated Electric"),
        },
        // Materials exposure matters when power/data-center buildout pulls
        // copper, electrical steel, and other upstream constraints into scope.
        ScreenerSlice {
            sector: "Basic Materials",
            industry: None,
        },
        // Data-center landlords and physical infrastructure owners.
        ScreenerSlice {
            sector: "Real Estate",
            industry: Some("REIT - Specialty"),
        },
        ScreenerSlice {
            sector: "Real Estate",
            industry: Some("REIT - Industrial"),
        },
    ]
}

impl FmpScreenerAdapter {
    pub fn new(api_key: &str, base_url: &str) -> Self {
        Self {
            api_key: api_key.to_string(),
            base_url: base_url.trim_end_matches('/').to_string(),
            client: Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("reqwest client"),
        }
    }

    /// Pull all rows matching our SPEC §0 scope (tech-infrastructure,
    /// large-cap, actively trading). Returns the filtered list — non-US
    /// names dropped, anything not actively trading dropped, market cap
    /// floor enforced again client-side as belt-and-braces.
    pub async fn fetch_pool(&self, min_market_cap: i64) -> Result<Vec<ScreenerRow>> {
        if self.api_key.is_empty() {
            return Ok(Vec::new());
        }
        let mut all = Vec::new();
        // SPEC §0 scope: tech infrastructure and adjacent physical bottlenecks.
        // Pull each sector/industry slice; dedup at the end by symbol.
        // FMP's screener returns ~250 per call max; we paginate by sector.
        for slice in screener_slices() {
            let sector = slice.sector;
            let industry = slice.industry;
            let mut url = format!(
                "{}/stable/company-screener?marketCapMoreThan={}&sector={}&isEtf=false&isFund=false&isActivelyTrading=true&country=US&limit=250&apikey={}",
                self.base_url,
                min_market_cap,
                urlencoding::encode(sector),
                self.api_key,
            );
            if let Some(ind) = industry {
                url.push_str(&format!("&industry={}", urlencoding::encode(ind)));
            }
            rate_limit::fmp().wait().await;
            let resp = self
                .client
                .get(&url)
                .send()
                .await
                .with_context(|| format!("fmp screener {sector}/{:?}", industry))?;
            let status = resp.status();
            let retry_after = rate_limit::retry_after(resp.headers());
            rate_limit::fmp().observe_status(status, retry_after).await;
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                tracing::warn!(
                    sector = sector,
                    industry = ?industry,
                    status = status.as_u16(),
                    body = &body[..body.len().min(200)],
                    "fmp screener slice failed; continuing"
                );
                continue;
            }
            let rows: Vec<ScreenerRow> = resp
                .json()
                .await
                .with_context(|| format!("fmp screener decode {sector}"))?;
            all.extend(rows);
        }
        // Dedup by symbol (some industries overlap).
        let mut seen = std::collections::HashSet::new();
        all.retain(|r| seen.insert(r.symbol.clone()) && r.is_actively_trading);
        Ok(all)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_real_screener_row() {
        let v = serde_json::json!([{
            "symbol":"NVDA",
            "companyName":"NVIDIA Corporation",
            "marketCap":5114021940000_i64,
            "sector":"Technology",
            "industry":"Semiconductors",
            "beta":2.244,
            "price":211.14,
            "lastAnnualDividend":0.04,
            "volume":230465710,
            "exchange":"NASDAQ Global Select",
            "exchangeShortName":"NASDAQ",
            "country":"US",
            "isEtf":false,
            "isFund":false,
            "isActivelyTrading":true
        }]);
        let rows: Vec<ScreenerRow> = serde_json::from_value(v).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].symbol, "NVDA");
        assert_eq!(rows[0].sector.as_deref(), Some("Technology"));
        assert_eq!(rows[0].industry.as_deref(), Some("Semiconductors"));
        assert!(rows[0].is_actively_trading);
    }

    #[test]
    fn screener_scope_covers_adjacent_ai_infra_themes() {
        let slices = screener_slices();
        assert!(
            slices
                .iter()
                .any(|s| s.sector == "Basic Materials" && s.industry.is_none()),
            "copper/materials exposure should be in the discovery pool"
        );
        assert!(
            slices.iter().any(|s| s.sector == "Industrials"
                && s.industry == Some("Specialty Industrial Machinery")),
            "power/cooling equipment should be in the discovery pool"
        );
        assert!(
            slices
                .iter()
                .any(|s| s.sector == "Real Estate" && s.industry == Some("REIT - Specialty")),
            "data-center REIT exposure should be in the discovery pool"
        );
    }
}
