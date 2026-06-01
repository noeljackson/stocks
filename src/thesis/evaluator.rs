//! Condition evaluator (#14) — pure-function metric resolution + comparison.
//!
//! Walks every pending or inconclusive condition, resolves its `target.metric` against
//! `company_fact` / `price_bar`, compares observed value vs target.value
//! per target.op, and updates condition status.
//!
//! Conservative semantics:
//! - observed satisfies the target → status = `satisfied` (terminal)
//! - observed violates AND we're past deadline → status = `refuted`
//! - observed violates BUT still within deadline → leave `pending`
//!   (the world could still come to the thesis's view)
//! - no observation for the metric → status = `inconclusive` (skipped this
//!   pass; will try again next tick)

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Row, postgres::PgPool};
use tracing::{info, warn};

/// Parsed target from the condition's JSONB.
#[derive(Debug, Clone, Deserialize)]
pub struct Target {
    pub metric: String,
    /// `>` | `>=` | `<` | `<=` | `==` | `!=`
    pub op: String,
    pub value: f64,
    #[serde(default)]
    pub unit: Option<String>,
}

/// Outcome of one evaluation pass on one condition. Pure data.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Evaluation {
    pub status: ConditionStatus,
    pub observed: Option<f64>,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConditionStatus {
    Pending,
    Satisfied,
    Refuted,
    Inconclusive,
    Stale,
}

impl ConditionStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Satisfied => "satisfied",
            Self::Refuted => "refuted",
            Self::Inconclusive => "inconclusive",
            Self::Stale => "stale",
        }
    }

    fn from_db(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "satisfied" => Some(Self::Satisfied),
            "refuted" => Some(Self::Refuted),
            "inconclusive" => Some(Self::Inconclusive),
            "stale" => Some(Self::Stale),
            _ => None,
        }
    }

    fn should_evaluate(self) -> bool {
        matches!(self, Self::Pending | Self::Inconclusive)
    }
}

/// Compare `observed` against `target` per `op`. Returns true if the target
/// condition is satisfied (i.e. the prediction came true).
#[must_use]
pub fn satisfies(observed: f64, op: &str, target: f64) -> bool {
    match op {
        ">" => observed > target,
        ">=" => observed >= target,
        "<" => observed < target,
        "<=" => observed <= target,
        "==" => (observed - target).abs() < f64::EPSILON,
        "!=" => (observed - target).abs() >= f64::EPSILON,
        _ => false,
    }
}

