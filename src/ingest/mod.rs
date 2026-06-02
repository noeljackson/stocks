//! Adapter framework: fetch → normalize → append-only store + emit (SPEC §3).
//!
//! Each adapter is a small async object with `name`, `interval`, `poll`. The
//! [`run`] runner spawns each on its own ticker and dedups via `content_hash`
//! (so re-polling old data is harmless).

pub mod cboe;
pub mod crowd_sentiment_service;
pub mod discovery_pool_service;
pub mod edgar;
pub mod fmp;
pub mod fmp_estimates;
pub mod fmp_estimates_service;
pub mod fmp_intraday;
pub mod fmp_news;
pub mod fmp_opinion;
pub mod fmp_opinion_service;
pub mod fmp_screener;
pub mod fred;
pub mod massive;
pub mod massive_news;
pub mod news_service;
pub mod rate_limit;
pub mod sec;
pub mod source_health;
pub mod twse;
pub mod xbrl;

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use tokio::sync::Notify;
use tokio::task::JoinSet;
use tracing::{error, info};

use crate::platform::bus::Bus;
use crate::platform::store::Store;

const MACRO_TARGET: &str = "macro_regime";

#[must_use]
pub fn interval_secs_from_env(name: &str, default_secs: u64) -> Duration {
    let secs = std::env::var(name)
        .ok()
        .and_then(|v| parse_interval_secs(&v, default_secs))
        .unwrap_or(default_secs);
    Duration::from_secs(secs.max(1))
}

#[must_use]
pub fn max_symbols_from_env(name: &str, default_symbols: i64) -> i64 {
    std::env::var(name)
        .ok()
        .and_then(|v| parse_positive_i64(&v))
        .unwrap_or(default_symbols)
        .max(1)
}

#[must_use]
pub fn parse_interval_secs(raw: &str, default_secs: u64) -> Option<u64> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    match trimmed.parse::<u64>() {
        Ok(0) => Some(default_secs),
        Ok(v) => Some(v),
        Err(_) => None,
    }
}

#[must_use]
pub fn parse_positive_i64(raw: &str) -> Option<i64> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    match trimmed.parse::<i64>() {
        Ok(v) if v > 0 => Some(v),
        _ => None,
    }
}

/// Normalized item produced by an adapter.
#[derive(Debug, Clone)]
pub struct Event {
    pub source: String,
    pub kind: String,
    pub symbol: String,   // "" for market-wide
    pub subject: String,  // NATS subject to publish on
    pub payload: Vec<u8>, // canonical JSON
    pub source_ts: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
struct FilingEvidenceFields {
    symbol: String,
    observed_at: DateTime<Utc>,
    source_id: String,
    source_ref: serde_json::Value,
    summary: String,
    strength: f64,
    url: Option<String>,
}

impl Event {
    /// Stable dedup key over source + kind + symbol + payload.
    #[must_use]
    pub fn content_hash(&self) -> String {
        let mut h = Sha256::new();
        for s in [&self.source, &self.kind, &self.symbol] {
            h.update(s.as_bytes());
            h.update([0]);
        }
        h.update(&self.payload);
        hex::encode(h.finalize())
    }
}

#[async_trait]
pub trait Adapter: Send + Sync {
    fn name(&self) -> &str;
    fn interval(&self) -> Duration;
    async fn poll(&self) -> Result<Vec<Event>>;
}

/// Runs adapters concurrently until `shutdown` resolves.
pub async fn run<F>(
    store: Store,
    bus: Bus,
    adapters: Vec<Box<dyn Adapter>>,
    shutdown: F,
) -> Result<()>
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    let store = Arc::new(store);
    let bus = Arc::new(bus);
    let stop = Arc::new(Notify::new());
    let mut set: JoinSet<()> = JoinSet::new();

    for adapter in adapters {
        let store = store.clone();
        let bus = bus.clone();
        let stop = stop.clone();
        set.spawn(async move { adapter_loop(adapter, store, bus, stop).await });
    }

