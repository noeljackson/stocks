//! FMP analyst-estimates adapter — snapshots consensus per fiscal period and
//! emits `estimate_revision` events when it drifts (#18).
//!
//! The endpoint `/stable/analyst-estimates?symbol=&period=annual` returns one
//! row per future fiscal year with consensus EPS/revenue (avg/low/high) plus
//! `numAnalystsEps` / `numAnalystsRevenue`. We don't get a built-in revision
//! timeline; we manufacture one by snapshotting daily and diffing.
//!
//! Pure helpers in this module: `decode_response`, `diff_snapshots`. The
//! service loop lives in `fmp_estimates_service` (separate concern).

use std::time::Duration;

use anyhow::{Context, Result};
use chrono::NaiveDate;
use reqwest::Client;
use serde::Deserialize;

use super::rate_limit;

/// Decoded shape of one row in the FMP analyst-estimates response.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct EstimateRow {
    pub symbol: String,
    /// "YYYY-MM-DD" — fiscal period end.
    pub date: String,
    #[serde(default, alias = "epsAvg")]
    pub eps_avg: Option<f64>,
    #[serde(default, alias = "epsLow")]
    pub eps_low: Option<f64>,
    #[serde(default, alias = "epsHigh")]
    pub eps_high: Option<f64>,
    #[serde(default, alias = "revenueAvg")]
    pub revenue_avg: Option<f64>,
    #[serde(default, alias = "revenueLow")]
    pub revenue_low: Option<f64>,
    #[serde(default, alias = "revenueHigh")]
    pub revenue_high: Option<f64>,
    #[serde(default, alias = "numAnalystsEps")]
    pub num_analysts_eps: Option<i32>,
    #[serde(default, alias = "numAnalystsRevenue")]
    pub num_analysts_revenue: Option<i32>,
}

/// Pure: decode raw JSON to a list of estimate rows.
pub fn decode_response(json: &serde_json::Value) -> Result<Vec<EstimateRow>> {
    serde_json::from_value::<Vec<EstimateRow>>(json.clone())
        .context("decode fmp analyst-estimates response")
}

/// Normalized period info (post-parse).
#[derive(Debug, Clone, PartialEq)]
pub struct NormalizedEstimate {
    pub symbol: String,
    pub fiscal_period_end: NaiveDate,
    pub eps_avg: Option<f64>,
    pub eps_low: Option<f64>,
    pub eps_high: Option<f64>,
    pub revenue_avg: Option<f64>,
    pub revenue_low: Option<f64>,
    pub revenue_high: Option<f64>,
    pub num_analysts_eps: Option<i32>,
    pub num_analysts_revenue: Option<i32>,
}

/// Pure: parse one row's date and drop rows we can't normalize.
#[must_use]
pub fn normalize(rows: &[EstimateRow]) -> Vec<NormalizedEstimate> {
    rows.iter()
        .filter_map(|r| {
            let nd = NaiveDate::parse_from_str(&r.date, "%Y-%m-%d").ok()?;
            Some(NormalizedEstimate {
                symbol: r.symbol.clone(),
                fiscal_period_end: nd,
                eps_avg: r.eps_avg,
                eps_low: r.eps_low,
                eps_high: r.eps_high,
                revenue_avg: r.revenue_avg,
                revenue_low: r.revenue_low,
                revenue_high: r.revenue_high,
                num_analysts_eps: r.num_analysts_eps,
                num_analysts_revenue: r.num_analysts_revenue,
            })
        })
        .collect()
}

/// What a diff produces. `direction` follows the migration's CHECK constraint.
#[derive(Debug, Clone, PartialEq)]
pub struct RevisionDelta {
    pub eps_delta: Option<f64>,
    pub eps_delta_pct: Option<f64>,
    pub revenue_delta: Option<f64>,
    pub revenue_delta_pct: Option<f64>,
    pub direction: &'static str,
}

