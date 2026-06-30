use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{Value, json};

use super::{StrategyDecision, StrategyDefinitionInput, TargetSide, TradePermissionInput};

#[derive(Debug, Clone, Default)]
pub struct AutomationControlState {
    pub kill_switch_enabled: bool,
    pub kill_switch_reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DataFreshnessPolicyState {
    pub status: String,
    pub latest_bar_at: Option<DateTime<Utc>>,
    pub max_age_days: i64,
    pub stale: bool,
}

#[derive(Debug, Clone)]
pub struct SessionPolicyState {
    pub is_open: bool,
    pub label: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RiskPolicyState {
    pub veto: bool,
    pub reasons: Vec<String>,
    pub warnings: Vec<String>,
    pub size_mult: f64,
    pub snapshot: Value,
}

#[derive(Debug, Clone)]
pub struct CapitalPolicyState {
    pub target_weight_pct: Option<f64>,
    pub max_allocation_pct: Option<f64>,
    pub target_notional_usd: Option<f64>,
    pub max_notional_usd: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct SleevePolicyState {
    pub status: String,
    pub current_side: TargetSide,
    pub allocated_notional_usd: Option<f64>,
    pub snapshot: Value,
}

#[derive(Debug, Clone)]
pub struct BrokerPolicyState {
    pub status: String,
    pub mismatch: bool,
    pub latest_sync_at: Option<DateTime<Utc>>,
    pub snapshot: Value,
}

#[derive(Debug, Clone)]
pub struct ProofPolicyInput {
    pub definition: StrategyDefinitionInput,
    pub permission: Option<TradePermissionInput>,
    pub decision: StrategyDecision,
    pub control: AutomationControlState,
    pub data_freshness: DataFreshnessPolicyState,
    pub session: SessionPolicyState,
    pub risk: RiskPolicyState,
    pub capital: CapitalPolicyState,
    pub sleeve: SleevePolicyState,
    pub broker: BrokerPolicyState,
    pub now: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProofPolicyDecision {
    pub result: String,
    pub blocked_reasons: Vec<String>,
    pub input_snapshot: Value,
    pub permission_snapshot: Value,
    pub risk_result: Value,
    pub data_freshness: Value,
    pub session_state: Value,
    pub capital_allocation: Value,
    pub broker_reconciliation: Value,
}

pub fn evaluate_proof_policy(input: &ProofPolicyInput) -> ProofPolicyDecision {
    let mut blocked_reasons = Vec::new();
    let permission_snapshot = permission_snapshot(input.permission.as_ref());
    if input.control.kill_switch_enabled {
        blocked_reasons.push("automation_kill_switch".to_string());
    }
    match input.permission.as_ref() {
        Some(permission) => {
            if permission.status != "approved" {
                blocked_reasons.push("permission_not_approved".to_string());
            }
            if permission.environment_scope != "shadow" {
                blocked_reasons.push("environment_not_shadow".to_string());
            }
            if permission.manual_freeze {
                blocked_reasons.push("permission_frozen".to_string());
            }
            if permission
                .expires_at
                .is_some_and(|expires_at| expires_at <= input.now)
            {
                blocked_reasons.push("permission_expired".to_string());
            }
            if permission.strategy_id != input.definition.strategy_id
                || permission.strategy_version != input.definition.strategy_version
            {
                blocked_reasons.push("permission_strategy_mismatch".to_string());
            }
        }
        None => blocked_reasons.push("permission_missing".to_string()),
    }
    if input.definition.config_hash.trim().is_empty() {
        blocked_reasons.push("strategy_config_hash_missing".to_string());
    }
    if input.decision.kind == super::StrategyDecisionKind::Blocked {
        blocked_reasons.extend(input.decision.blocked_reasons.iter().cloned());
    }
    if input.data_freshness.stale || input.data_freshness.status == "stale" {
        blocked_reasons.push("data_freshness_stale".to_string());
    }
    if input.data_freshness.status == "missing" {
        blocked_reasons.push("data_freshness_missing".to_string());
    }
    if !input.session.is_open {
        blocked_reasons.push("session_closed".to_string());
    }
    if input.risk.veto {
        blocked_reasons.extend(
            input
                .risk
                .reasons
                .iter()
                .map(|reason| format!("risk_{reason}")),
        );
    }
    if let (Some(target), Some(max)) = (
        input.capital.target_weight_pct,
        input.capital.max_allocation_pct,
    ) {
        if target > max + 0.000_001 {
            blocked_reasons.push("allocation_exceeds_permission_cap".to_string());
        }
    }
    if let (Some(target), Some(max)) = (
        input.capital.target_notional_usd,
        input.capital.max_notional_usd,
    ) {
        if target > max + 0.000_001 {
            blocked_reasons.push("allocation_exceeds_notional_cap".to_string());
        }
    }
    match input.sleeve.status.as_str() {
        "frozen" => blocked_reasons.push("sleeve_frozen".to_string()),
        "closed" => blocked_reasons.push("sleeve_closed".to_string()),
        _ => {}
    }
    if input.broker.mismatch || input.broker.status == "mismatch" {
        blocked_reasons.push("broker_state_mismatch".to_string());
    }
    blocked_reasons.sort();
    blocked_reasons.dedup();

    let result = if blocked_reasons.is_empty() {
        if input.risk.warnings.is_empty() {
            "passed"
        } else {
            "warning"
        }
    } else {
        "blocked"
    };

    ProofPolicyDecision {
        result: result.to_string(),
        input_snapshot: json!({
            "evaluated_at": input.now,
            "strategy": {
                "strategy_id": input.definition.strategy_id,
                "strategy_version": input.definition.strategy_version,
                "strategy_config_hash": input.definition.config_hash,
                "family": input.definition.family,
                "status": input.definition.status,
            },
            "decision": {
                "kind": input.decision.kind,
                "target_side": input.decision.target_side.map(TargetSide::as_str),
                "target_weight_pct": input.decision.target_weight_pct,
                "reason_codes": input.decision.reason_codes,
                "blocked_reasons": input.decision.blocked_reasons,
            },
            "control": {
                "kill_switch_enabled": input.control.kill_switch_enabled,
                "kill_switch_reason": input.control.kill_switch_reason,
            },
            "sleeve": input.sleeve.snapshot,
        }),
        permission_snapshot,
        risk_result: json!({
            "veto": input.risk.veto,
            "reasons": input.risk.reasons,
            "warnings": input.risk.warnings,
            "size_mult": input.risk.size_mult,
            "snapshot": input.risk.snapshot,
        }),
        data_freshness: json!({
            "status": input.data_freshness.status,
            "latest_bar_at": input.data_freshness.latest_bar_at,
            "max_age_days": input.data_freshness.max_age_days,
            "stale": input.data_freshness.stale,
        }),
        session_state: json!({
            "is_open": input.session.is_open,
            "label": input.session.label,
            "reason": input.session.reason,
        }),
        capital_allocation: json!({
            "target_weight_pct": input.capital.target_weight_pct,
            "max_allocation_pct": input.capital.max_allocation_pct,
            "target_notional_usd": input.capital.target_notional_usd,
            "max_notional_usd": input.capital.max_notional_usd,
        }),
        broker_reconciliation: json!({
            "status": input.broker.status,
            "mismatch": input.broker.mismatch,
            "latest_sync_at": input.broker.latest_sync_at,
            "snapshot": input.broker.snapshot,
        }),
        blocked_reasons,
    }
}

fn permission_snapshot(permission: Option<&TradePermissionInput>) -> Value {
    match permission {
        Some(permission) => json!({
            "permission_id": permission.permission_id,
            "symbol": permission.symbol,
            "strategy_id": permission.strategy_id,
            "strategy_version": permission.strategy_version,
            "status": permission.status,
            "instrument_scope": permission.instrument_scope,
            "environment_scope": permission.environment_scope,
            "manual_freeze": permission.manual_freeze,
            "expires_at": permission.expires_at,
            "max_allocation_pct": permission.max_allocation_pct,
            "max_notional_usd": permission.max_notional_usd,
            "max_quantity": permission.max_quantity,
        }),
        None => json!({"status": "missing"}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::{StrategyDecisionKind, ValidationPlan};
    use chrono::{Duration as ChronoDuration, TimeZone};
    use pretty_assertions::assert_eq;
    use uuid::Uuid;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 30, 14, 0, 0).unwrap()
    }

    fn definition() -> StrategyDefinitionInput {
        StrategyDefinitionInput {
            strategy_id: "technical_timing".to_string(),
            strategy_version: "0.1.0".to_string(),
            family: "technical_timing".to_string(),
            display_name: "Technical Timing".to_string(),
            status: "shadow".to_string(),
            config_hash: "sha256:test".to_string(),
            config: json!({}),
        }
    }

    fn permission() -> TradePermissionInput {
        TradePermissionInput {
            permission_id: Uuid::nil(),
            symbol: "NVDA".to_string(),
            strategy_id: "technical_timing".to_string(),
            strategy_version: "0.1.0".to_string(),
            status: "approved".to_string(),
            instrument_scope: "equity_long_only".to_string(),
            environment_scope: "shadow".to_string(),
            manual_freeze: false,
            expires_at: Some(now() + ChronoDuration::days(10)),
            max_allocation_pct: Some(0.10),
            max_notional_usd: Some(20_000.0),
            max_quantity: None,
        }
    }

    fn decision() -> StrategyDecision {
        StrategyDecision {
            kind: StrategyDecisionKind::EmitDesired,
            target_side: Some(TargetSide::Long),
            target_weight_pct: Some(0.05),
            rationale: "test".to_string(),
            reason_codes: vec!["technical_long_constructive".to_string()],
            blocked_reasons: vec![],
            feature_snapshot: json!({"technical": {"state": "constructive"}}),
            signal_ref: json!({"mode": "shadow"}),
            validation: ValidationPlan {
                forward_only: true,
                horizon_days: 20,
                evaluation_due_at: now() + ChronoDuration::days(20),
                churn_event: true,
            },
        }
    }

    fn base_input() -> ProofPolicyInput {
        ProofPolicyInput {
            definition: definition(),
            permission: Some(permission()),
            decision: decision(),
            control: AutomationControlState::default(),
            data_freshness: DataFreshnessPolicyState {
                status: "fresh".to_string(),
                latest_bar_at: Some(now() - ChronoDuration::hours(1)),
                max_age_days: 5,
                stale: false,
            },
            session: SessionPolicyState {
                is_open: true,
                label: "regular".to_string(),
                reason: None,
            },
            risk: RiskPolicyState {
                veto: false,
                reasons: vec![],
                warnings: vec![],
                size_mult: 1.0,
                snapshot: json!({"veto": false}),
            },
            capital: CapitalPolicyState {
                target_weight_pct: Some(0.05),
                max_allocation_pct: Some(0.10),
                target_notional_usd: Some(5_000.0),
                max_notional_usd: Some(20_000.0),
            },
            sleeve: SleevePolicyState {
                status: "active".to_string(),
                current_side: TargetSide::Flat,
                allocated_notional_usd: None,
                snapshot: json!({"status": "active"}),
            },
            broker: BrokerPolicyState {
                status: "in_sync".to_string(),
                mismatch: false,
                latest_sync_at: Some(now() - ChronoDuration::minutes(5)),
                snapshot: json!({"status": "in_sync"}),
            },
            now: now(),
        }
    }

    fn assert_blocked(input: ProofPolicyInput, reason: &str) {
        let proof = evaluate_proof_policy(&input);
        assert_eq!(proof.result, "blocked");
        assert!(
            proof.blocked_reasons.iter().any(|r| r == reason),
            "missing {reason:?} in {:?}",
            proof.blocked_reasons
        );
    }

    #[test]
    fn stale_data_blocks_proof() {
        let mut input = base_input();
        input.data_freshness.stale = true;
        input.data_freshness.status = "stale".to_string();
        assert_blocked(input, "data_freshness_stale");
    }

    #[test]
    fn missing_or_frozen_permission_blocks_proof() {
        let mut input = base_input();
        input.permission = None;
        assert_blocked(input, "permission_missing");

        let mut input = base_input();
        input.permission.as_mut().unwrap().manual_freeze = true;
        assert_blocked(input, "permission_frozen");
    }

    #[test]
    fn exceeded_allocation_blocks_proof() {
        let mut input = base_input();
        input.capital.target_weight_pct = Some(0.20);
        assert_blocked(input, "allocation_exceeds_permission_cap");
    }

    #[test]
    fn risk_veto_blocks_proof() {
        let mut input = base_input();
        input.risk.veto = true;
        input.risk.reasons = vec!["drawdown_brake_halt".to_string()];
        assert_blocked(input, "risk_drawdown_brake_halt");
    }

    #[test]
    fn closed_session_blocks_proof() {
        let mut input = base_input();
        input.session.is_open = false;
        input.session.reason = Some("market closed".to_string());
        assert_blocked(input, "session_closed");
    }

    #[test]
    fn broker_mismatch_blocks_proof() {
        let mut input = base_input();
        input.broker.mismatch = true;
        input.broker.status = "mismatch".to_string();
        assert_blocked(input, "broker_state_mismatch");
    }

    #[test]
    fn kill_switch_blocks_proof() {
        let mut input = base_input();
        input.control.kill_switch_enabled = true;
        input.control.kill_switch_reason = Some("operator halt".to_string());
        assert_blocked(input, "automation_kill_switch");
    }
}