    // Wait for the caller's shutdown signal, then notify every adapter.
    shutdown.await;
    info!("ingest run: shutdown signaled");
    stop.notify_waiters();
    while set.join_next().await.is_some() {}
    Ok(())
}

async fn adapter_loop(
    adapter: Box<dyn Adapter>,
    store: Arc<Store>,
    bus: Arc<Bus>,
    stop: Arc<Notify>,
) {
    let interval = adapter.interval();
    let name = adapter.name().to_string();
    loop {
        run_once(&*adapter, &store, &bus, &name).await;
        tokio::select! {
            () = tokio::time::sleep(interval) => {},
            () = stop.notified() => {
                info!(adapter = %name, "adapter stopping");
                return;
            }
        }
    }
}

async fn run_once(adapter: &dyn Adapter, store: &Store, bus: &Bus, name: &str) {
    let owner = format!("ingest.{name}");
    if let Err(e) = store.mark_source_started(name, 0).await {
        error!(adapter = name, error = %e, "source health start record failed");
    }
    let benchmark_task = benchmark_task_for_adapter(name);
    if let Some((action, target)) = benchmark_task {
        if let Err(e) = store
            .mark_source_tasks_fetching_for_scope(
                "benchmark",
                &[action],
                &[target.to_string()],
                &owner,
            )
            .await
        {
            error!(adapter = name, error = %e, "source task claim failed");
        }
    }
    let events = match adapter.poll().await {
        Ok(e) => e,
        Err(e) => {
            let message = e.to_string();
            let retry_after_at = if name == "fred" && is_rate_limit_error(&message) {
                rate_limit::fred().retry_after_at().await
            } else {
                None
            };
            let failure_kind = if is_rate_limit_error(&message) {
                "rate_limited"
            } else {
                "error"
            };
            if let Err(record_err) = store
                .record_source_failure(name, failure_kind, &message, retry_after_at)
                .await
            {
                error!(adapter = name, error = %record_err, "source health failure record failed");
            }
            if let Some((action, target)) = benchmark_task {
                if let Err(task_err) = store
                    .fail_source_tasks_for_scope(
                        "benchmark",
                        action,
                        &[target.to_string()],
                        &owner,
                        failure_kind,
                        &message,
                        retry_after_at,
                    )
                    .await
                {
                    error!(adapter = name, error = %task_err, "source task failure record failed");
                }
            }
            error!(adapter = name, error = %e, "poll failed");
            return;
        }
    };
    let rows_seen = events.len() as i64;
    let symbols_attempted = events
        .iter()
        .filter_map(|ev| (!ev.symbol.is_empty()).then_some(ev.symbol.as_str()))
        .collect::<std::collections::BTreeSet<_>>()
        .len() as i32;
    let mut stored = 0u32;
    let mut published = 0u32;
    for ev in events {
        let symbol_opt = if ev.symbol.is_empty() {
            None
        } else {
            Some(ev.symbol.as_str())
        };
        let inserted = match store
            .append_ingest_event(
                &ev.source,
                &ev.kind,
                symbol_opt,
                &ev.payload,
                &ev.content_hash(),
                ev.source_ts,
            )
            .await
        {
            Ok(b) => b,
            Err(e) => {
                error!(adapter = name, error = %e, "store failed");
                continue;
            }
        };
        if !inserted {
            continue; // already seen
        }
        stored += 1;
        if let Some(fields) = filing_evidence_fields(&ev) {
            if let Err(e) = upsert_filing_evidence_item(store, &fields).await {
                error!(
                    adapter = name,
                    symbol = %ev.symbol,
                    error = %e,
                    "filing evidence_item upsert failed"
                );
            }
        }
        if !ev.subject.is_empty() {
            if let Err(e) = bus.publish(&ev.subject, &ev.payload).await {
                error!(adapter = name, subject = %ev.subject, error = %e, "publish failed");
                continue;
            }
            published += 1;
        }
    }
    if stored > 0 {
        info!(adapter = name, new = stored, published, "ingested");
    }
    if let Err(e) = store
        .record_source_success(name, rows_seen, stored as i64, symbols_attempted, 0)
        .await
    {
        error!(adapter = name, error = %e, "source health success record failed");
    }
    if let Some((action, target)) = benchmark_task {
        let targets_with_rows = if rows_seen > 0 {
            vec![target.to_string()]
        } else {
            Vec::new()
        };
        if let Err(e) = store
            .complete_source_tasks_for_scope(
                "benchmark",
                action,
                &[target.to_string()],
                &targets_with_rows,
                &owner,
                chrono::Duration::minutes(30),
            )
            .await
        {
            error!(adapter = name, error = %e, "source task completion failed");
        }
    }
}