/// Pure: diff two snapshots for the same (symbol, period). Returns `None` when
/// nothing changed materially — no revision row should be written.
///
/// `prev = None` is the "first time seeing this period" case → `Some(initial)`.
#[must_use]
pub fn diff_snapshots(
    prev: Option<&NormalizedEstimate>,
    curr: &NormalizedEstimate,
) -> Option<RevisionDelta> {
    let Some(prev) = prev else {
        return Some(RevisionDelta {
            eps_delta: None,
            eps_delta_pct: None,
            revenue_delta: None,
            revenue_delta_pct: None,
            direction: "initial",
        });
    };

    let eps_delta = match (prev.eps_avg, curr.eps_avg) {
        (Some(p), Some(c)) if (c - p).abs() > 1e-9 => Some(c - p),
        _ => None,
    };
    let revenue_delta = match (prev.revenue_avg, curr.revenue_avg) {
        (Some(p), Some(c)) if (c - p).abs() > 0.5 => Some(c - p), // sub-dollar noise on $B figures
        _ => None,
    };

    let pct = |delta: Option<f64>, base: Option<f64>| -> Option<f64> {
        match (delta, base) {
            (Some(d), Some(b)) if b.abs() > 1e-9 => Some(100.0 * d / b.abs()),
            _ => None,
        }
    };
    let eps_delta_pct = pct(eps_delta, prev.eps_avg);
    let revenue_delta_pct = pct(revenue_delta, prev.revenue_avg);

    let analysts_changed = prev.num_analysts_eps != curr.num_analysts_eps
        || prev.num_analysts_revenue != curr.num_analysts_revenue;

    let direction = match (eps_delta, revenue_delta) {
        (None, None) if analysts_changed => "coverage_change",
        (None, None) => return None, // nothing changed
        (Some(e), None) => {
            if e > 0.0 {
                "up"
            } else {
                "down"
            }
        }
        (None, Some(r)) => {
            if r > 0.0 {
                "up"
            } else {
                "down"
            }
        }
        (Some(e), Some(r)) => match (e > 0.0, r > 0.0) {
            (true, true) => "up",
            (false, false) => "down",
            _ => "mixed",
        },
    };

    Some(RevisionDelta {
        eps_delta,
        eps_delta_pct,
        revenue_delta,
        revenue_delta_pct,
        direction,
    })
}

// ---------- HTTP client ----------

pub struct FmpEstimatesAdapter {
    api_key: String,
    base_url: String,
    client: Client,
}

impl FmpEstimatesAdapter {
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

