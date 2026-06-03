//! Service loop for FMP company profile + earnings calendar (#260).

use std::{collections::BTreeSet, time::Duration};

use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use sqlx::postgres::PgPool;
use tracing::{info, warn};

use super::fmp_profile_calendar::{
    FmpProfileCalendarAdapter, NormalizedCompanyProfile, NormalizedEarningsEvent, decode_earnings,
    decode_profile, normalize_earnings, normalize_profile,
};
use super::{max_symbols_from_env, rate_limit, source_health};
use crate::platform::store::Store;

const SOURCE_NAME: &str = "fmp_profile_calendar";
const PROFILE_ACTION: &str = "fmp_company_profile";
const EARNINGS_ACTION: &str = "fmp_earnings_calendar";
const ACTIONS: [&str; 2] = [PROFILE_ACTION, EARNINGS_ACTION];

#[derive(Debug, Default)]
pub struct ProfileCalendarScanStats {
    profile_rows: usize,
    earnings_rows: usize,
    upserted_rows: usize,
    profile_error: Option<String>,
    earnings_error: Option<String>,
}

impl ProfileCalendarScanStats {
    fn rows_seen(&self) -> usize {
        self.profile_rows + self.earnings_rows
    }

    fn has_failure(&self) -> bool {
        self.profile_error.is_some() || self.earnings_error.is_some()
    }
}

#[derive(Debug, Default)]
struct ActionCoverage {
    profile_symbols: BTreeSet<String>,
    earnings_symbols: BTreeSet<String>,
}

impl ActionCoverage {
    fn record(&mut self, symbol: &str, stats: &ProfileCalendarScanStats) {
        if stats.profile_rows > 0 {
            self.profile_symbols.insert(symbol.to_string());
        }
        if stats.earnings_rows > 0 {
            self.earnings_symbols.insert(symbol.to_string());
        }
    }

    fn symbols_for_action(&self, action: &str) -> Vec<String> {
        let symbols = match action {
            PROFILE_ACTION => &self.profile_symbols,
            EARNINGS_ACTION => &self.earnings_symbols,
            _ => return Vec::new(),
        };
        symbols.iter().cloned().collect()
    }
}

