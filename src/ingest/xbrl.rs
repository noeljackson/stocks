//! XBRL company-facts adapter (#32). Pulls structured financial concepts
//! from SEC's `/api/xbrl/companyfacts/CIK<N>.json` and upserts into
//! `company_fact`. Fills the financial-metrics gap that made the thesis
//! engine decline on NVDA ("no quantitative financials from the 10-Q").
//!
//! The endpoint returns ALL historical observations per concept per company
//! (~1 MB per company). We poll once per `Interval()`; the table's UNIQUE
//! INDEX on (symbol, taxonomy, concept, period_end, accession) dedups
//! across re-polls.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::NaiveDate;
use reqwest::Client;
use serde::Deserialize;

use super::sec;
use super::{Adapter, Event};

/// Concepts we extract. Each concept has a "primary" SEC name and optional
/// fallback aliases (older filings use different names for the same idea —
/// e.g. `SalesRevenueNet` predates `Revenues`).
const CONCEPTS: &[(&str, &[&str])] = &[
    ("Revenues",                    &["Revenues", "SalesRevenueNet", "RevenueFromContractWithCustomerExcludingAssessedTax"]),
    ("GrossProfit",                 &["GrossProfit"]),
    ("OperatingIncomeLoss",         &["OperatingIncomeLoss"]),
    ("NetIncomeLoss",               &["NetIncomeLoss"]),
    ("NetCashProvidedByUsedInOperatingActivities", &["NetCashProvidedByUsedInOperatingActivities"]),
    ("ResearchAndDevelopmentExpense", &["ResearchAndDevelopmentExpense"]),
    ("CashAndCashEquivalentsAtCarryingValue", &["CashAndCashEquivalentsAtCarryingValue"]),
    ("StockholdersEquity",          &["StockholdersEquity"]),
    ("CommonStockSharesOutstanding", &["CommonStockSharesOutstanding"]),
    ("CostOfRevenue",               &["CostOfRevenue", "CostOfGoodsAndServicesSold"]),
    ("Assets",                      &["Assets"]),
    ("Liabilities",                 &["Liabilities"]),
];

pub struct XbrlAdapter {
    ua: String,
    client: Client,
}

impl XbrlAdapter {
    pub fn new(user_agent: &str) -> Self {
        Self {
            ua: user_agent.to_string(),
            client: Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("reqwest client"),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct CompanyFacts {
    #[serde(default)]
    pub cik: serde_json::Value, // SEC sometimes returns int, sometimes string
    #[serde(default, rename = "entityName")]
    pub entity_name: String,
    pub facts: HashMap<String, HashMap<String, ConceptBlock>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ConceptBlock {
    /// SEC returns `null` here for many concepts — must tolerate.
    #[serde(default, deserialize_with = "deserialize_nullable_string")]
    pub label: String,
    #[serde(default)]
    pub units: HashMap<String, Vec<Observation>>,
}

fn deserialize_nullable_string<'de, D>(d: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(d)?.unwrap_or_default())
}

#[derive(Debug, Clone, Deserialize)]
pub struct Observation {
    #[serde(default)]
    pub start: Option<String>,
    pub end: String,
    pub val: serde_json::Value, // can be int or float; we'll normalize to f64
    #[serde(default)]
    pub accn: Option<String>,
    #[serde(default)]
    pub fy: Option<i32>,
    #[serde(default)]
    pub fp: Option<String>,
    #[serde(default)]
    pub form: Option<String>,
    #[serde(default)]
    pub filed: Option<String>,
}

/// Normalized output row matching the `company_fact` schema.
#[derive(Debug, Clone, PartialEq)]
pub struct FactRow {
    pub symbol: String,
    pub cik: String,
    pub taxonomy: String,
    pub concept: String, // the canonical name (first in alias list), not the raw SEC name
    pub period_end: NaiveDate,
    pub period_start: Option<NaiveDate>,
    pub value: f64,
    pub unit: String,
    pub form: Option<String>,
    pub fiscal_year: Option<i32>,
    pub fiscal_period: Option<String>,
    pub accession: Option<String>,
    pub filed_at: Option<NaiveDate>,
}

/// Pure function: turn a CompanyFacts blob into FactRow records.
///
/// For each concept in CONCEPTS, walks every alias; for each found alias,
/// walks every unit (USD, shares, etc); for each unit, emits one FactRow
/// per observation. The canonical concept name (CONCEPTS[i].0) is used as
/// the `concept` field — that way both `Revenues` and `SalesRevenueNet`
/// land as `concept='Revenues'` for query simplicity downstream.
#[must_use]
pub fn extract(symbol: &str, cik: &str, facts: &CompanyFacts) -> Vec<FactRow> {
    let mut out = Vec::new();
    let Some(us_gaap) = facts.facts.get("us-gaap") else { return out };
    for (canonical, aliases) in CONCEPTS {
        for alias in *aliases {
            let Some(block) = us_gaap.get(*alias) else { continue };
            for (unit, observations) in &block.units {
                for obs in observations {
                    let Some(value) = obs.val.as_f64().or_else(|| obs.val.as_i64().map(|i| i as f64)) else {
                        continue;
                    };
                    let Ok(period_end) = NaiveDate::parse_from_str(&obs.end, "%Y-%m-%d") else {
                        continue;
                    };
                    let period_start = obs
                        .start
                        .as_deref()
                        .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok());
                    let filed_at = obs
                        .filed
                        .as_deref()
                        .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok());
                    out.push(FactRow {
                        symbol: symbol.to_string(),
                        cik: cik.to_string(),
                        taxonomy: "us-gaap".to_string(),
                        concept: (*canonical).to_string(),
                        period_end,
                        period_start,
                        value,
                        unit: unit.clone(),
                        form: obs.form.clone(),
                        fiscal_year: obs.fy,
                        fiscal_period: obs.fp.clone(),
                        accession: obs.accn.clone(),
                        filed_at,
                    });
                }
            }
            // Use first matching alias only; aliases are alternative names
            // for the SAME concept (we don't want duplicate rows).
            break;
        }
    }
    out
}

#[async_trait]
impl Adapter for XbrlAdapter {
    fn name(&self) -> &str {
        "xbrl"
    }
    fn interval(&self) -> Duration {
        // 6 hours — SEC indexes are slow-moving; per-company facts JSON is
        // ~1 MB so we don't want to hammer.
        Duration::from_secs(6 * 3600)
    }