    /// Fetch annual estimates for one symbol. Returns the raw JSON value so the
    /// caller can persist it verbatim in `estimate_snapshot.raw`.
    pub async fn fetch_one(&self, symbol: &str) -> Result<serde_json::Value> {
        if self.api_key.is_empty() {
            return Ok(serde_json::Value::Array(vec![]));
        }
        let url = format!(
            "{}/stable/analyst-estimates?symbol={symbol}&period=annual&limit=10&apikey={key}",
            self.base_url,
            key = self.api_key,
        );
        rate_limit::fmp().wait().await;
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .with_context(|| format!("fmp estimates fetch {symbol}"))?;
        let status = resp.status();
        let retry_after = rate_limit::retry_after(resp.headers());
        rate_limit::fmp().observe_status(status, retry_after).await;
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "fmp estimates {symbol} {}: {}",
                status.as_u16(),
                &body[..body.len().min(256)]
            );
        }
        let json: serde_json::Value = resp
            .json()
            .await
            .with_context(|| format!("fmp estimates decode {symbol}"))?;
        Ok(json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ne(period: &str, eps: Option<f64>, rev: Option<f64>, an: Option<i32>) -> NormalizedEstimate {
        NormalizedEstimate {
            symbol: "MU".into(),
            fiscal_period_end: NaiveDate::parse_from_str(period, "%Y-%m-%d").unwrap(),
            eps_avg: eps,
            eps_low: None,
            eps_high: None,
            revenue_avg: rev,
            revenue_low: None,
            revenue_high: None,
            num_analysts_eps: an,
            num_analysts_revenue: an,
        }
    }

    #[test]
    fn decode_real_fmp_shape() {
        // Verbatim sample of a single MU row from a live probe on 2026-05-31.
        let v = serde_json::json!([{
            "symbol":"MU","date":"2030-08-28",
            "revenueLow":202556390264_f64,"revenueHigh":328747059875_f64,"revenueAvg":278062000000_f64,
            "ebitdaLow":101278195132_f64,"ebitdaHigh":164373529937_f64,"ebitdaAvg":139031000000_f64,
            "ebitLow":66843608787_f64,"ebitHigh":108486529758_f64,"ebitAvg":91760460000_f64,
            "netIncomeLow":56104166299_f64,"netIncomeHigh":107263305095_f64,"netIncomeAvg":86715000077_f64,
            "sgaExpenseLow":8237030302_f64,"sgaExpenseHigh":13368620415_f64,"sgaExpenseAvg":11307493765_f64,
            "epsAvg":77.08,"epsHigh":95.34516,"epsLow":49.87037,
            "numAnalystsRevenue":20,"numAnalystsEps":7
        }]);
        let rows = decode_response(&v).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].symbol, "MU");
        assert_eq!(rows[0].eps_avg, Some(77.08));
        assert_eq!(rows[0].num_analysts_eps, Some(7));
        let n = normalize(&rows);
        assert_eq!(n[0].fiscal_period_end.to_string(), "2030-08-28");
    }

    #[test]
    fn diff_first_sighting_emits_initial() {
        let curr = ne("2026-09-30", Some(10.0), Some(50e9), Some(20));
        let d = diff_snapshots(None, &curr).unwrap();
        assert_eq!(d.direction, "initial");
        assert!(d.eps_delta.is_none() && d.revenue_delta.is_none());
    }

    #[test]
    fn diff_no_change_returns_none() {
        let prev = ne("2026-09-30", Some(10.0), Some(50e9), Some(20));
        let curr = prev.clone();
        assert!(diff_snapshots(Some(&prev), &curr).is_none());
    }

    #[test]
    fn diff_eps_up_only_marks_up() {
        let prev = ne("2026-09-30", Some(10.0), Some(50e9), Some(20));
        let curr = ne("2026-09-30", Some(10.5), Some(50e9), Some(20));
        let d = diff_snapshots(Some(&prev), &curr).unwrap();
        assert_eq!(d.direction, "up");
        assert!((d.eps_delta.unwrap() - 0.5).abs() < 1e-9);
        assert!((d.eps_delta_pct.unwrap() - 5.0).abs() < 1e-9);
        assert!(d.revenue_delta.is_none());
    }

    #[test]
    fn diff_eps_down_revenue_up_is_mixed() {
        let prev = ne("2026-09-30", Some(10.0), Some(50e9), Some(20));
        let curr = ne("2026-09-30", Some(9.0), Some(52e9), Some(20));
        let d = diff_snapshots(Some(&prev), &curr).unwrap();
        assert_eq!(d.direction, "mixed");
    }

    #[test]
    fn diff_only_analyst_count_changed_marks_coverage_change() {
        let prev = ne("2026-09-30", Some(10.0), Some(50e9), Some(20));
        let curr = ne("2026-09-30", Some(10.0), Some(50e9), Some(22));
        let d = diff_snapshots(Some(&prev), &curr).unwrap();
        assert_eq!(d.direction, "coverage_change");
    }

    #[test]
    fn diff_sub_dollar_revenue_noise_is_ignored() {
        let prev = ne("2026-09-30", Some(10.0), Some(50_000_000_000.4), Some(20));
        let curr = ne("2026-09-30", Some(10.0), Some(50_000_000_000.6), Some(20));
        // Both metrics unchanged within tolerance; analysts same → None.
        assert!(diff_snapshots(Some(&prev), &curr).is_none());
    }

    #[test]
    fn diff_handles_eps_zero_base_without_dividing_by_zero() {
        let prev = ne("2026-09-30", Some(0.0), Some(50e9), Some(20));
        let curr = ne("2026-09-30", Some(0.5), Some(50e9), Some(20));
        let d = diff_snapshots(Some(&prev), &curr).unwrap();
        assert_eq!(d.direction, "up");
        assert_eq!(d.eps_delta, Some(0.5));
        assert!(d.eps_delta_pct.is_none(), "no pct when base is zero");
    }
}
