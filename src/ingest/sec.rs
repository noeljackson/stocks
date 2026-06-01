//! Shared SEC helpers: ticker → CIK lookup, User-Agent constants, rate-limit
//! pacing (SEC's published cap is 10 req/s with a descriptive UA).

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result};
use reqwest::Client;
use serde::Deserialize;

/// Hardcoded ticker → 10-digit CIK map for the seed Tier-1 set. When the
/// system grows past ~25 names this should move to a DB column on `ticker`
/// (or fetch from <https://www.sec.gov/files/company_tickers.json>); for now
/// the small static table is enough and keeps the adapters dependency-free.
const SEED: &[(&str, &str)] = &[
    ("NVDA", "0001045810"),
    ("MU", "0000723125"),
    ("AMD", "0000002488"),
    ("AMAT", "0000006951"),
    ("TSM", "0001046179"),
    ("ANET", "0001596532"),
    ("VRT", "0001674101"),
    ("CDNS", "0000813672"),
];

#[must_use]
pub fn cik_for(symbol: &str) -> Option<&'static str> {
    let up = symbol.to_ascii_uppercase();
    SEED.iter()
        .find(|(s, _)| *s == up.as_str())
        .map(|(_, c)| *c)
}

#[must_use]
pub fn all_seeded() -> impl Iterator<Item = (&'static str, &'static str)> {
    SEED.iter().copied()
}

#[derive(Debug, Deserialize)]
struct CompanyTickerEntry {
    cik_str: u64,
    ticker: String,
}

#[must_use]
pub fn normalize_cik(cik: u64) -> String {
    format!("{cik:010}")
}

pub fn parse_company_tickers(raw: &str) -> Result<BTreeMap<String, String>> {
    let entries: BTreeMap<String, CompanyTickerEntry> =
        serde_json::from_str(raw).context("decode SEC company_tickers.json")?;
    let mut out = seeded_ciks();
    for entry in entries.into_values() {
        out.insert(
            entry.ticker.to_ascii_uppercase(),
            normalize_cik(entry.cik_str),
        );
    }
    Ok(out)
}

pub async fn fetch_company_ticker_map(
    client: &Client,
    user_agent: &str,
) -> Result<BTreeMap<String, String>> {
    let raw = client
        .get("https://www.sec.gov/files/company_tickers.json")
        .header("User-Agent", user_agent)
        .header("Accept", "application/json")
        .send()
        .await
        .context("fetch SEC company_tickers.json")?
        .error_for_status()
        .context("SEC company_tickers.json status")?
        .text()
        .await
        .context("read SEC company_tickers.json")?;
    parse_company_tickers(&raw)
}

pub async fn ciks_for_symbols(
    client: &Client,
    user_agent: &str,
    symbols: &[String],
) -> Result<Vec<(String, String)>> {
    let map = fetch_company_ticker_map(client, user_agent).await?;
    let requested: BTreeSet<String> = symbols.iter().map(|s| s.to_ascii_uppercase()).collect();
    Ok(requested
        .into_iter()
        .filter_map(|symbol| map.get(&symbol).map(|cik| (symbol, cik.clone())))
        .collect())
}

fn seeded_ciks() -> BTreeMap<String, String> {
    SEED.iter()
        .map(|(symbol, cik)| ((*symbol).to_string(), (*cik).to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_tickers_resolve() {
        assert_eq!(cik_for("NVDA"), Some("0001045810"));
        assert_eq!(cik_for("nvda"), Some("0001045810"), "case insensitive");
        assert_eq!(cik_for("UNKNOWN"), None);
    }

    #[test]
    fn parses_sec_company_tickers_and_pads_cik() {
        let raw = r#"{
            "0": {"cik_str": 826083, "ticker": "DELL", "title": "Dell Technologies Inc."},
            "1": {"cik_str": 1045810, "ticker": "NVDA", "title": "NVIDIA CORP"}
        }"#;

        let map = parse_company_tickers(raw).expect("parse");

        assert_eq!(map.get("DELL").map(String::as_str), Some("0000826083"));
        assert_eq!(map.get("NVDA").map(String::as_str), Some("0001045810"));
        assert_eq!(map.get("TSM").map(String::as_str), Some("0001046179"));
    }
}
