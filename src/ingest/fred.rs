//! FRED ingest via the official REST API (requires FRED_API_KEY).
//!
//! The keyless fredgraph CSV endpoint is now Akamai-blocked (returns 504 to
//! non-browser clients). Without a key this adapter no-ops with a one-shot
//! warning rather than failing every interval.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use chrono::{NaiveDate, TimeZone, Utc};
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use tracing::warn;

use super::rate_limit;
use super::{Adapter, Event};
use crate::platform::subjects;

const SERIES: &[&str] = &["DGS10", "DGS3MO", "BAMLH0A0HYM2", "VIXCLS"];

pub struct FredAdapter {
    api_key: String,
    warned_no_key: AtomicBool,
    client: Client,
}

impl FredAdapter {
    pub fn new(api_key: &str) -> Self {
        Self {
            api_key: api_key.to_string(),
            warned_no_key: AtomicBool::new(false),
            client: Client::builder()
                .timeout(Duration::from_secs(20))
                .build()
                .expect("reqwest client"),
        }
    }
}

#[derive(Deserialize)]
struct FredResp {
    #[serde(default)]
    observations: Vec<Obs>,
}

#[derive(Deserialize)]
struct Obs {
    date: String,
    value: String,
}

#[async_trait]
impl Adapter for FredAdapter {
    fn name(&self) -> &str {
        "fred"
    }
    fn interval(&self) -> Duration {
        Duration::from_secs(6 * 3600)
    }

    async fn poll(&self) -> Result<Vec<Event>> {
        if self.api_key.is_empty() {
            if !self.warned_no_key.swap(true, Ordering::Relaxed) {
                warn!("fred: FRED_API_KEY not set; skipping macro ingest");
            }
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for id in SERIES {
            if let Some(ev) = self.poll_one(id).await? {
                out.push(ev);
            }
        }
        Ok(out)
    }
}

impl FredAdapter {
    async fn poll_one(&self, id: &str) -> Result<Option<Event>> {
        let url = format!(
            "https://api.stlouisfed.org/fred/series/observations\
             ?series_id={id}&api_key={}&file_type=json&sort_order=desc&limit=1",
            self.api_key
        );
        rate_limit::fred().wait().await;
        let resp = self.client.get(&url).send().await?;
        let status = resp.status();
        let retry_after = rate_limit::retry_after(resp.headers());
        rate_limit::fred().observe_status(status, retry_after).await;
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "fred {id} {}: {}",
                status.as_u16(),
                &body[..body.len().min(256)]
            );
        }
        let parsed: FredResp = resp.json().await?;
        let Some(obs) = parsed.observations.into_iter().next() else {
            return Ok(None);
        };
        if obs.value.is_empty() || obs.value == "." {
            return Ok(None);
        }
        let ts = NaiveDate::parse_from_str(&obs.date, "%Y-%m-%d")
            .ok()
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .map(|ndt| Utc.from_utc_datetime(&ndt));
        let payload = json!({"series": id, "date": obs.date, "value": obs.value});
        Ok(Some(Event {
            source: "fred".into(),
            kind: "series".into(),
            symbol: String::new(),
            subject: subjects::INGEST_MACRO.to_string(),
            payload: serde_json::to_vec(&payload)?,
            source_ts: ts,
        }))
    }
}