/// Pure evaluation logic (no DB). Caller resolves `observed` ahead of time.
#[must_use]
pub fn evaluate(
    target: &Target,
    observed: Option<f64>,
    deadline_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Evaluation {
    let Some(observed) = observed else {
        return Evaluation {
            status: ConditionStatus::Inconclusive,
            observed: None,
            reason: format!("no observation for metric {}", target.metric),
        };
    };
    if satisfies(observed, &target.op, target.value) {
        return Evaluation {
            status: ConditionStatus::Satisfied,
            observed: Some(observed),
            reason: format!(
                "{} {} {} satisfied (observed: {})",
                target.metric, target.op, target.value, observed
            ),
        };
    }
    // Target not met. Conservative: leave pending while deadline hasn't passed.
    if let Some(deadline) = deadline_at {
        if now > deadline {
            return Evaluation {
                status: ConditionStatus::Refuted,
                observed: Some(observed),
                reason: format!(
                    "deadline passed; {} {} {} not met (observed: {})",
                    target.metric, target.op, target.value, observed
                ),
            };
        }
    }
    Evaluation {
        status: ConditionStatus::Pending,
        observed: Some(observed),
        reason: format!(
            "{} = {} (target {} {}, deadline not yet passed)",
            target.metric, observed, target.op, target.value
        ),
    }
}

// ---------- metric resolution ----------

/// Resolve a metric like `"NVDA.Revenues"` or `"NVDA.close"` to a single
/// numeric value. Returns None when the metric isn't resolvable (unknown
/// pattern, no data yet, etc).
///
/// Supported patterns:
/// - `<SYMBOL>.<us-gaap-concept>` → latest company_fact.value for that
///   (symbol, concept), e.g. `NVDA.Revenues`, `MU.GrossProfit`
/// - `<SYMBOL>.close` → latest price_bar.close for the symbol
/// - `<SYMBOL>.gross_margin_pct` → computed: GrossProfit / Revenues × 100
/// - `<SYMBOL>.operating_margin_pct` → OperatingIncomeLoss / Revenues × 100
/// - `<SYMBOL>.net_margin_pct` → NetIncomeLoss / Revenues × 100
/// - **`<SYMBOL>.Q<n>_FY<year>_<base>`** → period-qualified, looks up the
///   matching quarter within the company's fiscal year (#62 / #18)
/// - **`<SYMBOL>.FY<year>_<base>`** → annual full-year value
///
/// Period-qualified resolution returns None (→ inconclusive) when:
/// - the period hasn't been reported yet (no matching company_fact row)
/// - the company hasn't reported enough quarters for the requested Q<n>
pub async fn resolve_metric(pool: &PgPool, metric: &str) -> Result<Option<f64>> {
    let Some((symbol, suffix)) = metric.split_once('.') else {
        return Ok(None);
    };
    let symbol = symbol.to_ascii_uppercase();

    // Period-qualified pattern first — strip Q/FY tag and dispatch.
    if let Some((period, base)) = parse_period_prefix(suffix) {
        return resolve_period_qualified(pool, &symbol, period, base).await;
    }

    match suffix {
        "close" => latest_close(pool, &symbol).await,
        "gross_margin_pct" => {
            ratio(pool, &symbol, "GrossProfit", "Revenues").await.map(|v| v.map(|r| r * 100.0))
        }
        "operating_margin_pct" => {
            ratio(pool, &symbol, "OperatingIncomeLoss", "Revenues")
                .await
                .map(|v| v.map(|r| r * 100.0))
        }
        "net_margin_pct" => {
            ratio(pool, &symbol, "NetIncomeLoss", "Revenues").await.map(|v| v.map(|r| r * 100.0))
        }
        concept => latest_fact(pool, &symbol, concept).await,
    }
}

/// Identifies a specific fiscal period within a company's XBRL filings.
/// `quarter = None` means annual (FY-only).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PeriodTag {
    pub fiscal_year: i32,
    pub quarter: Option<u8>,
}

/// Parse a metric suffix's period prefix.
///
/// - `"Q3_FY2026_revenue"`  → Some((PeriodTag{2026, Some(3)}, "revenue"))
/// - `"FY2026_revenue"`     → Some((PeriodTag{2026, None}, "revenue"))
/// - `"Q3_2026_revenue"`    → Some((PeriodTag{2026, Some(3)}, "revenue"))
/// - `"revenue"`            → None (current-period — caller falls back to latest_fact)
pub fn parse_period_prefix(suffix: &str) -> Option<(PeriodTag, &str)> {
    let mut chars = suffix.chars();
    let first = chars.next()?;

    // Pattern: "Q<n>_FY<year>_..." or "Q<n>_<year>_..."
    if first == 'Q' {
        let q_str: String = chars.by_ref().take_while(|c| c.is_ascii_digit()).collect();
        let q: u8 = q_str.parse().ok()?;
        if !(1..=4).contains(&q) {
            return None;
        }
        // After "Q<n>" we should be at "_FY..." or "_<year>...".
        let rest = suffix.strip_prefix(&format!("Q{q}_"))?;
        let (year, base) = parse_year_prefix(rest)?;
        return Some((PeriodTag { fiscal_year: year, quarter: Some(q) }, base));
    }

    // Pattern: "FY<year>_..."
    if suffix.starts_with("FY") {
        let (year, base) = parse_year_prefix(suffix)?;
        return Some((PeriodTag { fiscal_year: year, quarter: None }, base));
    }
    None
}

/// `"FY2026_revenue"` → Some((2026, "revenue"))
/// `"2026_revenue"`   → Some((2026, "revenue"))
fn parse_year_prefix(s: &str) -> Option<(i32, &str)> {
    let s = s.strip_prefix("FY").unwrap_or(s);
    let mut chars = s.chars();
    let year_str: String = (&mut chars).take_while(|c| c.is_ascii_digit()).collect();
    let year: i32 = year_str.parse().ok()?;
    let base = s.strip_prefix(&format!("{year}_"))?;
    Some((year, base))
}