fn filing_evidence_strength(form: &str) -> f64 {
    let normalized = form.trim().to_ascii_uppercase();
    match normalized.trim_end_matches("/A") {
        "10-K" | "10-Q" | "8-K" | "S-1" | "S-3" | "S-4" => 0.65,
        _ => 0.45,
    }
}

fn filing_evidence_fields(ev: &Event) -> Option<FilingEvidenceFields> {
    if ev.source != "edgar" || ev.symbol.is_empty() {
        return None;
    }
    let payload: serde_json::Value = serde_json::from_slice(&ev.payload).ok()?;
    let form = payload
        .get("form")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or(&ev.kind);
    let filing_date = payload
        .get("filing_date")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty());
    let accession = payload
        .get("accession")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty());
    let source_id = accession.map_or_else(
        || format!("edgar_event:{}:{}", ev.symbol, ev.content_hash()),
        |acc| format!("edgar_filing:{}:{acc}", ev.symbol),
    );
    let summary = filing_date.map_or_else(
        || format!("{} {form} filed", ev.symbol),
        |date| format!("{} {form} filed {date}", ev.symbol),
    );
    let source_ref = serde_json::json!({
        "table": "ingest_event",
        "source": "edgar",
        "content_hash": ev.content_hash(),
        "kind": ev.kind,
        "cik": payload.get("cik").cloned(),
        "form": form,
        "accession": accession,
        "filing_date": filing_date,
        "primary_document": payload.get("primary_document").cloned(),
        "url": payload.get("url").cloned(),
    });
    let url = payload
        .get("url")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    Some(FilingEvidenceFields {
        symbol: ev.symbol.clone(),
        observed_at: ev.source_ts.unwrap_or_else(Utc::now),
        source_id,
        source_ref,
        summary,
        strength: filing_evidence_strength(form),
        url,
    })
}

async fn upsert_filing_evidence_item(store: &Store, fields: &FilingEvidenceFields) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO evidence_item
             (symbol, kind, observed_at, source, source_id, source_ref,
              summary, strength, polarity, url)
           VALUES (
             $1, 'filing', $2, 'edgar', $3, $4,
             $5, $6, NULL, $7
           )
           ON CONFLICT (source, source_id) DO UPDATE SET
             source_ref = evidence_item.source_ref || EXCLUDED.source_ref,
             summary = EXCLUDED.summary,
             strength = EXCLUDED.strength,
             url = EXCLUDED.url,
             updated_at = now()"#,
    )
    .bind(&fields.symbol)
    .bind(fields.observed_at)
    .bind(&fields.source_id)
    .bind(&fields.source_ref)
    .bind(&fields.summary)
    .bind(fields.strength)
    .bind(&fields.url)
    .execute(&store.pool)
    .await
    .context("upsert filing evidence_item")?;
    Ok(())
}

fn is_rate_limit_error(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("429") || lower.contains("rate limit")
}

