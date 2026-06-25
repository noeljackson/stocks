//! Delayed live-bar publisher from FMP intraday bars.
//!
//! This is the first provider-backed bridge for chart live updates. It polls
//! the newest native 1-minute bars for the priority universe, persists them to
//! `price_bar_intraday`, then emits aggregate `market.bar.<interval>.<symbol>`
//! events. FMP polling is delayed market data, not a true websocket feed.

use std::time::Duration;

use anyhow::Result;
use chrono::{DateTime, TimeZone, Utc};
use serde::Serialize;
use tracing::{error, info, warn};

use crate::ingest::fmp_intraday::{FmpIntradayAdapter, IntradayPriceBarRow};
use crate::ingest::{interval_secs_from_env, max_symbols_from_env, rate_limit, source_health};
use crate::platform::{bus::Bus, store::Store, subjects};

const SOURCE: &str = "fmp_live_bars";
const OWNER: &str = "ingest.fmp_live_bars";
const NATIVE_INTERVAL: &str = "1min";
const STATUS: &str = "delayed";
const FRESH_FOR_MINUTES: i64 = 10;
const CHART_INTERVALS: &[(&str, i64)] = &[
    ("1m", 1),
    ("3m", 3),
    ("5m", 5),
    ("15m", 15),
    ("30m", 30),
    ("1h", 60),
    ("2h", 120),
    ("4h", 240),
];
const STATUS_ONLY_INTERVALS: &[&str] = &["1W", "3W", "1M"];

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MarketBarPayload {
    pub symbol: String,
    pub interval: String,
    pub time: String,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
    pub status: String,
    pub provider: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct MarketStatusPayload {
    symbol: String,
    interval: String,
    status: String,
    provider: String,
    reason: String,
}

#[derive(Debug, Clone, Copy, Default)]
struct PassOutcome {
    entitlement_blocked: bool,
}

pub async fn run(
    store: Store,
    bus: Bus,
    adapter: FmpIntradayAdapter,
    interval: Duration,
) -> Result<()> {
    bus.ensure_stream(subjects::STREAM_MARKET, subjects::MARKET_STREAM_SUBJECTS)
        .await?;
    let max_symbols = max_symbols_from_env("FMP_LIVE_BAR_MAX_SYMBOLS_PER_PASS", 25);
    let entitlement_backoff = interval_secs_from_env("FMP_LIVE_BAR_ENTITLEMENT_BACKOFF_SECS", 3600);
    let mut entitlement_blocked_until: Option<DateTime<Utc>> = None;
    loop {
        if !adapter.configured() {
            warn!("FMP_API_KEY missing; fmp_live_bars publisher idle");
            tokio::time::sleep(interval).await;
            continue;
        }
        if let Some(blocked_until) = entitlement_blocked_until {
            let now = Utc::now();
            if now < blocked_until {
                warn!(until = %blocked_until, "FMP live bars entitlement blocked; publisher idle");
                tokio::time::sleep(interval).await;
                continue;
            }
            entitlement_blocked_until = None;
        }
        match run_once(&store, &bus, &adapter, max_symbols).await {
            Ok(outcome) => {
                if outcome.entitlement_blocked {
                    let backoff = chrono::Duration::from_std(entitlement_backoff)
                        .unwrap_or_else(|_| chrono::Duration::hours(1));
                    entitlement_blocked_until = Some(Utc::now() + backoff);
                }
            }
            Err(e) => {
                let message = e.to_string();
                let retry_after_at = if source_health::failure_kind(&message) == "rate_limited" {
                    rate_limit::fmp().retry_after_at().await
                } else {
                    None
                };
                if let Err(record_err) = store
                    .record_source_failure(
                        SOURCE,
                        source_health::failure_kind(&message),
                        &message,
                        retry_after_at,
                    )
                    .await
                {
                    error!(error = %record_err, "fmp_live_bars source health failure record failed");
                }
                error!(error = %e, "fmp_live_bars pass failed");
            }
        }
        tokio::time::sleep(interval).await;
    }
}

async fn run_once(
    store: &Store,
    bus: &Bus,
    adapter: &FmpIntradayAdapter,
    max_symbols: i64,
) -> Result<PassOutcome> {
    let symbols = store.priority_scan_symbols(max_symbols).await?;
    if symbols.is_empty() {
        return Ok(PassOutcome::default());
    }
    store
        .mark_source_started(SOURCE, symbols.len() as i32)
        .await?;
    if let Err(e) = store
        .mark_source_tasks_fetching(&["fmp_live_bars"], &symbols, OWNER)
        .await
    {
        warn!(error = %e, "fmp_live_bars source task claim failed");
    }

    let mut rows_seen = 0_i64;
    let mut rows_inserted = 0_i64;
    let mut symbols_failed = 0_i32;
    let mut symbols_with_rows = Vec::new();
    let mut outcome = PassOutcome::default();
    for (index, symbol) in symbols.iter().enumerate() {
        match adapter.fetch_one(symbol, NATIVE_INTERVAL, 1).await {
            Ok(rows) => {
                rows_seen += rows.len() as i64;
                if rows.is_empty() {
                    continue;
                }
                rows_inserted += store.upsert_intraday_price_bars(&rows).await? as i64;
                symbols_with_rows.push(symbol.clone());
                for payload in latest_market_bar_payloads(symbol, &rows) {
                    let subject = format!("market.bar.{}.{}", payload.interval, symbol);
                    bus.publish(&subject, serde_json::to_string(&payload)?.as_bytes())
                        .await?;
                }
                for interval in STATUS_ONLY_INTERVALS {
                    publish_status(bus, symbol, interval, STATUS, "fmp delayed polling").await?;
                }
            }
            Err(e) => {
                symbols_failed += 1;
                let message = e.to_string();
                let status = market_status_for_error(&message);
                warn!(symbol = %symbol, status, error = %e, "fmp_live_bars symbol poll failed");
                publish_status_for_all_intervals(bus, symbol, status, &message).await?;
                if status == "entitlement_blocked" {
                    outcome.entitlement_blocked = true;
                    symbols_failed += (symbols.len() - index - 1) as i32;
                    for remaining in symbols.iter().skip(index + 1) {
                        publish_status_for_all_intervals(
                            bus,
                            remaining,
                            status,
                            "FMP intraday endpoint entitlement blocked before polling symbol",
                        )
                        .await?;
                    }
                    break;
                }
            }
        }
    }

    store
        .record_source_success(
            SOURCE,
            rows_seen,
            rows_inserted,
            symbols.len() as i32,
            symbols_failed,
        )
        .await?;
    if let Err(e) = store
        .complete_source_tasks_for_attempt(
            "fmp_live_bars",
            &symbols,
            &symbols_with_rows,
            OWNER,
            chrono::Duration::minutes(FRESH_FOR_MINUTES),
        )
        .await
    {
        warn!(error = %e, "fmp_live_bars source task completion failed");
    }
    if outcome.entitlement_blocked {
        store
            .record_source_failure(
                SOURCE,
                "entitlement_blocked",
                "FMP intraday endpoint entitlement blocked",
                None,
            )
            .await?;
    }
    info!(
        symbols = symbols.len(),
        rows_seen, rows_inserted, symbols_failed, "fmp_live_bars pass complete"
    );
    Ok(outcome)
}

async fn publish_status_for_all_intervals(
    bus: &Bus,
    symbol: &str,
    status: &str,
    reason: &str,
) -> Result<()> {
    for interval in CHART_INTERVALS
        .iter()
        .map(|(label, _)| *label)
        .chain(std::iter::once("1D"))
        .chain(STATUS_ONLY_INTERVALS.iter().copied())
    {
        publish_status(bus, symbol, interval, status, reason).await?;
    }
    Ok(())
}

async fn publish_status(
    bus: &Bus,
    symbol: &str,
    interval: &str,
    status: &str,
    reason: &str,
) -> Result<()> {
    let subject = format!("market.bar.{interval}.{symbol}");
    let payload = MarketStatusPayload {
        symbol: symbol.to_string(),
        interval: interval.to_string(),
        status: status.to_string(),
        provider: "fmp".to_string(),
        reason: reason.chars().take(240).collect(),
    };
    bus.publish(&subject, serde_json::to_string(&payload)?.as_bytes())
        .await
}

#[must_use]
pub fn latest_market_bar_payloads(
    symbol: &str,
    rows: &[IntradayPriceBarRow],
) -> Vec<MarketBarPayload> {
    if rows.is_empty() {
        return Vec::new();
    }
    let mut payloads = Vec::new();
    for (label, minutes) in CHART_INTERVALS {
        if let Some(payload) = latest_bucket_payload(symbol, label, *minutes, rows) {
            payloads.push(payload);
        }
    }
    if let Some(payload) = latest_day_payload(symbol, rows) {
        payloads.push(payload);
    }
    payloads
}

fn latest_bucket_payload(
    symbol: &str,
    interval: &str,
    bucket_minutes: i64,
    rows: &[IntradayPriceBarRow],
) -> Option<MarketBarPayload> {
    let latest = rows.iter().max_by_key(|row| row.ts)?;
    let bucket = bucket_start(latest.ts, bucket_minutes);
    let mut bucket_rows: Vec<&IntradayPriceBarRow> = rows
        .iter()
        .filter(|row| bucket_start(row.ts, bucket_minutes) == bucket)
        .collect();
    bucket_rows.sort_by_key(|row| row.ts);
    aggregate_payload(symbol, interval, bucket.to_rfc3339(), &bucket_rows)
}

fn latest_day_payload(symbol: &str, rows: &[IntradayPriceBarRow]) -> Option<MarketBarPayload> {
    let latest = rows.iter().max_by_key(|row| row.ts)?;
    let day = latest.ts.date_naive();
    let mut day_rows: Vec<&IntradayPriceBarRow> = rows
        .iter()
        .filter(|row| row.ts.date_naive() == day)
        .collect();
    day_rows.sort_by_key(|row| row.ts);
    aggregate_payload(symbol, "1D", day.format("%Y-%m-%d").to_string(), &day_rows)
}

fn aggregate_payload(
    symbol: &str,
    interval: &str,
    time: String,
    rows: &[&IntradayPriceBarRow],
) -> Option<MarketBarPayload> {
    let first = rows.first()?;
    let last = rows.last()?;
    Some(MarketBarPayload {
        symbol: symbol.to_string(),
        interval: interval.to_string(),
        time,
        open: first.open,
        high: rows
            .iter()
            .map(|row| row.high)
            .fold(f64::NEG_INFINITY, f64::max),
        low: rows.iter().map(|row| row.low).fold(f64::INFINITY, f64::min),
        close: last.close,
        volume: rows.iter().map(|row| row.volume).sum(),
        status: STATUS.to_string(),
        provider: "fmp".to_string(),
    })
}

fn bucket_start(ts: DateTime<Utc>, bucket_minutes: i64) -> DateTime<Utc> {
    let width = bucket_minutes.max(1) * 60;
    let epoch = ts.timestamp();
    let bucket_epoch = epoch.div_euclid(width) * width;
    Utc.timestamp_opt(bucket_epoch, 0).single().unwrap_or(ts)
}

#[must_use]
pub fn market_status_for_error(error: &str) -> &'static str {
    let lower = error.to_ascii_lowercase();
    if lower.contains("429") || lower.contains("rate limit") {
        "rate_limited"
    } else if lower.contains("401")
        || lower.contains("402")
        || lower.contains("403")
        || lower.contains("entitlement")
        || lower.contains("restricted endpoint")
        || lower.contains("subscription")
    {
        "entitlement_blocked"
    } else {
        "delayed"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(
        at: &str,
        open: f64,
        high: f64,
        low: f64,
        close: f64,
        volume: f64,
    ) -> IntradayPriceBarRow {
        IntradayPriceBarRow {
            symbol: "MSFT".to_string(),
            interval: NATIVE_INTERVAL.to_string(),
            ts: DateTime::parse_from_rfc3339(at)
                .unwrap()
                .with_timezone(&Utc),
            open,
            high,
            low,
            close,
            volume,
        }
    }

    #[test]
    fn latest_market_bar_payloads_aggregate_latest_bucket_and_day() {
        let rows = vec![
            row("2026-06-25T14:30:00Z", 100.0, 101.0, 99.0, 100.5, 10.0),
            row("2026-06-25T14:31:00Z", 100.5, 102.0, 100.0, 101.5, 20.0),
            row("2026-06-25T14:32:00Z", 101.5, 103.0, 101.0, 102.5, 30.0),
            row("2026-06-25T14:35:00Z", 102.5, 104.0, 102.0, 103.5, 40.0),
        ];

        let payloads = latest_market_bar_payloads("MSFT", &rows);
        let three_min = payloads
            .iter()
            .find(|payload| payload.interval == "3m")
            .unwrap();
        assert_eq!(three_min.time, "2026-06-25T14:33:00+00:00");
        assert_eq!(three_min.open, 102.5);
        assert_eq!(three_min.high, 104.0);
        assert_eq!(three_min.low, 102.0);
        assert_eq!(three_min.close, 103.5);
        assert_eq!(three_min.volume, 40.0);

        let day = payloads
            .iter()
            .find(|payload| payload.interval == "1D")
            .unwrap();
        assert_eq!(day.time, "2026-06-25");
        assert_eq!(day.open, 100.0);
        assert_eq!(day.high, 104.0);
        assert_eq!(day.low, 99.0);
        assert_eq!(day.close, 103.5);
        assert_eq!(day.volume, 100.0);
        assert_eq!(day.status, "delayed");
    }

    #[test]
    fn market_status_for_error_distinguishes_provider_failures() {
        assert_eq!(
            market_status_for_error("429 too many requests"),
            "rate_limited"
        );
        assert_eq!(
            market_status_for_error("403 subscription required"),
            "entitlement_blocked"
        );
        assert_eq!(
            market_status_for_error("402 Restricted Endpoint"),
            "entitlement_blocked"
        );
        assert_eq!(market_status_for_error("timeout"), "delayed");
    }
}