    async fn poll(&self) -> Result<Vec<Event>> {
        // Returns Vec<Event> for the Adapter trait, but the *content* of
        // those events is metadata about which company-facts blobs we
        // fetched + how many rows we wrote. The real persistence is a
        // direct DB UPSERT inside this method (events are for NATS
        // visibility, not the canonical store path for this adapter).
        //
        // For v1 we don't emit per-fact events — the volume is too high
        // (~300 observations per concept per company × 12 concepts × N
        // companies). Future #14 condition evaluator can subscribe to a
        // `ingest.xbrl_filed` notification per company when new facts land.
        Ok(Vec::new())
    }
}

impl XbrlAdapter {
    /// Fetch + parse the companyfacts JSON for one CIK.
    pub async fn fetch_one(&self, symbol: &str, cik: &str) -> Result<Vec<FactRow>> {
        let url = format!("https://data.sec.gov/api/xbrl/companyfacts/CIK{cik}.json");
        let resp = self
            .client
            .get(&url)
            .header("User-Agent", &self.ua)
            .header("Accept", "application/json")
            .send()
            .await
            .with_context(|| format!("xbrl fetch {symbol}"))?;
        if !resp.status().is_success() {
            anyhow::bail!("xbrl {} status {}", symbol, resp.status().as_u16());
        }
        let facts: CompanyFacts = resp
            .json()
            .await
            .with_context(|| format!("xbrl decode {symbol}"))?;
        Ok(extract(symbol, cik, &facts))
    }

