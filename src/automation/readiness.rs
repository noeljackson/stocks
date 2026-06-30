//! Strategy lifecycle readiness gates.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationStage {
    Draft,
    Shadow,
    Paper,
    CanaryLive,
    ExpandedLive,
    Frozen,
    Retired,
}

impl AutomationStage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Shadow => "shadow",
            Self::Paper => "paper",
            Self::CanaryLive => "canary_live",
            Self::ExpandedLive => "expanded_live",
            Self::Frozen => "frozen",
            Self::Retired => "retired",
        }
    }

    fn rank(self) -> Option<u8> {
        match self {
            Self::Draft => Some(0),
            Self::Shadow => Some(1),
            Self::Paper => Some(2),
            Self::CanaryLive => Some(3),
            Self::ExpandedLive => Some(4),
            Self::Frozen | Self::Retired => None,
        }
    }

    pub fn is_live_capable(self) -> bool {
        matches!(self, Self::CanaryLive | Self::ExpandedLive)
    }

    pub fn is_runnable(self) -> bool {
        matches!(
            self,
            Self::Shadow | Self::Paper | Self::CanaryLive | Self::ExpandedLive
        )
    }

    pub fn allows_environment_scope(self, environment_scope: &str) -> bool {
        let Some(stage_rank) = self.rank() else {
            return false;
        };
        let Some(environment_rank) = environment_rank(environment_scope) else {
            return false;
        };
        self.is_runnable() && environment_rank <= stage_rank
    }
}

