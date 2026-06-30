//! Shadow strategy automation.
//!
//! Strategies in this module emit desired exposure only. They do not create
//! broker orders, mutate broker state, or bypass later proof/reconciliation
//! gates.

mod policy;

use anyhow::{Context, Result};
use chrono::{DateTime, Datelike, Duration as ChronoDuration, NaiveDate, NaiveTime, Utc, Weekday};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::platform::{
    market_calendar,
    store::Store,
    technical::{TechnicalState, build_technical_state},
};
use crate::risk;

pub use policy::{
    AutomationControlState, BrokerPolicyState, CapitalPolicyState, DataFreshnessPolicyState,
    ProofPolicyDecision, ProofPolicyInput, RiskPolicyState, SessionPolicyState, SleevePolicyState,
    evaluate_proof_policy,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetSide {
    Flat,
    Long,
    Short,
}

impl TargetSide {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Flat => "flat",
            Self::Long => "long",
            Self::Short => "short",
        }
    }
}

impl TryFrom<&str> for TargetSide {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "flat" => Ok(Self::Flat),
            "long" => Ok(Self::Long),
            "short" => Ok(Self::Short),
            other => Err(anyhow::anyhow!("unsupported target side {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StrategyDecisionKind {
    EmitDesired,
    NoChange,
    Blocked,
}

#[derive(Debug, Clone, Serialize)]
pub struct StrategyDecision {
    pub kind: StrategyDecisionKind,
    pub target_side: Option<TargetSide>,
    pub target_weight_pct: Option<f64>,
    pub rationale: String,
    pub reason_codes: Vec<String>,
    pub blocked_reasons: Vec<String>,
    pub feature_snapshot: Value,
    pub signal_ref: Value,
    pub validation: ValidationPlan,
}

#[derive(Debug, Clone, Serialize)]
pub struct ValidationPlan {
    pub forward_only: bool,
    pub horizon_days: i64,
    pub evaluation_due_at: DateTime<Utc>,
    pub churn_event: bool,
}

#[derive(Debug, Clone)]
pub struct StrategyDefinitionInput {
    pub strategy_id: String,
    pub strategy_version: String,
    pub family: String,
    pub display_name: String,
    pub status: String,
    pub config_hash: String,
    pub config: Value,
}

#[derive(Debug, Clone)]
pub struct TradePermissionInput {
    pub permission_id: Uuid,
    pub symbol: String,
    pub strategy_id: String,
    pub strategy_version: String,
    pub status: String,
    pub instrument_scope: String,
    pub environment_scope: String,
    pub manual_freeze: bool,
    pub expires_at: Option<DateTime<Utc>>,
    pub max_allocation_pct: Option<f64>,
    pub max_notional_usd: Option<f64>,
    pub max_quantity: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct LatestDesiredPosition {
    pub desired_position_id: Uuid,
    pub target_side: TargetSide,
    pub target_weight_pct: Option<f64>,
    pub emitted_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct StrategyFeatures {
    pub symbol: String,
    pub technical: Option<TechnicalFeature>,
    pub thesis: Option<ThesisFeature>,
}

#[derive(Debug, Clone)]
pub struct TechnicalFeature {
    pub as_of: DateTime<Utc>,
    pub state: String,
    pub setup_kind: String,
    pub entry_stance: String,
    pub summary: String,
    pub close: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct ThesisFeature {
    pub thesis_id: Uuid,
    pub state: String,
    pub direction: Option<String>,
    pub freshness_status: Option<String>,
    pub freshness_score: Option<f64>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct StrategyEvaluationInput {
    pub definition: StrategyDefinitionInput,
    pub permission: Option<TradePermissionInput>,
    pub latest_desired: Option<LatestDesiredPosition>,
    pub features: StrategyFeatures,
    pub now: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct AutomationStrategyCandidate {
    pub definition: StrategyDefinitionInput,
    pub permission: TradePermissionInput,
    pub latest_desired: Option<LatestDesiredPosition>,
}

#[derive(Debug, Clone)]
pub struct DesiredPositionWrite {
    pub permission_id: Uuid,
    pub symbol: String,
    pub thesis_id: Option<Uuid>,
    pub strategy_id: String,
    pub strategy_version: String,
    pub strategy_config_hash: String,
    pub environment_scope: String,
    pub target_side: TargetSide,
    pub target_weight_pct: Option<f64>,
    pub rationale: String,
    pub reason_codes: Vec<String>,
    pub feature_snapshot: Value,
    pub signal_ref: Value,
    pub validation: ValidationPlan,
    pub prior_target_side: Option<TargetSide>,
    pub proof: ProofPolicyDecision,
}

#[derive(Debug, Clone)]
pub struct BlockedProofWrite {
    pub permission_id: Uuid,
    pub symbol: String,
    pub strategy_id: String,
    pub strategy_version: String,
    pub strategy_config_hash: String,
    pub environment_scope: String,
    pub proof: ProofPolicyDecision,
}

#[derive(Debug, Clone, Serialize)]
pub struct DesiredPositionReceipt {
    pub desired_position_id: Uuid,
    pub proof_id: Uuid,
    pub observation_id: Uuid,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct AutomationRunSummary {
    pub evaluated: usize,
    pub emitted: usize,
    pub unchanged: usize,
    pub blocked: usize,
    pub no_features: usize,
}

#[derive(Debug, Clone)]
pub struct BuiltinStrategyDefinition {
    pub strategy_id: &'static str,
    pub strategy_version: &'static str,
    pub family: &'static str,
    pub display_name: &'static str,
    pub description: &'static str,
    pub status: &'static str,
    pub config: Value,
}

impl BuiltinStrategyDefinition {
    pub fn config_hash(&self) -> String {
        config_hash(&self.config)
    }
}

pub fn builtin_strategy_definitions() -> Vec<BuiltinStrategyDefinition> {
    vec![
        BuiltinStrategyDefinition {
            strategy_id: "technical_timing",
            strategy_version: "0.1.0",
            family: "technical_timing",
            display_name: "Technical Timing",
            description: "Shadow-only technical timing strategy using derived chart state.",
            status: "shadow",
            config: json!({
                "default_weight_pct": 0.05,
                "max_bar_age_days": 5,
                "validation_horizon_days": 20,
                "long_entry_stances": ["actionable", "starter_ok", "constructive"],
                "flat_entry_stances": ["avoid", "avoid_chase", "wait_breakout", "wait_reversal", "wait_reclaim", "wait_data"]
            }),
        },
        BuiltinStrategyDefinition {
            strategy_id: "thesis_timing",
            strategy_version: "0.1.0",
            family: "thesis_timing",
            display_name: "Thesis Timing",
            description: "Shadow-only thesis timing strategy gated by actionable bullish thesis plus chart timing.",
            status: "shadow",
            config: json!({
                "default_weight_pct": 0.05,
                "max_bar_age_days": 5,
                "validation_horizon_days": 20,
                "actionable_thesis_states": ["actionable", "position_open"],
                "long_entry_stances": ["actionable", "starter_ok", "constructive"]
            }),
        },
    ]
}

pub fn config_hash(config: &Value) -> String {
    let encoded = serde_json::to_vec(config).unwrap_or_default();
    let digest = Sha256::digest(encoded);
    format!("sha256:{}", hex::encode(digest))
}

pub fn evaluate_strategy(input: &StrategyEvaluationInput) -> StrategyDecision {
    let mut blocked_reasons = common_blocked_reasons(input);
    let target = if blocked_reasons.is_empty() {
        match target_intent(input) {
            Ok(target) => Some(target),
            Err(mut reasons) => {
                blocked_reasons.append(&mut reasons);
                None
            }
        }
    } else {
        None
    };

    let Some(target) = target else {
        return blocked_decision(input, blocked_reasons);
    };

    let validation = validation_plan(input, target.side);
    let target_weight_pct = if target.side == TargetSide::Flat {
        Some(0.0)
    } else {
        Some(target_weight(input))
    };
    let kind = if latest_matches_target(
        input.latest_desired.as_ref(),
        target.side,
        target_weight_pct,
    ) {
        StrategyDecisionKind::NoChange
    } else {
        StrategyDecisionKind::EmitDesired
    };

    let feature_snapshot = feature_snapshot(
        input,
        Some(target.side),
        target_weight_pct,
        &target.reason_codes,
        &[],
    );
    StrategyDecision {
        kind,
        target_side: Some(target.side),
        target_weight_pct,
        rationale: target.rationale,
        reason_codes: target.reason_codes,
        blocked_reasons: vec![],
        signal_ref: signal_ref(input, &validation),
        feature_snapshot,
        validation,
    }
}

pub async fn run_once(store: &Store, limit: i64) -> Result<AutomationRunSummary> {
    store
        .ensure_builtin_automation_strategies(&builtin_strategy_definitions())
        .await?;
    let candidates = store.automation_strategy_candidates(limit).await?;
    let mut summary = AutomationRunSummary::default();
    for candidate in candidates {
        summary.evaluated += 1;
        let features = load_strategy_features(store, &candidate.permission.symbol)
            .await
            .with_context(|| format!("load strategy features {}", candidate.permission.symbol))?;
        if features.technical.is_none() {
            summary.no_features += 1;
        }
        let now = Utc::now();
        let input = StrategyEvaluationInput {
            definition: candidate.definition.clone(),
            permission: Some(candidate.permission.clone()),
            latest_desired: candidate.latest_desired.clone(),
            features,
            now,
        };
        let decision = evaluate_strategy(&input);
        let proof_input = proof_policy_input(store, &candidate, &input, &decision, now).await?;
        let proof = evaluate_proof_policy(&proof_input);
        if proof.result == "blocked" {
            let write = blocked_proof_write(&candidate, proof);
            store.insert_blocked_automation_proof(&write).await?;
            summary.blocked += 1;
            continue;
        }
        match decision.kind {
            StrategyDecisionKind::EmitDesired => {
                let write = desired_write(&candidate, &input, &decision, proof)
                    .context("build desired position write")?;
                store.insert_desired_strategy_position(&write).await?;
                summary.emitted += 1;
            }
            StrategyDecisionKind::NoChange => {
                summary.unchanged += 1;
            }
            StrategyDecisionKind::Blocked => {
                summary.blocked += 1;
            }
        }
    }
    Ok(summary)
}

pub async fn run(store: Store, interval: std::time::Duration, limit: i64) -> Result<()> {
    loop {
        let summary = run_once(&store, limit).await?;
        tracing::info!(
            evaluated = summary.evaluated,
            emitted = summary.emitted,
            unchanged = summary.unchanged,
            blocked = summary.blocked,
            no_features = summary.no_features,
            "automation strategy runner pass complete"
        );
        tokio::time::sleep(interval).await;
    }
}

async fn load_strategy_features(store: &Store, symbol: &str) -> Result<StrategyFeatures> {
    let daily = store.daily_technical_bars_for(symbol, 365 * 2).await?;
    let technical = if daily.is_empty() {
        None
    } else {
        let technical_state = build_technical_state(symbol, &daily, &[]);
        Some(technical_feature(&technical_state))
    };
    let thesis = store
        .theses_for_symbol(symbol)
        .await?
        .into_iter()
        .find(|t| !matches!(t.state.as_str(), "closed" | "disqualified"))
        .map(|t| ThesisFeature {
            thesis_id: t.thesis_id,
            state: t.state.as_str().to_string(),
            direction: t
                .forecast
                .get("direction")
                .and_then(Value::as_str)
                .map(str::to_ascii_lowercase),
            freshness_status: t
                .substance
                .as_ref()
                .map(|substance| substance.freshness_status.clone()),
            freshness_score: t
                .substance
                .as_ref()
                .map(|substance| substance.freshness_score),
            updated_at: t.updated_at,
        });
    Ok(StrategyFeatures {
        symbol: symbol.to_ascii_uppercase(),
        technical,
        thesis,
    })
}

fn technical_feature(state: &TechnicalState) -> TechnicalFeature {
    TechnicalFeature {
        as_of: state.as_of.unwrap_or_else(Utc::now),
        state: state.state.clone(),
        setup_kind: state.setup.kind.clone(),
        entry_stance: state.setup.entry_stance.clone(),
        summary: state.summary.clone(),
        close: state.daily.as_ref().map(|daily| daily.close),
    }
}

fn desired_write(
    candidate: &AutomationStrategyCandidate,
    input: &StrategyEvaluationInput,
    decision: &StrategyDecision,
    proof: ProofPolicyDecision,
) -> Result<DesiredPositionWrite> {
    let target_side = decision
        .target_side
        .context("emit decision missing target side")?;
    Ok(DesiredPositionWrite {
        permission_id: candidate.permission.permission_id,
        symbol: candidate.permission.symbol.clone(),
        thesis_id: input
            .features
            .thesis
            .as_ref()
            .map(|thesis| thesis.thesis_id),
        strategy_id: candidate.definition.strategy_id.clone(),
        strategy_version: candidate.definition.strategy_version.clone(),
        strategy_config_hash: candidate.definition.config_hash.clone(),
        environment_scope: candidate.permission.environment_scope.clone(),
        target_side,
        target_weight_pct: decision.target_weight_pct,
        rationale: decision.rationale.clone(),
        reason_codes: decision.reason_codes.clone(),
        feature_snapshot: decision.feature_snapshot.clone(),
        signal_ref: decision.signal_ref.clone(),
        validation: decision.validation.clone(),
        prior_target_side: candidate
            .latest_desired
            .as_ref()
            .map(|desired| desired.target_side),
        proof,
    })
}

fn blocked_proof_write(
    candidate: &AutomationStrategyCandidate,
    proof: ProofPolicyDecision,
) -> BlockedProofWrite {
    BlockedProofWrite {
        permission_id: candidate.permission.permission_id,
        symbol: candidate.permission.symbol.clone(),
        strategy_id: candidate.definition.strategy_id.clone(),
        strategy_version: candidate.definition.strategy_version.clone(),
        strategy_config_hash: candidate.definition.config_hash.clone(),
        environment_scope: candidate.permission.environment_scope.clone(),
        proof,
    }
}

async fn proof_policy_input(
    store: &Store,
    candidate: &AutomationStrategyCandidate,
    input: &StrategyEvaluationInput,
    decision: &StrategyDecision,
    now: DateTime<Utc>,
) -> Result<ProofPolicyInput> {
    let control = store.automation_control_state().await?;
    let data_freshness = data_freshness_policy_state(input);
    let session = session_policy_state(now);
    let (risk, capital) = risk_and_capital_policy_state(store, candidate, decision).await?;
    let sleeve = store
        .automation_sleeve_policy_state(candidate.permission.permission_id)
        .await?;
    let broker = store
        .automation_broker_policy_state(&candidate.permission.symbol)
        .await?;
    Ok(ProofPolicyInput {
        definition: candidate.definition.clone(),
        permission: Some(candidate.permission.clone()),
        decision: decision.clone(),
        control,
        data_freshness,
        session,
        risk,
        capital,
        sleeve,
        broker,
        now,
    })
}

fn data_freshness_policy_state(input: &StrategyEvaluationInput) -> DataFreshnessPolicyState {
    let max_age_days = config_i64(&input.definition.config, "max_bar_age_days", 5).max(1);
    let Some(technical) = input.features.technical.as_ref() else {
        return DataFreshnessPolicyState {
            status: "missing".to_string(),
            latest_bar_at: None,
            max_age_days,
            stale: true,
        };
    };
    let stale = input.now - technical.as_of > ChronoDuration::days(max_age_days);
    DataFreshnessPolicyState {
        status: if stale { "stale" } else { "fresh" }.to_string(),
        latest_bar_at: Some(technical.as_of),
        max_age_days,
        stale,
    }
}

async fn risk_and_capital_policy_state(
    store: &Store,
    candidate: &AutomationStrategyCandidate,
    decision: &StrategyDecision,
) -> Result<(RiskPolicyState, CapitalPolicyState)> {
    let positions = store.open_positions_for_risk().await.unwrap_or_default();
    let settings = store.portfolio_settings().await.unwrap_or_default();
    let realized_pnl = store.realized_pnl_total().await.unwrap_or(0.0);
    let (portfolio, portfolio_demo) =
        match risk::derive_portfolio(settings, &positions, realized_pnl) {
            Some(portfolio) => (portfolio, false),
            None => (
                risk::Portfolio {
                    total_value: 100_000.0,
                    cash_pct: 50.0,
                    drawdown_pct: 0.0,
                },
                true,
            ),
        };

    let target_weight_pct = decision.target_weight_pct;
    let target_notional_usd = target_weight_pct.map(|weight| weight * portfolio.total_value);
    let capital = CapitalPolicyState {
        target_weight_pct,
        max_allocation_pct: candidate.permission.max_allocation_pct,
        target_notional_usd,
        max_notional_usd: candidate.permission.max_notional_usd,
    };

    let risk_config = match store.active_config("risk").await {
        Ok((cfg_json, cfg_ver)) => match serde_json::from_value::<risk::Config>(cfg_json) {
            Ok(cfg) => Some((cfg, cfg_ver)),
            Err(error) => {
                return Ok((
                    RiskPolicyState {
                        veto: true,
                        reasons: vec!["config_invalid".to_string()],
                        warnings: vec![],
                        size_mult: 0.0,
                        snapshot: json!({
                            "status": "unavailable",
                            "reason": "risk config invalid",
                            "error": error.to_string(),
                        }),
                    },
                    capital,
                ));
            }
        },
        Err(error) => {
            return Ok((
                RiskPolicyState {
                    veto: true,
                    reasons: vec!["config_unavailable".to_string()],
                    warnings: vec![],
                    size_mult: 0.0,
                    snapshot: json!({
                        "status": "unavailable",
                        "reason": "risk config unavailable",
                        "error": error.to_string(),
                    }),
                },
                capital,
            ));
        }
    };

    let target_notional = match decision.target_side {
        Some(TargetSide::Long) => target_notional_usd.unwrap_or(0.0),
        _ => 0.0,
    };
    let cluster = store
        .ticker_cluster_id(&candidate.permission.symbol)
        .await
        .unwrap_or_default();
    let intent = risk::Intent {
        symbol: candidate.permission.symbol.clone(),
        cluster,
        instrument: "equity".to_string(),
        delta_notional: target_notional,
        premium_at_risk: 0.0,
    };
    let (cfg, cfg_ver) = risk_config.expect("risk_config is Some after early returns");
    let decision = risk::evaluate(&intent, &positions, portfolio, &cfg);
    let mut warnings = decision.warnings;
    if portfolio_demo {
        warnings.push("portfolio_demo".to_string());
    }
    let risk_status = if decision.veto {
        "veto"
    } else if warnings.is_empty() {
        "pass"
    } else {
        "warning"
    };
    Ok((
        RiskPolicyState {
            veto: decision.veto,
            reasons: decision.reasons,
            warnings,
            size_mult: decision.size_mult,
            snapshot: json!({
                "status": risk_status,
                "config_version": cfg_ver,
                "portfolio_demo": portfolio_demo,
                "portfolio": {
                    "total_value": portfolio.total_value,
                    "cash_pct": portfolio.cash_pct,
                    "drawdown_pct": portfolio.drawdown_pct,
                },
                "intent": {
                    "symbol": intent.symbol,
                    "cluster": intent.cluster,
                    "instrument": intent.instrument,
                    "delta_notional": intent.delta_notional,
                    "premium_at_risk": intent.premium_at_risk,
                },
            }),
        },
        capital,
    ))
}

fn session_policy_state(now: DateTime<Utc>) -> SessionPolicyState {
    if !market_calendar::is_us_equity_session(now.date_naive()) {
        return SessionPolicyState {
            is_open: false,
            label: "closed".to_string(),
            reason: Some("not_us_equity_session".to_string()),
        };
    }

    let (open, close) = regular_session_utc_bounds(now.date_naive());
    let time = now.time();
    if time < open {
        return SessionPolicyState {
            is_open: false,
            label: "pre_market".to_string(),
            reason: Some(format!("regular session opens at {open} UTC")),
        };
    }
    if time >= close {
        return SessionPolicyState {
            is_open: false,
            label: "after_hours".to_string(),
            reason: Some(format!("regular session closed at {close} UTC")),
        };
    }
    SessionPolicyState {
        is_open: true,
        label: "regular".to_string(),
        reason: None,
    }
}

fn regular_session_utc_bounds(day: NaiveDate) -> (NaiveTime, NaiveTime) {
    if is_us_eastern_dst(day) {
        (
            NaiveTime::from_hms_opt(13, 30, 0).expect("valid time"),
            NaiveTime::from_hms_opt(20, 0, 0).expect("valid time"),
        )
    } else {
        (
            NaiveTime::from_hms_opt(14, 30, 0).expect("valid time"),
            NaiveTime::from_hms_opt(21, 0, 0).expect("valid time"),
        )
    }
}

fn is_us_eastern_dst(day: NaiveDate) -> bool {
    let start = nth_weekday(day.year(), 3, Weekday::Sun, 2);
    let end = nth_weekday(day.year(), 11, Weekday::Sun, 1);
    day >= start && day < end
}

fn nth_weekday(year: i32, month: u32, weekday: Weekday, nth: u32) -> NaiveDate {
    let first = NaiveDate::from_ymd_opt(year, month, 1).expect("valid month");
    let offset = (7 + weekday.num_days_from_monday() as i64
        - first.weekday().num_days_from_monday() as i64)
        % 7;
    first + ChronoDuration::days(offset + 7 * (nth as i64 - 1))
}

#[derive(Debug, Clone)]
struct TargetIntent {
    side: TargetSide,
    rationale: String,
    reason_codes: Vec<String>,
}

fn common_blocked_reasons(input: &StrategyEvaluationInput) -> Vec<String> {
    let mut reasons = Vec::new();
    let Some(permission) = input.permission.as_ref() else {
        reasons.push("permission_missing".to_string());
        return reasons;
    };

    if permission.strategy_id != input.definition.strategy_id
        || permission.strategy_version != input.definition.strategy_version
    {
        reasons.push("permission_strategy_mismatch".to_string());
    }
    if input.definition.status != "shadow" {
        reasons.push("strategy_not_shadow".to_string());
    }
    if permission.status != "approved" {
        reasons.push("permission_not_approved".to_string());
    }
    if permission.environment_scope != "shadow" {
        reasons.push("environment_not_shadow".to_string());
    }
    if permission.manual_freeze {
        reasons.push("permission_frozen".to_string());
    }
    if permission
        .expires_at
        .is_some_and(|expires_at| expires_at <= input.now)
    {
        reasons.push("permission_expired".to_string());
    }

    let Some(technical) = input.features.technical.as_ref() else {
        reasons.push("technical_missing".to_string());
        return reasons;
    };
    let max_age_days = config_i64(&input.definition.config, "max_bar_age_days", 5).max(1);
    if input.now - technical.as_of > ChronoDuration::days(max_age_days) {
        reasons.push("technical_stale".to_string());
    }
    if technical.state == "unknown" || technical.entry_stance == "wait_data" {
        reasons.push("technical_invalid_state".to_string());
    }
    reasons
}

fn target_intent(input: &StrategyEvaluationInput) -> Result<TargetIntent, Vec<String>> {
    let family = input.definition.family.as_str();
    match family {
        "technical_timing" => technical_target(input),
        "thesis_timing" => thesis_target(input),
        _ => Err(vec!["strategy_family_unsupported".to_string()]),
    }
}

fn technical_target(input: &StrategyEvaluationInput) -> Result<TargetIntent, Vec<String>> {
    let technical = input
        .features
        .technical
        .as_ref()
        .ok_or_else(|| vec!["technical_missing".to_string()])?;
    if stance_list_contains(
        &input.definition.config,
        "long_entry_stances",
        &technical.entry_stance,
    ) {
        return long_target(
            input,
            format!("technical_long_{}", technical.entry_stance),
            format!(
                "{} technical state with {} entry stance.",
                technical.state, technical.entry_stance
            ),
        );
    }
    Ok(TargetIntent {
        side: TargetSide::Flat,
        rationale: format!(
            "{} entry stance does not permit shadow entry.",
            technical.entry_stance
        ),
        reason_codes: vec![format!("technical_flat_{}", technical.entry_stance)],
    })
}

fn thesis_target(input: &StrategyEvaluationInput) -> Result<TargetIntent, Vec<String>> {
    let Some(thesis) = input.features.thesis.as_ref() else {
        return Ok(TargetIntent {
            side: TargetSide::Flat,
            rationale: "No active thesis is available for thesis-timing entry.".to_string(),
            reason_codes: vec!["thesis_missing_flat".to_string()],
        });
    };
    if !stance_list_contains(
        &input.definition.config,
        "actionable_thesis_states",
        &thesis.state,
    ) {
        return Ok(TargetIntent {
            side: TargetSide::Flat,
            rationale: format!("Thesis state {} is not actionable for entry.", thesis.state),
            reason_codes: vec!["thesis_not_actionable".to_string()],
        });
    }
    if !thesis
        .direction
        .as_deref()
        .is_some_and(is_bullish_direction)
    {
        return Ok(TargetIntent {
            side: TargetSide::Flat,
            rationale: "Thesis direction is not bullish.".to_string(),
            reason_codes: vec!["thesis_not_bullish".to_string()],
        });
    }

    let technical = input
        .features
        .technical
        .as_ref()
        .ok_or_else(|| vec!["technical_missing".to_string()])?;
    if stance_list_contains(
        &input.definition.config,
        "long_entry_stances",
        &technical.entry_stance,
    ) {
        return long_target(
            input,
            "thesis_actionable_bullish".to_string(),
            format!(
                "Bullish actionable thesis and {} technical entry stance.",
                technical.entry_stance
            ),
        );
    }
    Ok(TargetIntent {
        side: TargetSide::Flat,
        rationale: format!(
            "Bullish thesis is actionable, but technical entry stance is {}.",
            technical.entry_stance
        ),
        reason_codes: vec!["thesis_waiting_for_timing".to_string()],
    })
}

fn long_target(
    input: &StrategyEvaluationInput,
    reason_code: String,
    rationale: String,
) -> Result<TargetIntent, Vec<String>> {
    let Some(permission) = input.permission.as_ref() else {
        return Err(vec!["permission_missing".to_string()]);
    };
    if permission.instrument_scope == "equity_short_only" {
        return Err(vec!["instrument_scope_blocks_long".to_string()]);
    }
    Ok(TargetIntent {
        side: TargetSide::Long,
        rationale,
        reason_codes: vec![reason_code],
    })
}

fn target_weight(input: &StrategyEvaluationInput) -> f64 {
    input
        .permission
        .as_ref()
        .and_then(|p| p.max_allocation_pct)
        .unwrap_or_else(|| config_f64(&input.definition.config, "default_weight_pct", 0.05))
        .clamp(0.0, 1.0)
}

fn latest_matches_target(
    latest: Option<&LatestDesiredPosition>,
    side: TargetSide,
    weight: Option<f64>,
) -> bool {
    let Some(latest) = latest else {
        return false;
    };
    latest.target_side == side && option_f64_close(latest.target_weight_pct, weight)
}

fn option_f64_close(left: Option<f64>, right: Option<f64>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => (left - right).abs() <= 0.000_001,
        (None, None) => true,
        (Some(left), None) | (None, Some(left)) => left.abs() <= 0.000_001,
    }
}

fn blocked_decision(
    input: &StrategyEvaluationInput,
    blocked_reasons: Vec<String>,
) -> StrategyDecision {
    let validation = validation_plan(input, TargetSide::Flat);
    StrategyDecision {
        kind: StrategyDecisionKind::Blocked,
        target_side: None,
        target_weight_pct: None,
        rationale: "Strategy evaluation blocked before desired exposure changed.".to_string(),
        reason_codes: vec![],
        feature_snapshot: feature_snapshot(input, None, None, &[], &blocked_reasons),
        signal_ref: signal_ref(input, &validation),
        blocked_reasons,
        validation,
    }
}

fn validation_plan(input: &StrategyEvaluationInput, target_side: TargetSide) -> ValidationPlan {
    let horizon_days = config_i64(&input.definition.config, "validation_horizon_days", 20).max(1);
    let previous_side = input.latest_desired.as_ref().map(|d| d.target_side);
    ValidationPlan {
        forward_only: true,
        horizon_days,
        evaluation_due_at: input.now + ChronoDuration::days(horizon_days),
        churn_event: previous_side.is_some_and(|side| side != target_side),
    }
}

fn config_i64(config: &Value, key: &str, default: i64) -> i64 {
    config.get(key).and_then(Value::as_i64).unwrap_or(default)
}

fn config_f64(config: &Value, key: &str, default: f64) -> f64 {
    config.get(key).and_then(Value::as_f64).unwrap_or(default)
}

fn stance_list_contains(config: &Value, key: &str, value: &str) -> bool {
    config
        .get(key)
        .and_then(Value::as_array)
        .is_some_and(|items| items.iter().any(|item| item.as_str() == Some(value)))
}

fn is_bullish_direction(direction: &str) -> bool {
    matches!(
        direction.to_ascii_lowercase().as_str(),
        "bullish" | "long" | "up" | "positive"
    )
}

fn signal_ref(input: &StrategyEvaluationInput, validation: &ValidationPlan) -> Value {
    json!({
        "source": "shadow_strategy_runner",
        "mode": "shadow",
        "strategy_id": input.definition.strategy_id,
        "strategy_version": input.definition.strategy_version,
        "strategy_config_hash": input.definition.config_hash,
        "family": input.definition.family,
        "forward_validation": validation,
    })
}

fn feature_snapshot(
    input: &StrategyEvaluationInput,
    target_side: Option<TargetSide>,
    target_weight_pct: Option<f64>,
    reason_codes: &[String],
    blocked_reasons: &[String],
) -> Value {
    let permission = input.permission.as_ref().map(|p| {
        json!({
            "permission_id": p.permission_id,
            "symbol": p.symbol,
            "status": p.status,
            "instrument_scope": p.instrument_scope,
            "environment_scope": p.environment_scope,
            "manual_freeze": p.manual_freeze,
            "expires_at": p.expires_at,
            "max_allocation_pct": p.max_allocation_pct,
            "max_notional_usd": p.max_notional_usd,
            "max_quantity": p.max_quantity,
        })
    });
    let technical = input.features.technical.as_ref().map(|t| {
        json!({
            "as_of": t.as_of,
            "state": t.state,
            "setup_kind": t.setup_kind,
            "entry_stance": t.entry_stance,
            "summary": t.summary,
            "close": t.close,
        })
    });
    let thesis = input.features.thesis.as_ref().map(|t| {
        json!({
            "thesis_id": t.thesis_id,
            "state": t.state,
            "direction": t.direction,
            "freshness_status": t.freshness_status,
            "freshness_score": t.freshness_score,
            "updated_at": t.updated_at,
        })
    });
    json!({
        "as_of": input.now,
        "symbol": input.features.symbol,
        "strategy": {
            "strategy_id": input.definition.strategy_id,
            "strategy_version": input.definition.strategy_version,
            "family": input.definition.family,
            "display_name": input.definition.display_name,
            "status": input.definition.status,
            "config_hash": input.definition.config_hash,
            "config": input.definition.config,
        },
        "permission": permission,
        "technical": technical,
        "thesis": thesis,
        "target": {
            "side": target_side.map(TargetSide::as_str),
            "weight_pct": target_weight_pct,
            "reason_codes": reason_codes,
            "blocked_reasons": blocked_reasons,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use pretty_assertions::assert_eq;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 30, 14, 0, 0).unwrap()
    }

    fn definition(family: &str) -> StrategyDefinitionInput {
        let seed = builtin_strategy_definitions()
            .into_iter()
            .find(|s| s.family == family)
            .unwrap();
        StrategyDefinitionInput {
            strategy_id: seed.strategy_id.to_string(),
            strategy_version: seed.strategy_version.to_string(),
            family: seed.family.to_string(),
            display_name: seed.display_name.to_string(),
            status: seed.status.to_string(),
            config_hash: seed.config_hash(),
            config: seed.config,
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
            max_allocation_pct: Some(0.07),
            max_notional_usd: None,
            max_quantity: None,
        }
    }

    fn technical(entry_stance: &str, state: &str) -> TechnicalFeature {
        TechnicalFeature {
            as_of: now() - ChronoDuration::days(1),
            state: state.to_string(),
            setup_kind: "constructive_trend".to_string(),
            entry_stance: entry_stance.to_string(),
            summary: "constructive setup".to_string(),
            close: Some(100.0),
        }
    }

    fn features(entry_stance: &str, state: &str) -> StrategyFeatures {
        StrategyFeatures {
            symbol: "NVDA".to_string(),
            technical: Some(technical(entry_stance, state)),
            thesis: None,
        }
    }

    fn base_input(family: &str) -> StrategyEvaluationInput {
        StrategyEvaluationInput {
            definition: definition(family),
            permission: Some(permission()),
            latest_desired: None,
            features: features("constructive", "constructive"),
            now: now(),
        }
    }

    #[test]
    fn technical_timing_emits_long_for_constructive_shadow_permission() {
        let decision = evaluate_strategy(&base_input("technical_timing"));

        assert_eq!(decision.kind, StrategyDecisionKind::EmitDesired);
        assert_eq!(decision.target_side, Some(TargetSide::Long));
        assert_eq!(decision.target_weight_pct, Some(0.07));
        assert!(
            decision
                .reason_codes
                .iter()
                .any(|r| r == "technical_long_constructive")
        );
        assert!(decision.blocked_reasons.is_empty());
        assert_eq!(
            decision.feature_snapshot["strategy"]["config_hash"],
            base_input("technical_timing").definition.config_hash
        );
        assert_eq!(decision.signal_ref["mode"], "shadow");
        assert!(decision.validation.forward_only);
    }

    #[test]
    fn unchanged_target_is_no_change_not_a_new_desired_position() {
        let mut input = base_input("technical_timing");
        input.latest_desired = Some(LatestDesiredPosition {
            desired_position_id: Uuid::nil(),
            target_side: TargetSide::Long,
            target_weight_pct: Some(0.07),
            emitted_at: now() - ChronoDuration::days(1),
        });

        let decision = evaluate_strategy(&input);

        assert_eq!(decision.kind, StrategyDecisionKind::NoChange);
        assert_eq!(decision.target_side, Some(TargetSide::Long));
        assert!(!decision.validation.churn_event);
    }

    #[test]
    fn frozen_expired_or_non_shadow_permissions_are_blocked() {
        let mut input = base_input("technical_timing");
        input.permission.as_mut().unwrap().manual_freeze = true;
        assert!(
            evaluate_strategy(&input)
                .blocked_reasons
                .iter()
                .any(|r| r == "permission_frozen")
        );

        let mut input = base_input("technical_timing");
        input.permission.as_mut().unwrap().expires_at = Some(now() - ChronoDuration::seconds(1));
        assert!(
            evaluate_strategy(&input)
                .blocked_reasons
                .iter()
                .any(|r| r == "permission_expired")
        );

        let mut input = base_input("technical_timing");
        input.permission.as_mut().unwrap().environment_scope = "paper".to_string();
        assert!(
            evaluate_strategy(&input)
                .blocked_reasons
                .iter()
                .any(|r| r == "environment_not_shadow")
        );
    }

    #[test]
    fn stale_or_unknown_technical_state_blocks_changes() {
        let mut input = base_input("technical_timing");
        input.features.technical.as_mut().unwrap().as_of = now() - ChronoDuration::days(9);
        assert!(
            evaluate_strategy(&input)
                .blocked_reasons
                .iter()
                .any(|r| r == "technical_stale")
        );

        let mut input = base_input("technical_timing");
        input.features = features("wait_data", "unknown");
        assert!(
            evaluate_strategy(&input)
                .blocked_reasons
                .iter()
                .any(|r| r == "technical_invalid_state")
        );
    }

    #[test]
    fn thesis_timing_requires_actionable_bullish_thesis_for_long() {
        let mut input = base_input("thesis_timing");
        input.permission.as_mut().unwrap().strategy_id = "thesis_timing".to_string();
        input.features.thesis = Some(ThesisFeature {
            thesis_id: Uuid::nil(),
            state: "actionable".to_string(),
            direction: Some("bullish".to_string()),
            freshness_status: Some("fresh".to_string()),
            freshness_score: Some(0.9),
            updated_at: now() - ChronoDuration::days(1),
        });

        let decision = evaluate_strategy(&input);

        assert_eq!(decision.kind, StrategyDecisionKind::EmitDesired);
        assert_eq!(decision.target_side, Some(TargetSide::Long));
        assert!(
            decision
                .reason_codes
                .iter()
                .any(|r| r == "thesis_actionable_bullish")
        );

        input.features.thesis.as_mut().unwrap().state = "armed".to_string();
        let decision = evaluate_strategy(&input);
        assert_eq!(decision.kind, StrategyDecisionKind::EmitDesired);
        assert_eq!(decision.target_side, Some(TargetSide::Flat));
        assert!(
            decision
                .reason_codes
                .iter()
                .any(|r| r == "thesis_not_actionable")
        );
    }

    #[test]
    fn missing_permission_blocks_strategy_evaluation() {
        let mut input = base_input("technical_timing");
        input.permission = None;

        let decision = evaluate_strategy(&input);

        assert_eq!(decision.kind, StrategyDecisionKind::Blocked);
        assert!(
            decision
                .blocked_reasons
                .iter()
                .any(|r| r == "permission_missing")
        );
    }

    #[test]
    fn missing_technical_features_become_missing_data_policy_state() {
        let mut input = base_input("technical_timing");
        input.features.technical = None;

        let state = data_freshness_policy_state(&input);

        assert_eq!(state.status, "missing");
        assert!(state.stale);
        assert_eq!(state.latest_bar_at, None);
    }

    #[test]
    fn session_policy_tracks_regular_us_equity_hours() {
        let summer_open = Utc.with_ymd_and_hms(2026, 6, 30, 14, 0, 0).unwrap();
        let after_hours = Utc.with_ymd_and_hms(2026, 6, 30, 20, 30, 0).unwrap();
        let weekend = Utc.with_ymd_and_hms(2026, 7, 4, 16, 0, 0).unwrap();

        assert_eq!(session_policy_state(summer_open).label, "regular");
        assert_eq!(session_policy_state(after_hours).label, "after_hours");
        assert_eq!(session_policy_state(weekend).label, "closed");
    }
}