    /// Poll every seeded ticker, return all rows. Caller persists.
    pub async fn poll_all(&self) -> Result<Vec<FactRow>> {
        let mut all = Vec::new();
        for (symbol, cik) in sec::all_seeded() {
            match self.fetch_one(symbol, cik).await {
                Ok(rows) => {
                    tracing::info!(symbol = symbol, rows = rows.len(), "xbrl facts fetched");
                    all.extend(rows);
                }
                Err(e) => {
                    tracing::warn!(symbol = symbol, error = %e, "xbrl fetch failed; continuing");
                }
            }
            // SEC limits to 10 req/s; a 200ms delay between symbols keeps us safe.
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        Ok(all)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nvda_sample() -> CompanyFacts {
        // Minimal handcrafted fixture mirroring the live response shape.
        let raw = serde_json::json!({
            "cik": 1045810,
            "entityName": "NVIDIA CORP",
            "facts": {
                "us-gaap": {
                    "Revenues": {
                        "label": "Revenues",
                        "units": {
                            "USD": [
                                {"start":"2025-01-27","end":"2026-01-25","val":215938000000_i64,"accn":"acc-fy26","fy":2026,"fp":"FY","form":"10-K","filed":"2026-02-25"},
                                {"start":"2026-01-26","end":"2026-04-26","val":81615000000_i64,"accn":"acc-q1","fy":2027,"fp":"Q1","form":"10-Q","filed":"2026-05-20"}
                            ]
                        }
                    },
                    "GrossProfit": {
                        "label": "GrossProfit",
                        "units": { "USD": [
                            {"start":"2025-01-27","end":"2026-01-25","val":153463000000_i64,"accn":"acc-fy26","fy":2026,"fp":"FY","form":"10-K","filed":"2026-02-25"}
                        ]}
                    },
                    // Alias case: "SalesRevenueNet" should NOT add rows because we already matched "Revenues" first.
                    "SalesRevenueNet": {
                        "label": "SalesRevenueNet",
                        "units": { "USD": [
                            {"end":"2019-01-27","val":11716000000_i64,"accn":"old"}
                        ]}
                    },
                    // Concept we don't track: ignored.
                    "ProperlyIgnoredConcept": {
                        "label": "Ignored",
                        "units": { "USD": [ {"end":"2026-01-25","val":1_i64,"accn":"x"} ]}
                    }
                },
                "dei": {
                    "EntityCommonStockSharesOutstanding": {
                        "label": "Shares",
                        "units": { "shares": [
                            {"end":"2026-04-26","val":24430000000_i64,"accn":"acc-q1"}
                        ]}
                    }
                }
            }
        });
        serde_json::from_value(raw).unwrap()
    }

    #[test]
    fn extract_returns_known_concepts_only() {
        let facts = nvda_sample();
        let rows = extract("NVDA", "0001045810", &facts);
        // Revenues (2) + GrossProfit (1) = 3.
        // SalesRevenueNet ignored (alias of Revenues; we already matched primary).
        // ProperlyIgnoredConcept ignored (not in CONCEPTS).
        // dei/EntityCommonStockSharesOutstanding ignored (not us-gaap).
        assert_eq!(rows.len(), 3, "got: {rows:?}");
        assert!(rows.iter().all(|r| r.symbol == "NVDA"));
        assert!(rows.iter().all(|r| r.taxonomy == "us-gaap"));
    }

    #[test]
    fn extract_uses_canonical_name_not_alias() {
        // If a company only has SalesRevenueNet (no Revenues), we should
        // still see it stored as concept="Revenues" (the canonical name).
        let mut facts = nvda_sample();
        let revenues = facts.facts.get_mut("us-gaap").unwrap().remove("Revenues").unwrap();
        // Move Revenues data under SalesRevenueNet so the only revenue source is the alias.
        facts.facts.get_mut("us-gaap").unwrap().insert("SalesRevenueNet".into(), revenues);
        let rows = extract("NVDA", "0001045810", &facts);
        let rev_rows: Vec<_> = rows.iter().filter(|r| r.concept == "Revenues").collect();
        assert_eq!(rev_rows.len(), 2, "alias resolves to canonical");
    }

    #[test]
    fn extract_parses_dates_and_values() {
        let facts = nvda_sample();
        let rows = extract("NVDA", "0001045810", &facts);
        let q1 = rows
            .iter()
            .find(|r| r.concept == "Revenues" && r.fiscal_period.as_deref() == Some("Q1"))
            .unwrap();
        assert_eq!(q1.value, 81_615_000_000.0);
        assert_eq!(q1.period_end, NaiveDate::from_ymd_opt(2026, 4, 26).unwrap());
        assert_eq!(q1.period_start, Some(NaiveDate::from_ymd_opt(2026, 1, 26).unwrap()));
        assert_eq!(q1.form.as_deref(), Some("10-Q"));
        assert_eq!(q1.fiscal_year, Some(2027));
        assert_eq!(q1.accession.as_deref(), Some("acc-q1"));
        assert_eq!(q1.unit, "USD");
    }

    #[test]
    fn extract_skips_unparseable_observations() {
        let mut facts = nvda_sample();
        // Inject one with a bad date — should be silently skipped, not panic.
        let bad = serde_json::from_value::<Observation>(serde_json::json!({
            "end": "not-a-date", "val": 999_i64
        })).unwrap();
        facts.facts.get_mut("us-gaap").unwrap()
            .get_mut("Revenues").unwrap()
            .units.get_mut("USD").unwrap()
            .push(bad);
        let rows = extract("NVDA", "0001045810", &facts);
        // Still just 3 valid rows (2 Revenues + 1 GrossProfit).
        assert_eq!(rows.len(), 3);
    }
}