async fn resolve_period_qualified(
    pool: &PgPool,
    symbol: &str,
    period: PeriodTag,
    base: &str,
) -> Result<Option<f64>> {
    // Map the metric base to one or more XBRL concepts.
    let resolved = match base {
        "revenue" | "revenues" => period_fact(pool, symbol, "Revenues", period).await?,
        "gross_profit" => period_fact(pool, symbol, "GrossProfit", period).await?,
        "net_income" => period_fact(pool, symbol, "NetIncomeLoss", period).await?,
        "operating_income" => period_fact(pool, symbol, "OperatingIncomeLoss", period).await?,
        "gross_margin_pct" => period_ratio(pool, symbol, "GrossProfit", "Revenues", period).await?
            .map(|r| r * 100.0),
        "operating_margin_pct" => {
            period_ratio(pool, symbol, "OperatingIncomeLoss", "Revenues", period).await?
                .map(|r| r * 100.0)
        }
        "net_margin_pct" => period_ratio(pool, symbol, "NetIncomeLoss", "Revenues", period).await?
            .map(|r| r * 100.0),
        // Pass-through: if the LLM wrote a literal concept name like
        // "Q3_FY2026_Revenues", honor that too.
        other => period_fact(pool, symbol, other, period).await?,
    };
    Ok(resolved)
}

/// Look up the company_fact row for a (symbol, concept, fiscal_year, quarter).
///
/// XBRL doesn't always tag fiscal_period as "Q1"/"Q2"/etc — many filings only
/// set fiscal_period='FY'. We infer the quarter by sorting rows for the
/// fiscal year by period_end and picking the nth ~quarterly row (duration
/// ≈ 90 days). The 4th sequential entry is typically the full-year row
/// (~365d), which we filter out when looking for Qn (n<=4).
async fn period_fact(
    pool: &PgPool,
    symbol: &str,
    concept: &str,
    period: PeriodTag,
) -> Result<Option<f64>> {
    let rows = sqlx::query(
        r#"SELECT value::float8 AS v, period_start, period_end
             FROM company_fact
            WHERE symbol = $1 AND concept = $2 AND fiscal_year = $3
         ORDER BY period_end ASC, filed_at ASC"#,
    )
    .bind(symbol)
    .bind(concept)
    .bind(period.fiscal_year)
    .fetch_all(pool)
    .await
    .with_context(|| format!("period_fact {symbol}.{concept} FY{}", period.fiscal_year))?;
    if rows.is_empty() {
        return Ok(None);
    }
    // Build (value, duration_days). Use try_get for both date fields and
    // skip rows that don't decode cleanly rather than panicking — XBRL
    // can have surprising shapes and we'd rather lose a row than the whole pass.
    let durations: Vec<(f64, i64)> = rows
        .iter()
        .filter_map(|r| {
            let v: f64 = r.try_get("v").ok()?;
            let start: Option<chrono::NaiveDate> = r.try_get("period_start").ok().flatten();
            let end: chrono::NaiveDate = r.try_get("period_end").ok()?;
            let dur = start.map_or(0, |s| (end - s).num_days());
            Some((v, dur))
        })
        .collect();

    let resolved = match period.quarter {
        None => {
            // Annual: find the LAST ~365-day row (most recent annual; the
            // first one is often the comparative prior-year figure).
            durations
                .iter()
                .rev()
                .find(|(_, d)| *d >= 330 && *d <= 400)
                .map(|(v, _)| *v)
        }
        Some(q) => {
            // Quarterly: filter to ~90-day rows in ASC order, pick the
            // FOURTH-from-end's nth predecessor — i.e. take the LAST 4
            // quarterly rows and index Q1..Q4 from there. This handles the
            // common case where XBRL bundles the prior year's Q4 in the
            // same fiscal_year tag.
            let quarters: Vec<f64> = durations
                .iter()
                .filter(|(_, d)| *d >= 60 && *d <= 110)
                .map(|(v, _)| *v)
                .collect();
            // Take the LAST up-to-4 quarters of this FY (most chronologically recent).
            let n = quarters.len();
            let start_idx = n.saturating_sub(4);
            let last_four = &quarters[start_idx..];
            last_four.get((q as usize).saturating_sub(1)).copied()
        }
    };
    Ok(resolved)
}

async fn period_ratio(
    pool: &PgPool,
    symbol: &str,
    numerator: &str,
    denominator: &str,
    period: PeriodTag,
) -> Result<Option<f64>> {
    let n = period_fact(pool, symbol, numerator, period).await?;
    let d = period_fact(pool, symbol, denominator, period).await?;
    Ok(match (n, d) {
        (Some(n), Some(d)) if d != 0.0 => Some(n / d),
        _ => None,
    })
}