pub async fn run_once(pool: &PgPool, adapter: &FmpProfileCalendarAdapter) -> Result<usize> {
    let store = Store { pool: pool.clone() };
    let max_symbols = max_symbols_from_env("FMP_PROFILE_CALENDAR_MAX_SYMBOLS_PER_PASS", 75);
    let earnings_limit = max_symbols_from_env("FMP_EARNINGS_LIMIT_PER_SYMBOL", 10) as usize;
    let symbols = store
        .priority_scan_symbols(max_symbols)
        .await
        .unwrap_or_default();

    source_health::mark_started(pool, SOURCE_NAME, symbols.len() as i32).await?;
    store
        .mark_source_tasks_fetching(&ACTIONS, &symbols, "ingest.fmp_profile_calendar")
        .await?;

    let mut rows_seen = 0usize;
    let mut rows_upserted = 0usize;
    let mut symbols_failed = 0i32;
    let mut saw_rate_limit = false;
    let mut coverage = ActionCoverage::default();
    let mut failed_profile_symbols = BTreeSet::new();
    let mut failed_earnings_symbols = BTreeSet::new();
    let mut rate_limited_profile_symbols = BTreeSet::new();
    let mut rate_limited_earnings_symbols = BTreeSet::new();

    for symbol in &symbols {
        match scan_one(pool, adapter, symbol, earnings_limit).await {
            Ok(stats) => {
                rows_seen += stats.rows_seen();
                rows_upserted += stats.upserted_rows;
                coverage.record(symbol, &stats);
                if stats.has_failure() {
                    symbols_failed += 1;
                }
                if let Some(error) = &stats.profile_error {
                    failed_profile_symbols.insert(symbol.clone());
                    if source_health::failure_kind(error) == "rate_limited" {
                        saw_rate_limit = true;
                        rate_limited_profile_symbols.insert(symbol.clone());
                    }
                    warn!(
                        symbol = %symbol,
                        error = %error,
                        "fmp company profile scan failed"
                    );
                }
                if let Some(error) = &stats.earnings_error {
                    failed_earnings_symbols.insert(symbol.clone());
                    if source_health::failure_kind(error) == "rate_limited" {
                        saw_rate_limit = true;
                        rate_limited_earnings_symbols.insert(symbol.clone());
                    }
                    warn!(
                        symbol = %symbol,
                        error = %error,
                        "fmp earnings calendar scan failed"
                    );
                }
            }
            Err(e) => {
                symbols_failed += 1;
                failed_profile_symbols.insert(symbol.clone());
                failed_earnings_symbols.insert(symbol.clone());
                if source_health::failure_kind(&e.to_string()) == "rate_limited" {
                    saw_rate_limit = true;
                    rate_limited_profile_symbols.insert(symbol.clone());
                    rate_limited_earnings_symbols.insert(symbol.clone());
                }
                warn!(
                    symbol = %symbol,
                    error = %format!("{:#}", e),
                    "fmp profile/calendar scan_one failed"
                );
            }
        }
    }

    source_health::record_success(
        pool,
        SOURCE_NAME,
        rows_seen as i64,
        rows_upserted as i64,
        symbols.len() as i32,
        symbols_failed,
    )
    .await?;

    let successful_profile_symbols: Vec<String> = symbols
        .iter()
        .filter(|s| !failed_profile_symbols.contains(*s))
        .cloned()
        .collect();
    let successful_earnings_symbols: Vec<String> = symbols
        .iter()
        .filter(|s| !failed_earnings_symbols.contains(*s))
        .cloned()
        .collect();
    store
        .complete_source_tasks_for_attempt(
            PROFILE_ACTION,
            &successful_profile_symbols,
            &coverage.symbols_for_action(PROFILE_ACTION),
            "ingest.fmp_profile_calendar",
            chrono::Duration::minutes(30),
        )
        .await?;
    store
        .complete_source_tasks_for_attempt(
            EARNINGS_ACTION,
            &successful_earnings_symbols,
            &coverage.symbols_for_action(EARNINGS_ACTION),
            "ingest.fmp_profile_calendar",
            chrono::Duration::minutes(30),
        )
        .await?;

    let profile_non_rate_limited_failures: Vec<String> = failed_profile_symbols
        .difference(&rate_limited_profile_symbols)
        .cloned()
        .collect();
    if !profile_non_rate_limited_failures.is_empty() {
        store
            .fail_source_tasks_for_attempt(
                PROFILE_ACTION,
                &profile_non_rate_limited_failures,
                "ingest.fmp_profile_calendar",
                "failed",
                "one or more FMP company profile requests failed",
                None,
            )
            .await?;
    }
    let earnings_non_rate_limited_failures: Vec<String> = failed_earnings_symbols
        .difference(&rate_limited_earnings_symbols)
        .cloned()
        .collect();
    if !earnings_non_rate_limited_failures.is_empty() {
        store
            .fail_source_tasks_for_attempt(
                EARNINGS_ACTION,
                &earnings_non_rate_limited_failures,
                "ingest.fmp_profile_calendar",
                "failed",
                "one or more FMP earnings calendar requests failed",
                None,
            )
            .await?;
    }

    if saw_rate_limit {
        let retry_after_at = rate_limit::fmp().retry_after_at().await;
        source_health::record_failure(
            pool,
            SOURCE_NAME,
            "rate_limited",
            "one or more FMP profile/calendar requests were rate limited",
            retry_after_at,
        )
        .await?;
        let rate_limited_profile_symbols: Vec<String> =
            rate_limited_profile_symbols.into_iter().collect();
        if !rate_limited_profile_symbols.is_empty() {
            store
                .fail_source_tasks_for_attempt(
                    PROFILE_ACTION,
                    &rate_limited_profile_symbols,
                    "ingest.fmp_profile_calendar",
                    "rate_limited",
                    "one or more FMP company profile requests were rate limited",
                    retry_after_at,
                )
                .await?;
        }
        let rate_limited_earnings_symbols: Vec<String> =
            rate_limited_earnings_symbols.into_iter().collect();
        if !rate_limited_earnings_symbols.is_empty() {
            store
                .fail_source_tasks_for_attempt(
                    EARNINGS_ACTION,
                    &rate_limited_earnings_symbols,
                    "ingest.fmp_profile_calendar",
                    "rate_limited",
                    "one or more FMP earnings calendar requests were rate limited",
                    retry_after_at,
                )
                .await?;
        }
    }

    Ok(rows_upserted)
}

