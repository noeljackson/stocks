//! SEC EDGAR ingest via the submissions JSON API (free, requires UA).

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{NaiveDate, TimeZone, Utc};
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;

use super::{Adapter, Event};
use crate::platform::subjects;

const MAX_FILINGS: usize = 10;

pub struct EdgarAdapter {
    ua: String,
    ciks: HashMap<String, String>, // ticker → 10-digit CIK
    client: Client,
}

impl EdgarAdapter {
    pub fn new(user_agent: &str) -> Self {
        let ciks = HashMap::from([
            ("NVDA".to_string(), "0001045810".to_string()),
            ("MU".to_string(), "0000723125".to_string()),
        ]);
        Self {
            ua: user_agent.to_string(),
            ciks,
            client: Client::builder()
                .timeout(Duration::from_secs(20))
                .build()
                .expect("reqwest client"),
        }
    }
}

#[derive(Deserialize)]
struct Submissions {
    filings: Filings,
}

#[derive(Deserialize)]
struct Filings {
    recent: Recent,
}

#[derive(Deserialize)]
struct Recent {
    #[serde(default, rename = "accessionNumber")]
    accession_number: Vec<String>,
    #[serde(default, rename = "filingDate")]
    filing_date: Vec<String>,
    #[serde(default)]
    form: Vec<String>,
    #[serde(default, rename = "primaryDocument")]
    primary_document: Vec<String>,
}

#[async_trait]
impl Adapter for EdgarAdapter {
    fn name(&self) -> &str {
        "edgar"
    }
    fn interval(&self) -> Duration {
        Duration::from_secs(30 * 60)
    }

    async fn poll(&self) -> Result<Vec<Event>> {
        let mut out = Vec::new();
        for (ticker, cik) in &self.ciks {
            let evs = self.poll_one(ticker, cik).await
                .with_context(|| format!("edgar {ticker}"))?;
            out.extend(evs);
        }
        Ok(out)
    }
}

impl EdgarAdapter {
    async fn poll_one(&self, ticker: &str, cik: &str) -> Result<Vec<Event>> {
        let url = format!("https://data.sec.gov/submissions/CIK{cik}.json");
        let resp = self
            .client
            .get(&url)
            .header("User-Agent", &self.ua) // SEC requires a descriptive UA
            .header("Accept", "application/json")
            .send()
            .await?;
        if !resp.status().is_success() {
            anyhow::bail!("status {}", resp.status().as_u16());
        }
        let body: Submissions = resp.json().await?;
        let r = body.filings.recent;
        let n = r.accession_number.len().min(MAX_FILINGS);
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let filed = NaiveDate::parse_from_str(&r.filing_date[i], "%Y-%m-%d")
                .ok()
                .and_then(|d| d.and_hms_opt(0, 0, 0))
                .map(|ndt| Utc.from_utc_datetime(&ndt));
            let doc = r.primary_document.get(i).cloned().unwrap_or_default();
            let acc_no_dashes = r.accession_number[i].replace('-', "");
            let doc_url = format!(
                "https://www.sec.gov/Archives/edgar/data/{}/{acc_no_dashes}/{doc}",
                cik.trim_start_matches('0'),
            );
            let payload = json!({
                "ticker": ticker, "cik": cik, "form": r.form[i],
                "accession": r.accession_number[i], "filing_date": r.filing_date[i],
                "primary_document": doc, "url": doc_url,
            });
            out.push(Event {
                source: "edgar".into(),
                kind: r.form[i].clone(),
                symbol: ticker.to_string(),
                subject: subjects::INGEST_FILING.to_string(),
                payload: serde_json::to_vec(&payload)?,
                source_ts: filed,
            });
        }
        Ok(out)
    }
}