async fn latest_fact(pool: &PgPool, symbol: &str, concept: &str) -> Result<Option<f64>> {
    let row = sqlx::query(
        r#"SELECT value::float8 AS v FROM company_fact
            WHERE symbol = $1 AND concept = $2
         ORDER BY period_end DESC, filed_at DESC LIMIT 1"#,
    )
    .bind(symbol)
    .bind(concept)
    .fetch_optional(pool)
    .await
    .with_context(|| format!("latest_fact {symbol}.{concept}"))?;
    Ok(row.map(|r| r.get::<f64, _>("v")))
}

async fn ratio(
    pool: &PgPool,
    symbol: &str,
    numerator: &str,
    denominator: &str,
) -> Result<Option<f64>> {
    let n = latest_fact(pool, symbol, numerator).await?;
    let d = latest_fact(pool, symbol, denominator).await?;
    Ok(match (n, d) {
        (Some(n), Some(d)) if d != 0.0 => Some(n / d),
        _ => None,
    })
}

async fn latest_close(pool: &PgPool, symbol: &str) -> Result<Option<f64>> {
    let row = sqlx::query(
        r#"SELECT close::float8 AS v FROM price_bar
            WHERE symbol = $1 ORDER BY ts DESC LIMIT 1"#,
    )
    .bind(symbol)
    .fetch_optional(pool)
    .await
    .with_context(|| format!("latest_close {symbol}"))?;
    Ok(row.map(|r| r.get::<f64, _>("v")))
}

// ---------- service loop ----------

/// One pass: evaluate every pending or inconclusive condition with a parseable
/// target, update statuses in the DB. Returns counts per outcome.
pub async fn run_once(pool: &PgPool) -> Result<EvalCounts> {
    let now = Utc::now();
    let rows = sqlx::query(
        r#"SELECT thesis_id, role, position, COALESCE(name, '') AS name,
                  status, target, deadline_at
             FROM v_condition
            WHERE target IS NOT NULL"#,
    )
    .fetch_all(pool)
    .await
    .context("v_condition fetch")?;

    let mut counts = EvalCounts::default();
    for row in rows {
        let thesis_id: uuid::Uuid = row.try_get("thesis_id")?;
        let role: String = row.try_get("role")?;
        let position: i64 = row.try_get("position")?;
        let name: String = row.try_get("name")?;
        let status: String = row.try_get("status")?;
        let target_json: serde_json::Value = row.try_get("target")?;
        let deadline_at: Option<DateTime<Utc>> = row.try_get("deadline_at")?;

        let Some(current_status) = ConditionStatus::from_db(&status) else {
            counts.skipped += 1;
            continue;
        };
        if !current_status.should_evaluate() {
            continue;
        }

        let Ok(target) = serde_json::from_value::<Target>(target_json) else {
            counts.skipped += 1;
            continue;
        };
        let observed = match resolve_metric(pool, &target.metric).await {
            Ok(v) => v,
            Err(e) => {
                warn!(metric = %target.metric, error = %e, "metric resolution failed");
                counts.skipped += 1;
                continue;
            }
        };
        let eval = evaluate(&target, observed, deadline_at, now);
        match eval.status {
            ConditionStatus::Pending => {
                counts.still_pending += 1;
                continue; // no DB update needed
            }
            ConditionStatus::Satisfied => counts.satisfied += 1,
            ConditionStatus::Refuted => counts.refuted += 1,
            ConditionStatus::Inconclusive => counts.inconclusive += 1,
            ConditionStatus::Stale => counts.stale += 1,
        }

        if let Err(e) =
            update_condition_status(pool, thesis_id, &role, position, eval.status, eval.observed)
                .await
        {
            warn!(error = %e, "update_condition_status failed");
        } else {
            info!(thesis = %thesis_id, role = %role, name = %name,
                  status = %eval.status.as_str(), reason = %eval.reason,
                  "condition evaluated");
        }
    }
    Ok(counts)
}

#[derive(Debug, Default, Clone, Copy)]
pub struct EvalCounts {
    pub satisfied: usize,
    pub refuted: usize,
    pub inconclusive: usize,
    pub stale: usize,
    pub still_pending: usize,
    pub skipped: usize,
}