async fn scan_one(
    pool: &PgPool,
    adapter: &FmpProfileCalendarAdapter,
    symbol: &str,
    earnings_limit: usize,
) -> Result<ProfileCalendarScanStats> {
    let mut stats = ProfileCalendarScanStats::default();

    match scan_profile(pool, adapter, symbol).await {
        Ok((rows_seen, rows_upserted)) => {
            stats.profile_rows = rows_seen;
            stats.upserted_rows += rows_upserted;
        }
        Err(e) => stats.profile_error = Some(format!("{:#}", e)),
    }

    match scan_earnings(pool, adapter, symbol, earnings_limit).await {
        Ok((rows_seen, rows_upserted)) => {
            stats.earnings_rows = rows_seen;
            stats.upserted_rows += rows_upserted;
        }
        Err(e) => stats.earnings_error = Some(format!("{:#}", e)),
    }

    Ok(stats)
}

async fn scan_profile(
    pool: &PgPool,
    adapter: &FmpProfileCalendarAdapter,
    symbol: &str,
) -> Result<(usize, usize)> {
    let raw = adapter.fetch_profile(symbol).await?;
    let profile_rows = decode_profile(&raw)?;
    let Some(profile_row) = profile_rows.first() else {
        return Ok((0, 0));
    };
    let profile = normalize_profile(profile_row);
    let raw_row = raw.get(0).cloned().unwrap_or(serde_json::Value::Null);
    upsert_company_profile(pool, &profile, &raw_row).await?;
    Ok((profile_rows.len(), 1))
}

async fn scan_earnings(
    pool: &PgPool,
    adapter: &FmpProfileCalendarAdapter,
    symbol: &str,
    earnings_limit: usize,
) -> Result<(usize, usize)> {
    let raw = adapter.fetch_earnings(symbol, earnings_limit).await?;
    let earnings_rows = decode_earnings(&raw)?;
    let normalized_earnings = normalize_earnings(&earnings_rows);
    let mut upserted = 0;
    for (i, event) in normalized_earnings.iter().enumerate() {
        let raw_row = raw.get(i).cloned().unwrap_or(serde_json::Value::Null);
        upsert_earnings_event(pool, event, &raw_row).await?;
        upsert_earnings_evidence_item(pool, event).await?;
        upserted += 1;
    }
    Ok((normalized_earnings.len(), upserted))
}