impl TryFrom<&str> for AutomationStage {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "draft" => Ok(Self::Draft),
            "shadow" => Ok(Self::Shadow),
            "paper" => Ok(Self::Paper),
            "canary_live" => Ok(Self::CanaryLive),
            "expanded_live" => Ok(Self::ExpandedLive),
            "frozen" => Ok(Self::Frozen),
            "retired" => Ok(Self::Retired),
            other => Err(anyhow::anyhow!("unsupported automation stage {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadinessStatus {
    Ready,
    Blocked,
}

impl ReadinessStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Blocked => "blocked",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ReadinessMetrics {
    pub observations_total: i64,
    pub outcomes_scored: i64,
    pub directional_outcomes_scored: i64,
    pub signal_quality_rate: Option<f64>,
    pub mean_forward_return_pct: Option<f64>,
    pub mean_max_drawdown_pct: Option<f64>,
    pub churn_rate: Option<f64>,
    pub proof_pass_rate: Option<f64>,
    pub incident_rate: Option<f64>,
    pub open_critical_incidents: i64,
    pub paper_orders_total: i64,
    pub paper_fill_quality_rate: Option<f64>,
    pub mean_slippage_bps: Option<f64>,
    pub baseline_excess_return_pct: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReadinessThresholds {
    pub min_directional_outcomes: i64,
    pub min_signal_quality_rate: f64,
    pub min_mean_forward_return_pct: f64,
    pub max_mean_drawdown_pct: f64,
    pub max_churn_rate: f64,
    pub min_proof_pass_rate: f64,
    pub max_incident_rate: f64,
    pub min_paper_orders_for_live: i64,
    pub min_paper_fill_quality_rate: f64,
    pub max_mean_slippage_bps: f64,
    pub min_baseline_excess_return_pct: f64,
}

impl Default for ReadinessThresholds {
    fn default() -> Self {
        Self {
            min_directional_outcomes: 20,
            min_signal_quality_rate: 0.55,
            min_mean_forward_return_pct: 0.0,
            max_mean_drawdown_pct: -8.0,
            max_churn_rate: 0.35,
            min_proof_pass_rate: 0.80,
            max_incident_rate: 0.05,
            min_paper_orders_for_live: 5,
            min_paper_fill_quality_rate: 0.80,
            max_mean_slippage_bps: 25.0,
            min_baseline_excess_return_pct: 0.0,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PromotionApproval {
    pub from_stage: AutomationStage,
    pub to_stage: AutomationStage,
    pub status: String,
    pub approved_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct ReadinessInput {
    pub current_stage: AutomationStage,
    pub metrics: ReadinessMetrics,
    pub approval: Option<PromotionApproval>,
    pub thresholds: ReadinessThresholds,
    pub now: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReadinessDecision {
    pub current_stage: AutomationStage,
    pub target_stage: Option<AutomationStage>,
    pub status: ReadinessStatus,
    pub readiness_score: f64,
    pub blockers: Vec<String>,
    pub warnings: Vec<String>,
    pub approval_required: bool,
    pub approval_valid: bool,
    pub freeze_live_permissions: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManualStageChange {
    pub allowed: bool,
    pub event_kind: &'static str,
    pub reason: Option<&'static str>,
}

pub fn evaluate_readiness(input: &ReadinessInput) -> ReadinessDecision {
    let target_stage = next_stage(input.current_stage);
    let mut blockers = Vec::new();
    let mut warnings = Vec::new();
    let approval_required = target_stage.is_some();

    if target_stage.is_none() {
        warnings.push("no_higher_stage".to_string());
    }
    if input.current_stage == AutomationStage::Frozen {
        blockers.push("strategy_frozen".to_string());
    }
    if input.current_stage == AutomationStage::Retired {
        blockers.push("strategy_retired".to_string());
    }

    if input.metrics.directional_outcomes_scored < input.thresholds.min_directional_outcomes {
        blockers.push("insufficient_scored_outcomes".to_string());
    }
    require_min(
        &mut blockers,
        input.metrics.signal_quality_rate,
        input.thresholds.min_signal_quality_rate,
        "signal_quality_below_threshold",
    );
    require_min(
        &mut blockers,
        input.metrics.mean_forward_return_pct,
        input.thresholds.min_mean_forward_return_pct,
        "mean_forward_return_below_threshold",
    );
    require_min(
        &mut blockers,
        input.metrics.baseline_excess_return_pct,
        input.thresholds.min_baseline_excess_return_pct,
        "baseline_underperformed",
    );
    require_max(
        &mut blockers,
        input.metrics.churn_rate,
        input.thresholds.max_churn_rate,
        "churn_rate_high",
    );
    require_min(
        &mut blockers,
        input.metrics.proof_pass_rate,
        input.thresholds.min_proof_pass_rate,
        "proof_pass_rate_below_threshold",
    );
    require_max(
        &mut blockers,
        input.metrics.incident_rate,
        input.thresholds.max_incident_rate,
        "incident_rate_high",
    );
    if input.metrics.open_critical_incidents > 0 {
        blockers.push("open_critical_incidents".to_string());
    }
    match input.metrics.mean_max_drawdown_pct {
        Some(drawdown) if drawdown < input.thresholds.max_mean_drawdown_pct => {
            blockers.push("mean_drawdown_too_deep".to_string());
        }
        None => blockers.push("mean_drawdown_missing".to_string()),
        _ => {}
    }

    let target_is_live = target_stage.is_some_and(AutomationStage::is_live_capable);
    if target_is_live {
        if input.metrics.paper_orders_total < input.thresholds.min_paper_orders_for_live {
            blockers.push("insufficient_paper_orders".to_string());
        }
        require_min(
            &mut blockers,
            input.metrics.paper_fill_quality_rate,
            input.thresholds.min_paper_fill_quality_rate,
            "paper_fill_quality_below_threshold",
        );
        if let Some(slippage) = input.metrics.mean_slippage_bps {
            if slippage > input.thresholds.max_mean_slippage_bps {
                blockers.push("slippage_too_high".to_string());
            }
        }
    }

    let approval_valid = match (approval_required, target_stage, input.approval.as_ref()) {
        (false, _, _) => false,
        (true, Some(target), Some(approval)) => {
            if approval.from_stage != input.current_stage || approval.to_stage != target {
                blockers.push("approval_stage_mismatch".to_string());
                false
            } else if approval.status != "approved" {
                blockers.push("approval_not_active".to_string());
                false
            } else if approval
                .expires_at
                .is_some_and(|expires_at| expires_at <= input.now)
            {
                blockers.push("approval_expired".to_string());
                false
            } else {
                true
            }
        }
        (true, Some(_), None) => {
            blockers.push("approval_missing".to_string());
            false
        }
        _ => false,
    };

    blockers.sort();
    blockers.dedup();
    warnings.sort();
    warnings.dedup();

    let freeze_live_permissions = input.current_stage.is_live_capable()
        && (blockers.iter().any(|b| {
            matches!(
                b.as_str(),
                "incident_rate_high"
                    | "open_critical_incidents"
                    | "proof_pass_rate_below_threshold"
            )
        }));
    let status = if blockers.is_empty() && approval_valid {
        ReadinessStatus::Ready
    } else {
        ReadinessStatus::Blocked
    };

    ReadinessDecision {
        current_stage: input.current_stage,
        target_stage,
        status,
        readiness_score: readiness_score(input, approval_valid),
        blockers,
        warnings,
        approval_required,
        approval_valid,
        freeze_live_permissions,
    }
}

pub fn manual_stage_change(from: AutomationStage, to: AutomationStage) -> ManualStageChange {
    if from == to {
        return ManualStageChange {
            allowed: false,
            event_kind: "blocked",
            reason: Some("same_stage"),
        };
    }
    if matches!(to, AutomationStage::Frozen) {
        return ManualStageChange {
            allowed: true,
            event_kind: "frozen",
            reason: None,
        };
    }
    if matches!(to, AutomationStage::Retired) {
        return ManualStageChange {
            allowed: true,
            event_kind: "retired",
            reason: None,
        };
    }
    match (from.rank(), to.rank()) {
        (Some(from_rank), Some(to_rank)) if to_rank < from_rank => ManualStageChange {
            allowed: true,
            event_kind: "demoted",
            reason: None,
        },
        (Some(from_rank), Some(to_rank)) if to_rank > from_rank => ManualStageChange {
            allowed: false,
            event_kind: "blocked",
            reason: Some("promotion_requires_readiness_gate"),
        },
        _ => ManualStageChange {
            allowed: false,
            event_kind: "blocked",
            reason: Some("manual_change_not_allowed"),
        },
    }
}

fn require_min(blockers: &mut Vec<String>, value: Option<f64>, threshold: f64, reason: &str) {
    match value {
        Some(value) if value + f64::EPSILON < threshold => blockers.push(reason.to_string()),
        None => blockers.push(reason.to_string()),
        _ => {}
    }
}

fn require_max(blockers: &mut Vec<String>, value: Option<f64>, threshold: f64, reason: &str) {
    match value {
        Some(value) if value > threshold + f64::EPSILON => blockers.push(reason.to_string()),
        None => blockers.push(reason.to_string()),
        _ => {}
    }
}

fn readiness_score(input: &ReadinessInput, approval_valid: bool) -> f64 {
    let thresholds = &input.thresholds;
    let metrics = &input.metrics;
    let mut parts = Vec::new();
    parts.push(ratio_score(
        metrics.directional_outcomes_scored as f64,
        thresholds.min_directional_outcomes as f64,
    ));
    parts.push(ratio_score(
        metrics.signal_quality_rate.unwrap_or(0.0),
        thresholds.min_signal_quality_rate,
    ));
    parts.push(non_negative_score(
        metrics.mean_forward_return_pct.unwrap_or(-10.0),
        thresholds.min_mean_forward_return_pct,
    ));
    parts.push(non_negative_score(
        metrics.baseline_excess_return_pct.unwrap_or(-10.0),
        thresholds.min_baseline_excess_return_pct,
    ));
    parts.push(inverted_ratio_score(
        metrics.churn_rate.unwrap_or(1.0),
        thresholds.max_churn_rate,
    ));
    parts.push(ratio_score(
        metrics.proof_pass_rate.unwrap_or(0.0),
        thresholds.min_proof_pass_rate,
    ));
    parts.push(inverted_ratio_score(
        metrics.incident_rate.unwrap_or(1.0),
        thresholds.max_incident_rate,
    ));
    if next_stage(input.current_stage).is_some_and(AutomationStage::is_live_capable) {
        parts.push(ratio_score(
            metrics.paper_orders_total as f64,
            thresholds.min_paper_orders_for_live as f64,
        ));
        parts.push(ratio_score(
            metrics.paper_fill_quality_rate.unwrap_or(0.0),
            thresholds.min_paper_fill_quality_rate,
        ));
    }
    parts.push(if approval_valid { 1.0 } else { 0.0 });
    let avg = parts.iter().sum::<f64>() / parts.len().max(1) as f64;
    avg.clamp(0.0, 1.0)
}

fn ratio_score(value: f64, threshold: f64) -> f64 {
    if threshold <= f64::EPSILON {
        return 1.0;
    }
    (value / threshold).clamp(0.0, 1.0)
}

fn inverted_ratio_score(value: f64, threshold: f64) -> f64 {
    if threshold <= f64::EPSILON {
        return if value <= threshold { 1.0 } else { 0.0 };
    }
    (1.0 - value / threshold).clamp(0.0, 1.0)
}

fn non_negative_score(value: f64, threshold: f64) -> f64 {
    if value >= threshold {
        1.0
    } else {
        (1.0 + value.abs().recip()).clamp(0.0, 1.0) * 0.5
    }
}

fn next_stage(stage: AutomationStage) -> Option<AutomationStage> {
    match stage {
        AutomationStage::Draft => Some(AutomationStage::Shadow),
        AutomationStage::Shadow => Some(AutomationStage::Paper),
        AutomationStage::Paper => Some(AutomationStage::CanaryLive),
        AutomationStage::CanaryLive => Some(AutomationStage::ExpandedLive),
        AutomationStage::ExpandedLive | AutomationStage::Frozen | AutomationStage::Retired => None,
    }
}

fn environment_rank(environment_scope: &str) -> Option<u8> {
    match environment_scope {
        "shadow" => Some(1),
        "paper" => Some(2),
        "canary_live" => Some(3),
        "expanded_live" => Some(4),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration as ChronoDuration, TimeZone};

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 30, 15, 0, 0).unwrap()
    }

    fn passing_metrics() -> ReadinessMetrics {
        ReadinessMetrics {
            observations_total: 44,
            outcomes_scored: 36,
            directional_outcomes_scored: 32,
            signal_quality_rate: Some(0.66),
            mean_forward_return_pct: Some(4.2),
            mean_max_drawdown_pct: Some(-3.1),
            churn_rate: Some(0.12),
            proof_pass_rate: Some(0.94),
            incident_rate: Some(0.01),
            baseline_excess_return_pct: Some(2.4),
            ..ReadinessMetrics::default()
        }
    }

    fn approval(from_stage: AutomationStage, to_stage: AutomationStage) -> PromotionApproval {
        PromotionApproval {
            from_stage,
            to_stage,
            status: "approved".to_string(),
            approved_at: now() - ChronoDuration::days(1),
            expires_at: Some(now() + ChronoDuration::days(7)),
        }
    }

    #[test]
    fn promotion_pass_requires_scored_edge_and_operator_approval() {
        let input = ReadinessInput {
            current_stage: AutomationStage::Shadow,
            metrics: passing_metrics(),
            approval: Some(approval(AutomationStage::Shadow, AutomationStage::Paper)),
            thresholds: ReadinessThresholds::default(),
            now: now(),
        };

        let decision = evaluate_readiness(&input);

        assert_eq!(decision.status, ReadinessStatus::Ready);
        assert_eq!(decision.target_stage, Some(AutomationStage::Paper));
        assert!(decision.blockers.is_empty());
        assert!(decision.approval_valid);
        assert!(!decision.freeze_live_permissions);
    }

    #[test]
    fn promotion_fail_lists_specific_metric_blockers() {
        let mut metrics = passing_metrics();
        metrics.directional_outcomes_scored = 4;
        metrics.signal_quality_rate = Some(0.44);
        metrics.mean_forward_return_pct = Some(-1.2);
        metrics.mean_max_drawdown_pct = Some(-12.0);
        metrics.churn_rate = Some(0.61);
        metrics.proof_pass_rate = Some(0.58);
        metrics.incident_rate = Some(0.18);
        metrics.baseline_excess_return_pct = Some(-2.1);
        let input = ReadinessInput {
            current_stage: AutomationStage::Shadow,
            metrics,
            approval: None,
            thresholds: ReadinessThresholds::default(),
            now: now(),
        };

        let decision = evaluate_readiness(&input);

        assert_eq!(decision.status, ReadinessStatus::Blocked);
        assert!(
            decision
                .blockers
                .contains(&"insufficient_scored_outcomes".to_string())
        );
        assert!(
            decision
                .blockers
                .contains(&"signal_quality_below_threshold".to_string())
        );
        assert!(
            decision
                .blockers
                .contains(&"mean_drawdown_too_deep".to_string())
        );
        assert!(
            decision
                .blockers
                .contains(&"proof_pass_rate_below_threshold".to_string())
        );
        assert!(decision.blockers.contains(&"approval_missing".to_string()));
    }

    #[test]
    fn expired_operator_approval_blocks_otherwise_ready_promotion() {
        let mut approval = approval(AutomationStage::Shadow, AutomationStage::Paper);
        approval.expires_at = Some(now() - ChronoDuration::seconds(1));
        let input = ReadinessInput {
            current_stage: AutomationStage::Shadow,
            metrics: passing_metrics(),
            approval: Some(approval),
            thresholds: ReadinessThresholds::default(),
            now: now(),
        };

        let decision = evaluate_readiness(&input);

        assert_eq!(decision.status, ReadinessStatus::Blocked);
        assert!(decision.blockers.contains(&"approval_expired".to_string()));
        assert!(!decision.approval_valid);
    }

    #[test]
    fn excessive_incidents_freeze_live_capable_permissions() {
        let mut metrics = passing_metrics();
        metrics.paper_orders_total = 18;
        metrics.paper_fill_quality_rate = Some(0.92);
        metrics.incident_rate = Some(0.27);
        metrics.open_critical_incidents = 1;
        let input = ReadinessInput {
            current_stage: AutomationStage::CanaryLive,
            metrics,
            approval: Some(approval(
                AutomationStage::CanaryLive,
                AutomationStage::ExpandedLive,
            )),
            thresholds: ReadinessThresholds::default(),
            now: now(),
        };

        let decision = evaluate_readiness(&input);

        assert_eq!(decision.status, ReadinessStatus::Blocked);
        assert!(
            decision
                .blockers
                .contains(&"incident_rate_high".to_string())
        );
        assert!(
            decision
                .blockers
                .contains(&"open_critical_incidents".to_string())
        );
        assert!(decision.freeze_live_permissions);
    }

    #[test]
    fn manual_demotion_is_allowed_without_readiness_but_manual_promotion_is_not() {
        let demotion = manual_stage_change(AutomationStage::ExpandedLive, AutomationStage::Paper);
        assert!(demotion.allowed);
        assert_eq!(demotion.event_kind, "demoted");

        let promotion = manual_stage_change(AutomationStage::Paper, AutomationStage::ExpandedLive);
        assert!(!promotion.allowed);
        assert_eq!(promotion.reason, Some("promotion_requires_readiness_gate"));
    }
}