async fn update_condition_status(
    pool: &PgPool,
    thesis_id: uuid::Uuid,
    role: &str,
    position: i64,
    status: ConditionStatus,
    observed: Option<f64>,
) -> Result<()> {
    let idx = (position - 1).max(0).to_string();
    let observed_json = match observed {
        Some(v) => serde_json::Value::Number(serde_json::Number::from_f64(v).unwrap_or_else(|| 0.into())),
        None => serde_json::Value::Null,
    };
    let now_str = Utc::now().to_rfc3339();
    // Reuse per-role static SQL (same pattern as staleness.rs).
    let sql = match role {
        "conviction" => SQL_EVAL_CONVICTION,
        "trigger" => SQL_EVAL_TRIGGER,
        "invalidation" => SQL_EVAL_INVALIDATION,
        "fulfillment" => SQL_EVAL_FULFILLMENT,
        _ => return Ok(()),
    };
    sqlx::query(sql)
        .bind(&idx)
        .bind(serde_json::Value::String(status.as_str().to_string()))
        .bind(&observed_json)
        .bind(serde_json::Value::String(now_str))
        .bind(thesis_id)
        .execute(pool)
        .await
        .with_context(|| format!("eval update {thesis_id} {role} pos={position}"))?;
    Ok(())
}

// create_if_missing = true on `status` too — LLM-drafted conditions don't
// have a `status` key until the evaluator writes one. Earlier this was
// false, which silently turned the status update into a no-op (#60).
const SQL_EVAL_CONVICTION: &str = r#"UPDATE thesis SET conviction_conditions = jsonb_set(jsonb_set(jsonb_set(COALESCE(conviction_conditions,'[]'::jsonb), ARRAY[$1,'status'], $2, true), ARRAY[$1,'last_observed_value'], $3, true), ARRAY[$1,'last_checked_at'], $4, true) WHERE thesis_id=$5"#;
const SQL_EVAL_TRIGGER: &str = r#"UPDATE thesis SET trigger_conditions = jsonb_set(jsonb_set(jsonb_set(COALESCE(trigger_conditions,'[]'::jsonb), ARRAY[$1,'status'], $2, true), ARRAY[$1,'last_observed_value'], $3, true), ARRAY[$1,'last_checked_at'], $4, true) WHERE thesis_id=$5"#;
const SQL_EVAL_INVALIDATION: &str = r#"UPDATE thesis SET invalidation_conditions = jsonb_set(jsonb_set(jsonb_set(COALESCE(invalidation_conditions,'[]'::jsonb), ARRAY[$1,'status'], $2, true), ARRAY[$1,'last_observed_value'], $3, true), ARRAY[$1,'last_checked_at'], $4, true) WHERE thesis_id=$5"#;
const SQL_EVAL_FULFILLMENT: &str = r#"UPDATE thesis SET fulfillment_conditions = jsonb_set(jsonb_set(jsonb_set(COALESCE(fulfillment_conditions,'[]'::jsonb), ARRAY[$1,'status'], $2, true), ARRAY[$1,'last_observed_value'], $3, true), ARRAY[$1,'last_checked_at'], $4, true) WHERE thesis_id=$5"#;