async fn upsert_company_profile(
    pool: &PgPool,
    row: &NormalizedCompanyProfile,
    raw: &serde_json::Value,
) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO company_profile
             (symbol, company_name, currency, market_cap, beta, exchange,
              exchange_full_name, industry, sector, country, website, description,
              ceo, full_time_employees, ipo_date, is_etf, is_adr, is_fund,
              is_actively_trading, raw)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                   $11, $12, $13, $14, $15, $16, $17, $18, $19, $20::jsonb)
           ON CONFLICT (symbol) DO UPDATE SET
              company_name = EXCLUDED.company_name,
              currency = EXCLUDED.currency,
              market_cap = EXCLUDED.market_cap,
              beta = EXCLUDED.beta,
              exchange = EXCLUDED.exchange,
              exchange_full_name = EXCLUDED.exchange_full_name,
              industry = EXCLUDED.industry,
              sector = EXCLUDED.sector,
              country = EXCLUDED.country,
              website = EXCLUDED.website,
              description = EXCLUDED.description,
              ceo = EXCLUDED.ceo,
              full_time_employees = EXCLUDED.full_time_employees,
              ipo_date = EXCLUDED.ipo_date,
              is_etf = EXCLUDED.is_etf,
              is_adr = EXCLUDED.is_adr,
              is_fund = EXCLUDED.is_fund,
              is_actively_trading = EXCLUDED.is_actively_trading,
              raw = EXCLUDED.raw,
              profile_at = now(),
              updated_at = CASE
                  WHEN company_profile.raw IS DISTINCT FROM EXCLUDED.raw THEN now()
                  ELSE company_profile.updated_at
              END"#,
    )
    .bind(&row.symbol)
    .bind(&row.company_name)
    .bind(&row.currency)
    .bind(row.market_cap)
    .bind(row.beta)
    .bind(&row.exchange)
    .bind(&row.exchange_full_name)
    .bind(&row.industry)
    .bind(&row.sector)
    .bind(&row.country)
    .bind(&row.website)
    .bind(&row.description)
    .bind(&row.ceo)
    .bind(row.full_time_employees)
    .bind(row.ipo_date)
    .bind(row.is_etf)
    .bind(row.is_adr)
    .bind(row.is_fund)
    .bind(row.is_actively_trading)
    .bind(raw)
    .execute(pool)
    .await
    .context("upsert company_profile")?;
    Ok(())
}

async fn upsert_earnings_event(
    pool: &PgPool,
    row: &NormalizedEarningsEvent,
    raw: &serde_json::Value,
) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO earnings_calendar_event
             (symbol, report_date, eps_actual, eps_estimated, revenue_actual,
              revenue_estimated, last_updated, raw)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8::jsonb)
           ON CONFLICT (symbol, report_date) DO UPDATE SET
              eps_actual = EXCLUDED.eps_actual,
              eps_estimated = EXCLUDED.eps_estimated,
              revenue_actual = EXCLUDED.revenue_actual,
              revenue_estimated = EXCLUDED.revenue_estimated,
              last_updated = EXCLUDED.last_updated,
              raw = EXCLUDED.raw,
              updated_at = CASE
                  WHEN earnings_calendar_event.raw IS DISTINCT FROM EXCLUDED.raw THEN now()
                  ELSE earnings_calendar_event.updated_at
              END"#,
    )
    .bind(&row.symbol)
    .bind(row.report_date)
    .bind(row.eps_actual)
    .bind(row.eps_estimated)
    .bind(row.revenue_actual)
    .bind(row.revenue_estimated)
    .bind(row.last_updated)
    .bind(raw)
    .execute(pool)
    .await
    .context("upsert earnings_calendar_event")?;
    Ok(())
}

async fn upsert_earnings_evidence_item(pool: &PgPool, row: &NormalizedEarningsEvent) -> Result<()> {
    let observed_at = date_at_utc(row.report_date);
    let summary = earnings_summary(row);
    sqlx::query(
        r#"INSERT INTO evidence_item
             (symbol, kind, observed_at, source, source_id, source_ref,
              summary, strength, polarity, updated_at)
           VALUES ($1, 'earnings_calendar', $2, $3, $4, $5::jsonb,
                   $6, 0.55, NULL, now())
           ON CONFLICT (source, source_id) DO UPDATE SET
              observed_at = EXCLUDED.observed_at,
              source_ref = evidence_item.source_ref || EXCLUDED.source_ref,
              summary = EXCLUDED.summary,
              strength = EXCLUDED.strength,
              polarity = EXCLUDED.polarity,
              updated_at = now()"#,
    )
    .bind(&row.symbol)
    .bind(observed_at)
    .bind(SOURCE_NAME)
    .bind(format!(
        "earnings_calendar:{}:{}",
        row.symbol, row.report_date
    ))
    .bind(serde_json::json!({
        "table": "earnings_calendar_event",
        "symbol": row.symbol,
        "report_date": row.report_date,
        "last_updated": row.last_updated,
        "eps_actual": row.eps_actual,
        "eps_estimated": row.eps_estimated,
        "revenue_actual": row.revenue_actual,
        "revenue_estimated": row.revenue_estimated,
    }))
    .bind(summary)
    .execute(pool)
    .await
    .context("upsert earnings_calendar evidence_item")?;
    Ok(())
}

