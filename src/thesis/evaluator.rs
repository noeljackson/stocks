//! Condition evaluator (#14) — pure-function metric resolution + comparison.
//!
//! Walks every pending condition, resolves its `target.metric` against
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
/// Supported patterns (v1):
/// - `<SYMBOL>.<us-gaap-concept>` → latest company_fact.value for that
///   (symbol, concept), e.g. `NVDA.Revenues`, `MU.GrossProfit`
/// - `<SYMBOL>.close` → latest price_bar.close for the symbol
/// - `<SYMBOL>.gross_margin_pct` → computed: GrossProfit / Revenues × 100
/// - `<SYMBOL>.operating_margin_pct` → OperatingIncomeLoss / Revenues × 100
/// - `<SYMBOL>.net_margin_pct` → NetIncomeLoss / Revenues × 100
///
/// Period-qualified metrics like `NVDA.Q3_FY2027_revenue` are NOT resolvable
/// in v1 — they imply forward observations the system can't compute yet,
/// and return None → status = inconclusive.
pub async fn resolve_metric(pool: &PgPool, metric: &str) -> Result<Option<f64>> {
    let Some((symbol, suffix)) = metric.split_once('.') else {
        return Ok(None);
    };
    let symbol = symbol.to_ascii_uppercase();

    // Skip period-qualified metrics until we add per-period resolution.
    if suffix.contains("FY") || suffix.contains('Q') && suffix.chars().any(|c| c.is_ascii_digit()) {
        return Ok(None);
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

/// One pass: evaluate every pending condition with a parseable target,
/// update statuses in the DB. Returns counts per outcome.
pub async fn run_once(pool: &PgPool) -> Result<EvalCounts> {
    let now = Utc::now();
    let rows = sqlx::query(
        r#"SELECT thesis_id, role, position, COALESCE(name, '') AS name,
                  target, deadline_at
             FROM v_condition
            WHERE status = 'pending' AND target IS NOT NULL"#,
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
        let target_json: serde_json::Value = row.try_get("target")?;
        let deadline_at: Option<DateTime<Utc>> = row.try_get("deadline_at")?;

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
