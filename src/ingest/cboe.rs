//! CBOE-sourced macro crowd-sentiment adapters (#20).
//!
//! Two endpoints, both free public CSV downloads:
//! - Equity Put/Call Ratio: `equitypc.csv`
//! - VIX history: `VIX_History.csv`
//!
//! Both have a preamble (CBOE legal blurb / single header row) followed by
//! comma-separated daily rows. We pull the file, parse, and emit normalized
//! (source, metric, value, observed_at) rows that the service layer upserts
//! into `crowd_sentiment`.

use std::time::Duration;

use anyhow::{Context, Result};
use chrono::NaiveDate;
use reqwest::Client;

/// One normalized observation ready for `crowd_sentiment` upsert.
#[derive(Debug, Clone, PartialEq)]
pub struct CrowdRow {
    pub source: &'static str,
    pub metric: &'static str,
    pub value: f64,
    pub observed_at: NaiveDate,
}

/// Parse CBOE's equity put/call CSV.
///
/// Layout (as of 2026-05-31):
///   - first lines: legal preamble (single text cells)
///   - then a row of headers: `DATE,CALL,PUT,TOTAL,P/C Ratio`
///   - then daily rows like `2026-05-30,3000000,2100000,5100000,0.70`
///
/// We skip until we find the `DATE` header row and then parse what follows.
#[must_use]
pub fn parse_cboe_pcr_csv(body: &str) -> Vec<CrowdRow> {
    let mut out = Vec::new();
    let mut in_data = false;
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with("DATE,") {
            in_data = true;
            continue;
        }
        if !in_data {
            continue;
        }
        let cols: Vec<&str> = line.split(',').collect();
        if cols.len() < 5 {
            continue;
        }
        // CBOE serves dates in MM/D/YYYY for the historical PCR file; try
        // both that and ISO so future format flips don't silently drop us.
        let date = NaiveDate::parse_from_str(cols[0].trim(), "%m/%d/%Y")
            .or_else(|_| NaiveDate::parse_from_str(cols[0].trim(), "%Y-%m-%d"))
            .ok();
        let Some(date) = date else { continue };
        let Ok(pcr) = cols[4].trim().parse::<f64>() else {
            continue;
        };
        out.push(CrowdRow {
            source: "cboe_pcr",
            metric: "equity_pcr",
            value: pcr,
            observed_at: date,
        });
    }
    out
}

/// Parse CBOE's VIX history CSV.
///
/// Layout: `DATE,OPEN,HIGH,LOW,CLOSE`. Dates in `MM/DD/YYYY` format
/// (CBOE's US convention, not ISO). We emit one row per date for the close.
#[must_use]
pub fn parse_cboe_vix_csv(body: &str) -> Vec<CrowdRow> {
    let mut out = Vec::new();
    let mut header_seen = false;
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with("DATE,") {
            header_seen = true;
            continue;
        }
        if !header_seen {
            continue;
        }
        let cols: Vec<&str> = line.split(',').collect();
        if cols.len() < 5 {
            continue;
        }
        // CBOE uses MM/DD/YYYY; newer files sometimes serve ISO. Try both.
        let date = NaiveDate::parse_from_str(cols[0].trim(), "%m/%d/%Y")
            .or_else(|_| NaiveDate::parse_from_str(cols[0].trim(), "%Y-%m-%d"))
            .ok();
        let Some(date) = date else { continue };
        let Ok(close) = cols[4].trim().parse::<f64>() else {
            continue;
        };
        out.push(CrowdRow {
            source: "cboe_vix",
            metric: "vix_close",
            value: close,
            observed_at: date,
        });
    }
    out
}

pub struct CboeAdapter {
    client: Client,
}

impl Default for CboeAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl CboeAdapter {
    #[must_use]
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(30))
                .user_agent("Mozilla/5.0 (compatible; stocks-trading-intel/0.1)")
                .build()
                .expect("reqwest client"),
        }
    }

    pub async fn fetch_pcr(&self) -> Result<Vec<CrowdRow>> {
        let url = "https://cdn.cboe.com/resources/options/volume_and_call_put_ratios/equitypc.csv";
        let body = self
            .client
            .get(url)
            .send()
            .await
            .context("fetch cboe pcr")?
            .error_for_status()
            .context("cboe pcr http")?
            .text()
            .await
            .context("cboe pcr body")?;
        Ok(parse_cboe_pcr_csv(&body))
    }

    pub async fn fetch_vix(&self) -> Result<Vec<CrowdRow>> {
        let url = "https://cdn.cboe.com/api/global/us_indices/daily_prices/VIX_History.csv";
        let body = self
            .client
            .get(url)
            .send()
            .await
            .context("fetch cboe vix")?
            .error_for_status()
            .context("cboe vix http")?
            .text()
            .await
            .context("cboe vix body")?;
        Ok(parse_cboe_vix_csv(&body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_pcr_handles_cboe_us_date_format() {
        // Actual live CBOE format as of 2026-05-31 — US dates, not ISO.
        let csv = "Cboe Volume and Put/Call Ratio data... legal blurb on one line,,,,\n\
                   , PRODUCT: EQUITY,,EXCHANGE: Cboe,\n\
                   DATE,CALL,PUT,TOTAL,P/C Ratio\n\
                   11/1/2006,976510,623929,1600439,0.64\n\
                   10/04/2019, 916877, 598296, 1515173, 0.65\n";
        let rows = parse_cboe_pcr_csv(csv);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].source, "cboe_pcr");
        assert_eq!(rows[0].metric, "equity_pcr");
        assert_eq!(rows[0].value, 0.64);
        assert_eq!(rows[0].observed_at.to_string(), "2006-11-01");
        assert_eq!(rows[1].value, 0.65);
    }

    #[test]
    fn parse_pcr_also_accepts_iso_dates() {
        let csv = "DATE,CALL,PUT,TOTAL,P/C Ratio\n\
                   2026-05-29,3000000,2100000,5100000,0.70\n";
        let rows = parse_cboe_pcr_csv(csv);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].observed_at.to_string(), "2026-05-29");
    }

    #[test]
    fn parse_pcr_drops_unparseable_dates() {
        let csv = "DATE,CALL,PUT,TOTAL,P/C Ratio\n\
                   not-a-date,1,1,2,0.5\n\
                   2026-05-30,1,1,2,0.8\n";
        let rows = parse_cboe_pcr_csv(csv);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].value, 0.8);
    }

    #[test]
    fn parse_vix_handles_cboe_date_format() {
        let csv = "DATE,OPEN,HIGH,LOW,CLOSE\n\
                   01/02/1990,17.24,17.24,17.24,17.24\n\
                   05/30/2026,14.50,15.10,14.20,14.85\n";
        let rows = parse_cboe_vix_csv(csv);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].metric, "vix_close");
        assert_eq!(rows[0].value, 17.24);
        assert_eq!(rows[1].observed_at.to_string(), "2026-05-30");
        assert_eq!(rows[1].value, 14.85);
    }

    #[test]
    fn parse_vix_also_accepts_iso_dates() {
        let csv = "DATE,OPEN,HIGH,LOW,CLOSE\n\
                   2026-05-30,14.50,15.10,14.20,14.85\n";
        let rows = parse_cboe_vix_csv(csv);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].observed_at.to_string(), "2026-05-30");
    }

    #[test]
    fn parse_pcr_handles_empty_body() {
        assert!(parse_cboe_pcr_csv("").is_empty());
        assert!(parse_cboe_vix_csv("").is_empty());
    }
}