/// Long-running service entry point.
pub async fn run(pool: PgPool, interval: std::time::Duration) -> Result<()> {
    info!(interval_secs = interval.as_secs(), "condition evaluator started");
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        match run_once(&pool).await {
            Ok(c) if c.satisfied + c.refuted + c.inconclusive + c.stale > 0 => {
                info!(
                    satisfied = c.satisfied, refuted = c.refuted,
                    inconclusive = c.inconclusive, stale = c.stale,
                    still_pending = c.still_pending, skipped = c.skipped,
                    "evaluator pass complete"
                );
            }
            Ok(_) => {} // quiet on no-op
            Err(e) => warn!(error = %e, "evaluator pass failed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use pretty_assertions::assert_eq;

    fn t(metric: &str, op: &str, value: f64) -> Target {
        Target { metric: metric.into(), op: op.into(), value, unit: None }
    }

    // ---- period prefix parsing (#62 / PR E) ----

    #[test]
    fn parse_period_strips_q_fy_correctly() {
        let (p, base) = parse_period_prefix("Q3_FY2026_revenue").unwrap();
        assert_eq!(p, PeriodTag { fiscal_year: 2026, quarter: Some(3) });
        assert_eq!(base, "revenue");
    }

    #[test]
    fn parse_period_strips_fy_only_for_annual() {
        let (p, base) = parse_period_prefix("FY2027_net_income").unwrap();
        assert_eq!(p, PeriodTag { fiscal_year: 2027, quarter: None });
        assert_eq!(base, "net_income");
    }

    #[test]
    fn parse_period_accepts_q_without_fy_keyword() {
        let (p, base) = parse_period_prefix("Q1_2026_gross_margin_pct").unwrap();
        assert_eq!(p, PeriodTag { fiscal_year: 2026, quarter: Some(1) });
        assert_eq!(base, "gross_margin_pct");
    }

    #[test]
    fn parse_period_returns_none_for_current_period_metric() {
        assert!(parse_period_prefix("revenue").is_none());
        assert!(parse_period_prefix("Revenues").is_none());
        assert!(parse_period_prefix("gross_margin_pct").is_none());
        assert!(parse_period_prefix("close").is_none());
    }

    #[test]
    fn parse_period_returns_none_for_unparseable_quarter() {
        // Q5 isn't valid; "Q_FY2026_x" missing digit; bare "Q3" no year.
        assert!(parse_period_prefix("Q5_FY2026_revenue").is_none());
        assert!(parse_period_prefix("Q_FY2026_revenue").is_none());
        assert!(parse_period_prefix("Q3_revenue").is_none());
    }

    #[test]
    fn parse_period_handles_uppercase_q_only() {
        // We don't normalize case — lowercase q is a different identifier.
        assert!(parse_period_prefix("q3_FY2026_revenue").is_none());
    }

    #[test]
    fn satisfies_ops() {
        assert!(satisfies(10.0, ">", 5.0));
        assert!(!satisfies(10.0, ">", 10.0));
        assert!(satisfies(10.0, ">=", 10.0));
        assert!(satisfies(5.0, "<", 10.0));
        assert!(satisfies(10.0, "<=", 10.0));
        assert!(satisfies(5.0, "==", 5.0));
        assert!(!satisfies(5.0, "!=", 5.0));
        assert!(!satisfies(5.0, "garbage", 5.0), "unknown op → false");
    }

    #[test]
    fn missing_observation_is_inconclusive() {
        let e = evaluate(&t("NVDA.Revenues", ">=", 100e9), None, None, Utc::now());
        assert_eq!(e.status, ConditionStatus::Inconclusive);
    }

    #[test]
    fn pending_and_inconclusive_conditions_are_retried() {
        assert!(ConditionStatus::Pending.should_evaluate());
        assert!(ConditionStatus::Inconclusive.should_evaluate());
        assert!(!ConditionStatus::Satisfied.should_evaluate());
        assert!(!ConditionStatus::Refuted.should_evaluate());
        assert!(!ConditionStatus::Stale.should_evaluate());
    }

    #[test]
    fn condition_status_decodes_db_values() {
        assert_eq!(ConditionStatus::from_db("pending"), Some(ConditionStatus::Pending));
        assert_eq!(ConditionStatus::from_db("inconclusive"), Some(ConditionStatus::Inconclusive));
        assert_eq!(ConditionStatus::from_db("bogus"), None);
    }

    #[test]
    fn satisfied_when_observed_meets_target() {
        let e = evaluate(&t("NVDA.Revenues", ">=", 100e9), Some(215e9), None, Utc::now());
        assert_eq!(e.status, ConditionStatus::Satisfied);
        assert_eq!(e.observed, Some(215e9));
    }

    #[test]
    fn pending_when_target_not_met_within_deadline() {
        let now = Utc::now();
        let future = now + Duration::days(30);
        let e = evaluate(&t("NVDA.Revenues", ">=", 100e9), Some(50e9), Some(future), now);
        assert_eq!(e.status, ConditionStatus::Pending, "could still resolve");
    }

    #[test]
    fn refuted_when_target_not_met_and_deadline_past() {
        let now = Utc::now();
        let past = now - Duration::days(1);
        let e = evaluate(&t("NVDA.Revenues", ">=", 100e9), Some(50e9), Some(past), now);
        assert_eq!(e.status, ConditionStatus::Refuted);
    }

    #[test]
    fn pending_when_target_not_met_and_no_deadline() {
        let e = evaluate(&t("NVDA.Revenues", ">=", 100e9), Some(50e9), None, Utc::now());
        assert_eq!(e.status, ConditionStatus::Pending);
    }
}