fn date_at_utc(date: NaiveDate) -> DateTime<Utc> {
    Utc.from_utc_datetime(&date.and_hms_opt(0, 0, 0).expect("valid midnight"))
}

fn earnings_summary(row: &NormalizedEarningsEvent) -> String {
    let phase = if row.eps_actual.is_some() || row.revenue_actual.is_some() {
        "reported"
    } else {
        "scheduled"
    };
    let mut parts = vec![format!(
        "{} earnings {phase} {}",
        row.symbol, row.report_date
    )];
    if let Some(v) = row.eps_estimated {
        parts.push(format!("EPS est {v:.2}"));
    }
    if let Some(v) = row.eps_actual {
        parts.push(format!("EPS actual {v:.2}"));
    }
    if let Some(v) = row.revenue_estimated {
        parts.push(format!("revenue est {}", compact_usd(v)));
    }
    if let Some(v) = row.revenue_actual {
        parts.push(format!("revenue actual {}", compact_usd(v)));
    }
    match parts.split_first() {
        Some((head, tail)) if !tail.is_empty() => format!("{head}: {}", tail.join("; ")),
        Some((head, _)) => head.to_string(),
        None => String::new(),
    }
}

fn compact_usd(value: f64) -> String {
    let abs = value.abs();
    if abs >= 1_000_000_000_000.0 {
        format!("${:.2}T", value / 1_000_000_000_000.0)
    } else if abs >= 1_000_000_000.0 {
        format!("${:.2}B", value / 1_000_000_000.0)
    } else if abs >= 1_000_000.0 {
        format!("${:.2}M", value / 1_000_000.0)
    } else {
        format!("${value:.0}")
    }
}

pub async fn run(
    pool: PgPool,
    adapter: FmpProfileCalendarAdapter,
    interval: Duration,
) -> Result<()> {
    info!(
        interval_secs = interval.as_secs(),
        "fmp_profile_calendar service started"
    );
    loop {
        let upserted = run_once(&pool, &adapter).await?;
        info!(upserted, "fmp profile/calendar pass complete");
        tokio::time::sleep(interval).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn earnings_summary_names_scheduled_estimates() {
        let row = NormalizedEarningsEvent {
            symbol: "NVDA".into(),
            report_date: NaiveDate::from_ymd_opt(2026, 8, 26).unwrap(),
            eps_actual: None,
            eps_estimated: Some(2.07),
            revenue_actual: None,
            revenue_estimated: Some(91_572_710_000.0),
            last_updated: None,
        };

        assert_eq!(
            earnings_summary(&row),
            "NVDA earnings scheduled 2026-08-26: EPS est 2.07; revenue est $91.57B"
        );
    }

    #[test]
    fn earnings_summary_names_reported_actuals() {
        let row = NormalizedEarningsEvent {
            symbol: "NVDA".into(),
            report_date: NaiveDate::from_ymd_opt(2026, 5, 20).unwrap(),
            eps_actual: Some(1.87),
            eps_estimated: Some(1.76),
            revenue_actual: Some(81_615_000_000.0),
            revenue_estimated: Some(78_423_370_000.0),
            last_updated: None,
        };

        assert!(earnings_summary(&row).contains("reported 2026-05-20"));
        assert!(earnings_summary(&row).contains("EPS actual 1.87"));
        assert!(earnings_summary(&row).contains("revenue actual $81.61B"));
    }
}