fn benchmark_task_for_adapter(name: &str) -> Option<(&'static str, &'static str)> {
    match name {
        "fred" => Some(("fred_macro", MACRO_TARGET)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn content_hash_dedups_across_runs() {
        let e1 = Event {
            source: "edgar".into(),
            kind: "10-K".into(),
            symbol: "NVDA".into(),
            subject: "ingest.filing".into(),
            payload: br#"{"x":1}"#.to_vec(),
            source_ts: None,
        };
        let mut e2 = e1.clone();
        e2.source_ts = Some(Utc::now()); // source_ts is NOT in the hash
        assert_eq!(e1.content_hash(), e2.content_hash());
    }

    #[test]
    fn content_hash_differs_by_symbol() {
        let mut a = Event {
            source: "edgar".into(),
            kind: "10-K".into(),
            symbol: "NVDA".into(),
            subject: String::new(),
            payload: b"{}".to_vec(),
            source_ts: None,
        };
        let original = a.content_hash();
        a.symbol = "MU".into();
        assert_ne!(a.content_hash(), original);
    }

    #[test]
    fn filing_evidence_fields_extracts_edgar_payload() {
        let ev = Event {
            source: "edgar".into(),
            kind: "10-Q".into(),
            symbol: "MU".into(),
            subject: "ingest.filing".into(),
            payload: br#"{
              "ticker":"MU",
              "cik":"0000723125",
              "form":"10-Q",
              "accession":"0000723125-26-000010",
              "filing_date":"2026-05-20",
              "primary_document":"mu-20260520.htm",
              "url":"https://www.sec.gov/Archives/edgar/data/723125/000072312526000010/mu-20260520.htm"
            }"#
            .to_vec(),
            source_ts: Some(Utc.with_ymd_and_hms(2026, 5, 20, 0, 0, 0).unwrap()),
        };

        let fields = filing_evidence_fields(&ev).expect("filing evidence");

        assert_eq!(fields.symbol, "MU");
        assert_eq!(fields.observed_at, ev.source_ts.unwrap());
        assert_eq!(fields.source_id, "edgar_filing:MU:0000723125-26-000010");
        assert_eq!(fields.summary, "MU 10-Q filed 2026-05-20");
        assert_eq!(fields.strength, 0.65);
        assert_eq!(
            fields.url.as_deref(),
            Some(
                "https://www.sec.gov/Archives/edgar/data/723125/000072312526000010/mu-20260520.htm"
            ),
        );
        assert_eq!(fields.source_ref["table"], "ingest_event");
        assert_eq!(fields.source_ref["accession"], "0000723125-26-000010");
    }

    #[test]
    fn filing_evidence_fields_ignores_non_edgar_events() {
        let ev = Event {
            source: "fmp".into(),
            kind: "price".into(),
            symbol: "MU".into(),
            subject: String::new(),
            payload: b"{}".to_vec(),
            source_ts: None,
        };

        assert!(filing_evidence_fields(&ev).is_none());
    }

    #[test]
    fn filing_evidence_strength_handles_common_amendments() {
        assert_eq!(filing_evidence_strength("10-K/A"), 0.65);
        assert_eq!(filing_evidence_strength(" 8-k "), 0.65);
        assert_eq!(filing_evidence_strength("SC 13G"), 0.45);
    }

    #[test]
    fn parse_interval_secs_tolerates_bad_inputs() {
        assert_eq!(parse_interval_secs("1800", 60), Some(1800));
        assert_eq!(parse_interval_secs(" 30 ", 60), Some(30));
        assert_eq!(parse_interval_secs("0", 60), Some(60));
        assert_eq!(parse_interval_secs("", 60), None);
        assert_eq!(parse_interval_secs("nope", 60), None);
    }

    #[test]
    fn parse_positive_i64_tolerates_bad_inputs() {
        assert_eq!(parse_positive_i64("75"), Some(75));
        assert_eq!(parse_positive_i64(" 12 "), Some(12));
        assert_eq!(parse_positive_i64("0"), None);
        assert_eq!(parse_positive_i64("-1"), None);
        assert_eq!(parse_positive_i64(""), None);
        assert_eq!(parse_positive_i64("nope"), None);
    }

    #[test]
    fn fred_adapter_maps_to_macro_benchmark_task() {
        assert_eq!(
            benchmark_task_for_adapter("fred"),
            Some(("fred_macro", "macro_regime"))
        );
        assert_eq!(benchmark_task_for_adapter("edgar"), None);
    }
}
