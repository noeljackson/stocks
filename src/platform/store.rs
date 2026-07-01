//! Postgres access layer (sqlx pool + typed helpers).
//!
//! sqlx::query (not query!) — we keep the macro discipline off for v0 because
//! compile-time SQL checking requires a live DB at build time (DATABASE_URL
//! must be reachable). We can flip to the macro form later by setting
//! SQLX_OFFLINE=true + checking in the sqlx-data.json.

use anyhow::{Context, Result};
use chrono::{DateTime, Duration as ChronoDuration, NaiveDate, Utc};
use serde::Serialize;
use sqlx::{
    Row,
    postgres::{PgPool, PgPoolOptions},
};
use std::time::Duration;

use crate::automation::{
    AllocationLimits, AutomationControlState, AutomationStage, AutomationStrategyCandidate,
    BlockedProofWrite, BrokerPolicyState, BuiltinStrategyDefinition, DesiredPositionReceipt,
    DesiredPositionWrite, LatestDesiredPosition, PromotionApproval, ReadinessInput,
    ReadinessMetrics, ReadinessThresholds, ReconciliationInput, SimulatedPosition,
    SimulatedReconciliationReceipt, SimulationConfig, SleeveAllocation, SleevePolicyState,
    StrategyDefinitionInput, TargetSide, TradePermissionInput, builtin_strategy_definitions,
    evaluate_readiness, manual_stage_change, reconcile_simulated,
};
use crate::llm::prompts::{InvocationRecorder, InvocationRow};
use crate::platform::domain::{
    Alert, AlertKind, Condition, MarketStateRow, ThesisDetail, ThesisFreshnessComponent,
    ThesisSubstance, ThesisVersionEvent, TickerContextRow, TickerRow, Watchlist, WatchlistMember,
    WellFormedCondCounts,
};
use crate::platform::technical::{TechnicalBar, TechnicalState};
use crate::price_alerts::{
    PriceAlertEvent, PriceAlertRule, PriceAlertRuleInput, PriceAlertRulePatch, PriceTrigger,
    validate_rule_input, validate_rule_patch,
};
use crate::reflection::technical_timing;
use crate::thesis::substance::{self, Thesis as SubstanceInput};

#[derive(Clone)]
pub struct Store {
    pub pool: PgPool,
}

#[derive(Debug, Clone, Copy)]
pub struct IntradayBarCoverage {
    pub oldest: Option<DateTime<Utc>>,
    pub latest: Option<DateTime<Utc>>,
    pub bars: i64,
}

#[derive(Debug, Clone)]
struct ThesisFreshnessSummary {
    score: f64,
    status: String,
    confidence_cap: Option<String>,
    penalties: Vec<String>,
    components: Vec<ThesisFreshnessComponent>,
}

#[derive(Debug, Clone, Copy)]
struct FreshnessThresholds {
    fresh: ChronoDuration,
    stale: ChronoDuration,
    old: ChronoDuration,
}

#[derive(Debug, Clone)]
struct BrainJournalDraft {
    journal_date: NaiveDate,
    category: String,
    source_kind: String,
    source_id: String,
    event_key: String,
    symbol: Option<String>,
    brain_thesis_id: Option<uuid::Uuid>,
    thesis_id: Option<uuid::Uuid>,
    title: String,
    summary: String,
    importance: i32,
    occurred_at: DateTime<Utc>,
    source_ref: serde_json::Value,
}

#[derive(Debug, Clone)]
struct DerivedRefreshTask {
    id: i64,
    generation: i32,
    target_kind: String,
    target_id: String,
    symbol: Option<String>,
    reason: String,
    dependency_kind: String,
    dependency_id: Option<String>,
}

#[derive(Debug, Clone)]
struct PromotionApprovalRow {
    approval_id: uuid::Uuid,
    approval: PromotionApproval,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct JournalTechnicalRead {
    stance: String,
    label: String,
    reason: String,
    trend: String,
    momentum: String,
    relative_strength: String,
    volume: String,
    volatility: String,
    source: String,
}

#[derive(Debug, Clone, Default)]
struct SymbolWorkflowFacts {
    symbol: String,
    active_tier: Option<i32>,
    in_pool: bool,
    context_version: Option<i32>,
    evidence_item_count: i64,
    open_evidence: i64,
    blocking_evidence: i64,
    due_source_tasks: i64,
    latest_thesis_id: Option<uuid::Uuid>,
    thesis_state: Option<String>,
    thesis_direction: Option<String>,
    thesis_reason: Option<String>,
    decline_count: i64,
    decline_reason: Option<String>,
    decision_count: i64,
    pending_manual_fill_count: i64,
    open_position_count: i64,
    automation_permission_count: i64,
    automation_approved_count: i64,
    automation_pending_count: i64,
    automation_frozen_count: i64,
    open_attention_count: i64,
    candidate_attention_id: Option<i64>,
    review_packet_attention_id: Option<i64>,
    attention_items: serde_json::Value,
}

#[derive(Debug, Clone)]
struct SymbolWorkflowDecision {
    state: &'static str,
    state_label: &'static str,
    tone: &'static str,
    reason: String,
    primary_kind: &'static str,
    primary_label: &'static str,
    primary_detail: String,
    review_packet_attention_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
struct SymbolWorkflowAction {
    kind: &'static str,
    label: &'static str,
    detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    attention_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
struct SymbolWorkflowStep {
    key: &'static str,
    label: &'static str,
    value: String,
    action: &'static str,
    tone: &'static str,
}

fn age_component(
    name: &str,
    now: DateTime<Utc>,
    last_at: Option<DateTime<Utc>>,
    thresholds: FreshnessThresholds,
    penalty: &str,
) -> (ThesisFreshnessComponent, Option<String>) {
    let Some(last_at) = last_at else {
        return (
            ThesisFreshnessComponent {
                name: name.to_string(),
                status: "missing".to_string(),
                score: 0.3,
                last_at: None,
                reason: format!("{name} has no observed timestamp"),
            },
            Some(format!("{name}: missing")),
        );
    };
    let age = now
        .signed_duration_since(last_at)
        .max(ChronoDuration::zero());
    let (status, score, reason, component_penalty) = if age <= thresholds.fresh {
        (
            "fresh",
            1.0,
            format!("{name} checked within freshness target"),
            None,
        )
    } else if age <= thresholds.stale {
        (
            "aging",
            0.8,
            format!("{name} is outside ideal freshness"),
            None,
        )
    } else if age <= thresholds.old {
        (
            "stale",
            0.6,
            format!("{name} is stale"),
            Some(format!("{name}: {penalty}")),
        )
    } else {
        (
            "old",
            0.4,
            format!("{name} is too old for high-confidence promotion"),
            Some(format!("{name}: {penalty}")),
        )
    };
    (
        ThesisFreshnessComponent {
            name: name.to_string(),
            status: status.to_string(),
            score,
            last_at: Some(last_at),
            reason,
        },
        component_penalty,
    )
}

fn news_component(
    recent_news_14d: i64,
    last_at: Option<DateTime<Utc>>,
) -> (ThesisFreshnessComponent, Option<String>) {
    let (status, score, reason, penalty) = if recent_news_14d >= 3 {
        (
            "fresh",
            1.0,
            format!("{recent_news_14d} recent articles in the last 14 days"),
            None,
        )
    } else if recent_news_14d > 0 {
        (
            "thin",
            0.7,
            format!("only {recent_news_14d} recent article(s) in the last 14 days"),
            Some("news: narrative evidence is thin".to_string()),
        )
    } else {
        (
            "missing",
            0.5,
            "no recent articles in the last 14 days".to_string(),
            Some("news: cannot rely on sentiment-shift evidence".to_string()),
        )
    };
    (
        ThesisFreshnessComponent {
            name: "news".to_string(),
            status: status.to_string(),
            score,
            last_at,
            reason,
        },
        penalty,
    )
}

fn freshness_status(score: f64) -> String {
    if score >= 0.85 {
        "fresh".to_string()
    } else if score >= 0.50 {
        "stale".to_string()
    } else {
        "limited".to_string()
    }
}

fn confidence_cap(score: f64, components: &[ThesisFreshnessComponent]) -> Option<String> {
    if score < 0.50 {
        return Some("low".to_string());
    }
    if score < 0.85
        || components
            .iter()
            .any(|c| c.name == "context" && matches!(c.status.as_str(), "stale" | "old"))
    {
        return Some("medium".to_string());
    }
    None
}

fn journal_attention_category(kind: &str, severity: &str) -> (&'static str, i32) {
    match kind {
        "candidate_review" | "thesis_review" => ("research", 70),
        "thesis_actionable" | "invalidation_hit" | "risk_review" => ("changed", 85),
        "context_stale" | "thesis_incomplete" => ("blocked", 70),
        _ if severity == "blocked" => ("blocked", 75),
        _ if severity == "decision" => ("changed", 80),
        _ => ("curious", 55),
    }
}

fn journal_source_task_category(
    state: &str,
    result: Option<&str>,
    priority: &str,
) -> (&'static str, i32) {
    match (state, result) {
        ("satisfied", Some("rows_seen")) => ("changed", 78),
        ("failed" | "blocked" | "rate_limited", _) => ("blocked", 78),
        ("no_rows", _) => ("curious", 55),
        ("queued" | "fetching", _) if matches!(priority, "high" | "blocking") => ("research", 62),
        _ => ("curious", 45),
    }
}

fn journal_thesis_state_importance(to_state: &str) -> i32 {
    match to_state {
        "actionable" | "armed" | "position_open" | "exiting" => 90,
        "building_conviction" => 78,
        "closed" | "disqualified" => 72,
        _ => 65,
    }
}

fn journal_thesis_state_score(state: Option<&str>) -> i32 {
    match state.unwrap_or_default() {
        "actionable" => 44,
        "armed" => 40,
        "building_conviction" => 34,
        "position_open" => 28,
        "forming" => 22,
        _ => 0,
    }
}

fn journal_technical_score(technical_state: Option<&str>, entry_stance: Option<&str>) -> i32 {
    match (
        technical_state.unwrap_or_default(),
        entry_stance.unwrap_or_default(),
    ) {
        ("constructive", _) => 24,
        ("base_building", _) | (_, "wait_breakout") => 22,
        (_, "constructive") => 18,
        ("extended", _) | (_, "avoid_chase") => -18,
        ("deteriorating", _) | (_, "avoid") => -30,
        ("unknown", _) | (_, "wait_data") => -6,
        _ => 0,
    }
}

fn journal_freshness_score(freshness: Option<&str>) -> i32 {
    match freshness.unwrap_or_default() {
        "fresh" => 15,
        "stale" => -8,
        "missing" => -14,
        "blocked" => -24,
        _ => 0,
    }
}

fn journal_direction_is_bullish(direction: Option<&str>) -> bool {
    matches!(
        direction.unwrap_or_default(),
        "up" | "bull" | "bullish" | "long" | "risk_on"
    )
}

fn journal_direction_is_bearish(direction: Option<&str>) -> bool {
    matches!(
        direction.unwrap_or_default(),
        "down" | "bear" | "bearish" | "short" | "risk_off"
    )
}

fn journal_waits_for_setup(technical_state: Option<&str>, entry_stance: Option<&str>) -> bool {
    matches!(technical_state.unwrap_or_default(), "extended")
        || matches!(entry_stance.unwrap_or_default(), "avoid_chase")
}

fn technical_component(cross: &serde_json::Value, key: &str, fallback: Option<&str>) -> String {
    cross
        .get(key)
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .or(fallback)
        .unwrap_or("unknown")
        .replace('_', " ")
}

fn technical_cross_value(cross: &serde_json::Value, key: &str) -> Option<String> {
    cross
        .get(key)
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
}

fn journal_technical_stance(
    technical_state: Option<&str>,
    setup_kind: Option<&str>,
    entry_stance: Option<&str>,
    buy_timing: Option<&str>,
    trend_state: Option<&str>,
) -> (&'static str, &'static str) {
    let values = [
        buy_timing,
        setup_kind,
        technical_state,
        entry_stance,
        trend_state,
    ];
    let has = |needles: &[&str]| {
        values
            .iter()
            .flatten()
            .any(|value| needles.iter().any(|needle| value == needle))
    };

    if has(&["pullback_reversal", "reversal_confirming", "starter_ok"]) {
        ("starter_ok", "starter OK")
    } else if has(&["pullback_watch", "wait_reversal"]) {
        ("wait_reversal", "wait reversal")
    } else if has(&["extended_chase", "avoid_chase", "extended", "extended_run"]) {
        ("avoid_chase", "avoid chase")
    } else if has(&["avoid_breakdown", "breakdown", "deteriorating", "avoid"]) {
        ("avoid_breakdown", "avoid breakdown")
    } else if has(&["constructive", "constructive_trend", "uptrend"]) {
        ("constructive", "constructive")
    } else if has(&["base_building", "wait_breakout"]) {
        ("wait_reversal", "wait reversal")
    } else {
        ("wait_data", "wait data")
    }
}

fn journal_technical_read(
    technical_state: Option<&str>,
    setup_kind: Option<&str>,
    entry_stance: Option<&str>,
    cross: &serde_json::Value,
) -> JournalTechnicalRead {
    let buy_timing = technical_cross_value(cross, "buy_timing");
    let trend_state = technical_cross_value(cross, "trend_state");
    let (stance, label) = journal_technical_stance(
        technical_state,
        setup_kind,
        entry_stance,
        buy_timing.as_deref(),
        trend_state.as_deref(),
    );
    let trend = technical_component(cross, "trend_state", technical_state);
    let momentum = technical_component(cross, "momentum_state", entry_stance);
    let relative_strength = technical_component(cross, "relative_strength_state", None);
    let volume = technical_component(cross, "volume_state", None);
    let volatility = technical_component(cross, "volatility_state", None);
    let source = if cross.get("buy_timing").is_some() || cross.get("trend_state").is_some() {
        "cross_technical"
    } else {
        "technical_summary"
    };

    JournalTechnicalRead {
        stance: stance.to_string(),
        label: label.to_string(),
        reason: format!(
            "trend {trend}; momentum {momentum}; relative strength {relative_strength}; volume {volume}; volatility {volatility}"
        ),
        trend,
        momentum,
        relative_strength,
        volume,
        volatility,
        source: source.to_string(),
    }
}

fn journal_candidate_score(
    state: Option<&str>,
    direction: Option<&str>,
    technical_state: Option<&str>,
    entry_stance: Option<&str>,
    freshness: Option<&str>,
    tier: i32,
    domain_fit: Option<f64>,
) -> i32 {
    let mut score = journal_thesis_state_score(state)
        + journal_technical_score(technical_state, entry_stance)
        + journal_freshness_score(freshness)
        + match tier {
            1 => 12,
            2 => 7,
            _ => 3,
        }
        + domain_fit.map_or(0, |v| (v / 10.0).round() as i32);

    if journal_direction_is_bullish(direction) {
        score += 8;
    } else if journal_direction_is_bearish(direction) {
        score -= 12;
    }
    score.clamp(0, 100)
}

fn journal_symbol_blockers(ticker: &TickerRow) -> Vec<String> {
    let mut blockers = Vec::new();
    if ticker.blocking_evidence > 0 {
        blockers.push(format!(
            "{} blocking evidence requirement(s)",
            ticker.blocking_evidence
        ));
    }
    if ticker.due_source_tasks > 0 {
        blockers.push(format!("{} due source task(s)", ticker.due_source_tasks));
    }
    match ticker.freshness_status.as_str() {
        "blocked" => blockers.push("brain inputs blocked".to_string()),
        "missing" => blockers.push("brain inputs missing".to_string()),
        "stale" => blockers.push("brain inputs stale".to_string()),
        _ => {}
    }
    blockers
}

fn journal_trade_desk_item(ticker: &TickerRow, score: i32, stance: &str) -> serde_json::Value {
    let state = ticker.thesis_state.as_deref().unwrap_or("no thesis");
    let direction = ticker.thesis_direction.as_deref().unwrap_or("no direction");
    let technical = ticker
        .technical_state
        .as_deref()
        .unwrap_or("unknown technicals");
    let entry = ticker.entry_stance.as_deref().unwrap_or("wait_data");
    let technical_read = journal_technical_read(
        ticker.technical_state.as_deref(),
        ticker.technical_setup_kind.as_deref(),
        ticker.entry_stance.as_deref(),
        &ticker.technical_cross,
    );
    let blockers = journal_symbol_blockers(ticker);
    let why_now = match stance {
        "consider" => format!(
            "{state} {direction} thesis with {technical} setup and {} inputs",
            ticker.freshness_status
        ),
        "wait" => format!("{state} {direction} thesis exists, but timing is {entry}"),
        "avoid" => format!(
            "{} / {} is not a clean long-entry read",
            direction, technical
        ),
        _ => {
            if ticker.open_theses == 0 {
                "no open thesis yet; research must come before a trade decision".to_string()
            } else {
                format!("{state} thesis needs more evidence before a decision")
            }
        }
    };
    let why_not = if blockers.is_empty() {
        match stance {
            "consider" => "risk overlay and human review still required before any position",
            "wait" => "setup quality is not clean enough for an entry today",
            "avoid" => "direction, setup, or freshness argues against adding risk",
            _ => "not enough thesis substance for a trade action",
        }
        .to_string()
    } else {
        blockers.join("; ")
    };
    let risk_note = match stance {
        "consider" if blockers.is_empty() => "eligible for review packet; size remains risk-gated",
        "wait" => "do not chase; wait for setup or fresh evidence",
        "avoid" => "avoid adding exposure until thesis/setup/freshness improves",
        _ => "research-only; not a trade proposal",
    };

    serde_json::json!({
        "symbol": ticker.symbol.clone(),
        "tier": ticker.tier,
        "score": score,
        "stance": stance,
        "thesis_id": ticker.latest_thesis_id,
        "thesis_state": ticker.thesis_state.clone(),
        "thesis_direction": ticker.thesis_direction.clone(),
        "technical_state": ticker.technical_state.clone(),
        "technical_setup_kind": ticker.technical_setup_kind.clone(),
        "entry_stance": ticker.entry_stance.clone(),
        "technical_read": technical_read,
        "technical_panel_path": format!("/symbol/{}?p=technical", ticker.symbol),
        "technical_pct_vs_200d": ticker.technical_pct_vs_200d,
        "freshness_status": ticker.freshness_status.clone(),
        "open_attention": ticker.open_attention,
        "review_packet_attention_id": ticker.review_packet_attention_id,
        "open_evidence": ticker.open_evidence,
        "blocking_evidence": ticker.blocking_evidence,
        "due_source_tasks": ticker.due_source_tasks,
        "parent_themes": ticker.parent_themes.clone(),
        "why_now": why_now,
        "why_not": why_not,
        "risk_note": risk_note,
        "blockers": blockers,
    })
}

fn journal_label(value: &str) -> String {
    value.replace('_', " ")
}

fn journal_event_key(
    source_kind: &str,
    source_id: impl std::fmt::Display,
    at: DateTime<Utc>,
) -> String {
    format!("{source_kind}:{source_id}:{}", at.timestamp_millis())
}

fn parse_derived_refresh_day(target_id: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(target_id, "%Y-%m-%d")
        .with_context(|| format!("invalid derived refresh day {target_id}"))
}

fn workflow_direction_label(direction: Option<&str>) -> &'static str {
    match direction {
        Some("up") => "bull",
        Some("down") => "bear",
        Some("neutral") => "neutral",
        _ => "none",
    }
}

fn workflow_count(value: i64, singular: &str, plural: &str) -> String {
    if value == 1 {
        format!("1 {singular}")
    } else {
        format!("{value} {plural}")
    }
}

fn workflow_status_step(facts: &SymbolWorkflowFacts, decision: &SymbolWorkflowDecision) -> String {
    if let Some(tier) = facts.active_tier {
        return format!("Universe T{tier}");
    }
    if decision.state == "nominated" {
        return "nominated".to_string();
    }
    if facts.in_pool {
        return "Discovery Pool".to_string();
    }
    "not active".to_string()
}

fn workflow_attention_step(facts: &SymbolWorkflowFacts) -> String {
    if facts.open_attention_count > 0 {
        workflow_count(facts.open_attention_count, "attention", "attention")
    } else {
        "no attention".to_string()
    }
}

fn workflow_evidence_step(facts: &SymbolWorkflowFacts) -> String {
    if facts.blocking_evidence > 0 {
        workflow_count(
            facts.blocking_evidence,
            "blocking evidence",
            "blocking evidence",
        )
    } else if facts.open_evidence > 0 {
        workflow_count(facts.open_evidence, "open evidence", "open evidence")
    } else if facts.evidence_item_count > 0 {
        workflow_count(facts.evidence_item_count, "fact", "facts")
    } else if facts.due_source_tasks > 0 {
        workflow_count(
            facts.due_source_tasks,
            "due source task",
            "due source tasks",
        )
    } else {
        "evidence ready".to_string()
    }
}

fn workflow_thesis_step(facts: &SymbolWorkflowFacts, decision: &SymbolWorkflowDecision) -> String {
    if let Some(state) = facts.thesis_state.as_deref() {
        return format!(
            "{} · {}",
            journal_label(state),
            workflow_direction_label(facts.thesis_direction.as_deref())
        );
    }
    if decision.state == "nominated" {
        return "nominated".to_string();
    }
    if facts.decline_count > 0 {
        return "declined attempt".to_string();
    }
    "no thesis".to_string()
}

fn workflow_decision_step(facts: &SymbolWorkflowFacts) -> String {
    if facts.open_position_count > 0 {
        return workflow_count(facts.open_position_count, "open position", "open positions");
    }
    if facts.pending_manual_fill_count > 0 {
        return "manual fill needed".to_string();
    }
    if facts.decision_count > 0 {
        return workflow_count(facts.decision_count, "decision", "decisions");
    }
    "no decision".to_string()
}

fn workflow_automation_step(facts: &SymbolWorkflowFacts) -> String {
    if facts.automation_frozen_count > 0 {
        return workflow_count(facts.automation_frozen_count, "frozen", "frozen");
    }
    if facts.automation_approved_count > 0 {
        return workflow_count(facts.automation_approved_count, "approved", "approved");
    }
    if facts.automation_pending_count > 0 {
        return workflow_count(facts.automation_pending_count, "pending", "pending");
    }
    if facts.automation_permission_count > 0 {
        return workflow_count(
            facts.automation_permission_count,
            "permission",
            "permissions",
        );
    }
    "not approved".to_string()
}

fn symbol_workflow_steps(
    facts: &SymbolWorkflowFacts,
    decision: &SymbolWorkflowDecision,
) -> Vec<SymbolWorkflowStep> {
    vec![
        SymbolWorkflowStep {
            key: "status",
            label: "Status",
            value: workflow_status_step(facts, decision),
            action: "overview",
            tone: decision.tone,
        },
        SymbolWorkflowStep {
            key: "attention",
            label: "Attention",
            value: workflow_attention_step(facts),
            action: "attention",
            tone: if facts.open_attention_count > 0 {
                "actionable"
            } else {
                "muted"
            },
        },
        SymbolWorkflowStep {
            key: "evidence",
            label: "Evidence",
            value: workflow_evidence_step(facts),
            action: "evidence",
            tone: if facts.blocking_evidence > 0 {
                "blocked"
            } else if facts.open_evidence > 0 || facts.due_source_tasks > 0 {
                "monitoring"
            } else {
                "ready"
            },
        },
        SymbolWorkflowStep {
            key: "thesis",
            label: "Thesis",
            value: workflow_thesis_step(facts, decision),
            action: "thesis",
            tone: match facts.thesis_state.as_deref() {
                Some("actionable") | Some("armed") | Some("building_conviction") => "actionable",
                Some(_) => "monitoring",
                None if facts.decline_count > 0 => "declined",
                None => "muted",
            },
        },
        SymbolWorkflowStep {
            key: "automation",
            label: "Automation",
            value: workflow_automation_step(facts),
            action: "automation",
            tone: if facts.automation_frozen_count > 0 {
                "blocked"
            } else if facts.automation_approved_count > 0 {
                "tracking"
            } else if facts.automation_pending_count > 0 {
                "monitoring"
            } else {
                "muted"
            },
        },
        SymbolWorkflowStep {
            key: "decision",
            label: "Decision",
            value: workflow_decision_step(facts),
            action: "tracking",
            tone: if facts.open_position_count > 0 || facts.decision_count > 0 {
                "tracking"
            } else {
                "muted"
            },
        },
    ]
}

fn classify_symbol_workflow(facts: &SymbolWorkflowFacts) -> SymbolWorkflowDecision {
    if facts.active_tier.is_none()
        && facts.candidate_attention_id.is_some()
        && facts.latest_thesis_id.is_none()
    {
        let attention_id = facts.candidate_attention_id;
        return SymbolWorkflowDecision {
            state: "nominated",
            state_label: "Nominated, not active",
            tone: "candidate",
            reason: facts.candidate_reason(),
            primary_kind: "attention",
            primary_label: "Promote to Universe",
            primary_detail: "Open the review packet and choose Universe/watchlist destinations."
                .to_string(),
            review_packet_attention_id: attention_id,
        };
    }

    if facts.active_tier.is_none() {
        return SymbolWorkflowDecision {
            state: "pool_candidate",
            state_label: if facts.in_pool {
                "Pool candidate"
            } else {
                "Not active"
            },
            tone: "candidate",
            reason: if facts.in_pool {
                "This symbol is in the discovery pool but not the active Universe.".to_string()
            } else {
                "This symbol is not in the active Universe yet.".to_string()
            },
            primary_kind: "promote",
            primary_label: "Promote to Universe",
            primary_detail:
                "Add this symbol to the monitored Universe before scheduled cognition runs."
                    .to_string(),
            review_packet_attention_id: None,
        };
    }

    if facts.blocking_evidence > 0 {
        return SymbolWorkflowDecision {
            state: "evidence_blocked",
            state_label: "Evidence blocked",
            tone: "blocked",
            reason: format!(
                "{} must be resolved before thesis work is reliable.",
                workflow_count(
                    facts.blocking_evidence,
                    "blocking evidence item",
                    "blocking evidence items"
                )
            ),
            primary_kind: "research",
            primary_label: "Start research",
            primary_detail: "Queue source tasks and refresh evidence for this symbol.".to_string(),
            review_packet_attention_id: facts.review_packet_attention_id,
        };
    }

    if facts.context_version.is_none() {
        return SymbolWorkflowDecision {
            state: "context_missing",
            state_label: "Context missing",
            tone: "blocked",
            reason: "Context is missing; cognition needs source-backed context before a thesis."
                .to_string(),
            primary_kind: "research",
            primary_label: "Start research",
            primary_detail: "Queue context, evidence, and thesis work for this symbol.".to_string(),
            review_packet_attention_id: facts.review_packet_attention_id,
        };
    }

    if facts.open_position_count > 0 {
        return SymbolWorkflowDecision {
            state: "position_tracking",
            state_label: "Position tracking",
            tone: "tracking",
            reason: "A position is open; conditions and exits matter now.".to_string(),
            primary_kind: "tracking",
            primary_label: "Track position",
            primary_detail: "Open the decision and position history for this symbol.".to_string(),
            review_packet_attention_id: facts.review_packet_attention_id,
        };
    }

    if facts.pending_manual_fill_count > 0 {
        return SymbolWorkflowDecision {
            state: "decision_recorded",
            state_label: "Fill needed",
            tone: "actionable",
            reason: "A confirmed decision exists, but no open position is recorded yet."
                .to_string(),
            primary_kind: "decision",
            primary_label: "Record fill",
            primary_detail: "Open the decision drawer and record the manual fill.".to_string(),
            review_packet_attention_id: facts.review_packet_attention_id,
        };
    }

    if facts.decision_count > 0 {
        return SymbolWorkflowDecision {
            state: "decision_recorded",
            state_label: "Decision recorded",
            tone: "tracking",
            reason: "A decision exists; review replay and follow-up conditions.".to_string(),
            primary_kind: "tracking",
            primary_label: "Track decision",
            primary_detail: "Open decision history and replay for this symbol.".to_string(),
            review_packet_attention_id: facts.review_packet_attention_id,
        };
    }

    if let Some(state) = facts.thesis_state.as_deref() {
        let actionable = matches!(state, "actionable" | "armed" | "building_conviction");
        return SymbolWorkflowDecision {
            state: if actionable {
                "thesis_actionable"
            } else {
                "thesis_monitoring"
            },
            state_label: if actionable {
                "Actionable thesis"
            } else {
                "Monitoring thesis"
            },
            tone: if actionable {
                "actionable"
            } else {
                "monitoring"
            },
            reason: facts.thesis_reason.clone().unwrap_or_else(|| {
                "Review the current thesis and source-backed evidence.".to_string()
            }),
            primary_kind: if actionable { "decision" } else { "thesis" },
            primary_label: if actionable {
                "Record decision"
            } else {
                "Review thesis"
            },
            primary_detail: if actionable {
                "Open the decision drawer prefilled from the current thesis.".to_string()
            } else {
                "Open the thesis tab for evidence, risks, and conditions.".to_string()
            },
            review_packet_attention_id: facts.review_packet_attention_id,
        };
    }

    if facts.decline_count > 0 {
        return SymbolWorkflowDecision {
            state: "declined",
            state_label: "Declined thesis",
            tone: "declined",
            reason: facts
                .decline_reason
                .clone()
                .unwrap_or_else(|| "The system declined to invent an edge.".to_string()),
            primary_kind: "retry_thesis",
            primary_label: "Retry thesis",
            primary_detail:
                "Resolve the active decline and queue fresh research, context, and thesis work."
                    .to_string(),
            review_packet_attention_id: facts.review_packet_attention_id,
        };
    }

    SymbolWorkflowDecision {
        state: "context_ready",
        state_label: "Context ready",
        tone: "ready",
        reason: "Context exists; cognition should draft or decline a thesis.".to_string(),
        primary_kind: "overview",
        primary_label: "Check cognition",
        primary_detail: "Review the latest context, evidence, and cognition status.".to_string(),
        review_packet_attention_id: facts.review_packet_attention_id,
    }
}

impl SymbolWorkflowFacts {
    fn candidate_reason(&self) -> String {
        self.attention_reason()
            .unwrap_or_else(|| "Discovery nominated this symbol for operator review.".to_string())
    }

    fn attention_reason(&self) -> Option<String> {
        self.attention_items.as_array().and_then(|items| {
            items
                .iter()
                .find(|item| {
                    item.get("kind").and_then(serde_json::Value::as_str) == Some("candidate_review")
                })
                .and_then(|item| item.get("reason").and_then(serde_json::Value::as_str))
                .map(str::to_string)
        })
    }
}

fn symbol_workflow_response(facts: &SymbolWorkflowFacts) -> serde_json::Value {
    let decision = classify_symbol_workflow(facts);
    let steps = symbol_workflow_steps(facts, &decision);
    serde_json::json!({
        "symbol": facts.symbol.clone(),
        "state": decision.state,
        "state_label": decision.state_label,
        "tone": decision.tone,
        "reason": decision.reason,
        "primary_action": SymbolWorkflowAction {
            kind: decision.primary_kind,
            label: decision.primary_label,
            detail: decision.primary_detail,
            attention_id: decision.review_packet_attention_id,
        },
        "steps": steps,
        "attention": facts.attention_items.clone(),
        "review_packet_attention_id": decision.review_packet_attention_id,
        "updated_at": Utc::now(),
    })
}

impl Store {
    pub async fn connect(url: &str) -> Result<Self> {
        // Strip the sslmode=disable querystring noise that pgx accepts but
        // sqlx doesn't always: prefer ssl-mode in connection options.
        let max_connections = std::env::var("DATABASE_MAX_CONNECTIONS")
            .ok()
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(8)
            .clamp(1, 32);
        let acquire_timeout_secs = std::env::var("DATABASE_ACQUIRE_TIMEOUT_SECS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(5)
            .clamp(1, 60);
        let pool = PgPoolOptions::new()
            .max_connections(max_connections)
            .acquire_timeout(Duration::from_secs(acquire_timeout_secs))
            .connect(url)
            .await
            .with_context(|| format!("db connect {url}"))?;
        Ok(Self { pool })
    }

    /// Stores a raw event append-only (SPEC §4 PIT corpus). Returns `true`
    /// if newly inserted; `false` if `content_hash` already existed (dedup).
    pub async fn append_ingest_event(
        &self,
        source: &str,
        kind: &str,
        symbol: Option<&str>,
        payload: &[u8],
        content_hash: &str,
        source_ts: Option<DateTime<Utc>>,
    ) -> Result<bool> {
        let payload_str = std::str::from_utf8(payload).context("payload utf-8")?;
        let res = sqlx::query(
            r#"INSERT INTO ingest_event (source, kind, symbol, payload, content_hash, source_ts)
               VALUES ($1, $2, $3, $4::jsonb, $5, $6)
               ON CONFLICT (content_hash) DO NOTHING"#,
        )
        .bind(source)
        .bind(kind)
        .bind(symbol)
        .bind(payload_str)
        .bind(content_hash)
        .bind(source_ts)
        .execute(&self.pool)
        .await
        .context("append_ingest_event")?;
        Ok(res.rows_affected() > 0)
    }

    pub async fn mark_source_started(&self, source: &str, symbols_attempted: i32) -> Result<()> {
        sqlx::query(
            r#"INSERT INTO source_health
                 (source, last_started_at, last_status, symbols_attempted,
                  symbols_failed, rows_seen, rows_inserted, updated_at)
               VALUES ($1, now(), 'running', $2, 0, 0, 0, now())
               ON CONFLICT (source) DO UPDATE SET
                   last_started_at = EXCLUDED.last_started_at,
                   last_status = 'running',
                   symbols_attempted = EXCLUDED.symbols_attempted,
                   symbols_failed = 0,
                   rows_seen = 0,
                   rows_inserted = 0,
                   last_failure_kind = NULL,
                   last_error = NULL,
                   retry_after_at = NULL,
                   updated_at = now()"#,
        )
        .bind(source)
        .bind(symbols_attempted)
        .execute(&self.pool)
        .await
        .with_context(|| format!("mark_source_started {source}"))?;
        Ok(())
    }

    pub async fn record_source_success(
        &self,
        source: &str,
        rows_seen: i64,
        rows_inserted: i64,
        symbols_attempted: i32,
        symbols_failed: i32,
    ) -> Result<()> {
        let status = if rows_inserted == 0 {
            "no_new_rows"
        } else {
            "ok"
        };
        sqlx::query(
            r#"INSERT INTO source_health
                 (source, last_success_at, last_status, last_failure_kind,
                  last_error, retry_after_at, rows_seen, rows_inserted,
                  symbols_attempted, symbols_failed, updated_at)
               VALUES ($1, now(), $2, NULL, NULL, NULL, $3, $4, $5, $6, now())
               ON CONFLICT (source) DO UPDATE SET
                   last_success_at = EXCLUDED.last_success_at,
                   last_status = EXCLUDED.last_status,
                   last_failure_kind = NULL,
                   last_error = NULL,
                   retry_after_at = NULL,
                   rows_seen = EXCLUDED.rows_seen,
                   rows_inserted = EXCLUDED.rows_inserted,
                   symbols_attempted = EXCLUDED.symbols_attempted,
                   symbols_failed = EXCLUDED.symbols_failed,
                   updated_at = now()"#,
        )
        .bind(source)
        .bind(status)
        .bind(rows_seen)
        .bind(rows_inserted)
        .bind(symbols_attempted)
        .bind(symbols_failed)
        .execute(&self.pool)
        .await
        .with_context(|| format!("record_source_success {source}"))?;
        Ok(())
    }

    pub async fn record_source_failure(
        &self,
        source: &str,
        failure_kind: &str,
        error: &str,
        retry_after_at: Option<DateTime<Utc>>,
    ) -> Result<()> {
        sqlx::query(
            r#"INSERT INTO source_health
                 (source, last_failure_at, last_status, last_failure_kind,
                  last_error, retry_after_at, updated_at)
               VALUES ($1, now(), 'failed', $2, $3, $4, now())
               ON CONFLICT (source) DO UPDATE SET
                   last_failure_at = EXCLUDED.last_failure_at,
                   last_status = EXCLUDED.last_status,
                   last_failure_kind = EXCLUDED.last_failure_kind,
                   last_error = EXCLUDED.last_error,
                   retry_after_at = EXCLUDED.retry_after_at,
                   updated_at = now()"#,
        )
        .bind(source)
        .bind(failure_kind)
        .bind(error.chars().take(500).collect::<String>())
        .bind(retry_after_at)
        .execute(&self.pool)
        .await
        .with_context(|| format!("record_source_failure {source}"))?;
        Ok(())
    }

    /// Returns the active config body (raw JSON) + version for `name`.
    pub async fn active_config(&self, name: &str) -> Result<(serde_json::Value, i32)> {
        let row =
            sqlx::query("SELECT body, version FROM config WHERE name = $1 AND active LIMIT 1")
                .bind(name)
                .fetch_one(&self.pool)
                .await
                .with_context(|| format!("active_config {name}"))?;
        let body: serde_json::Value = row.try_get("body")?;
        let version: i32 = row.try_get("version")?;
        Ok((body, version))
    }

    /// Reads the operator-set portfolio frame (#26). Returns the singleton
    /// row; `account_size_usd` is `None` until the operator sets it.
    pub async fn portfolio_settings(&self) -> Result<crate::risk::PortfolioSettings> {
        let row = sqlx::query(
            r#"SELECT account_size_usd::float8 AS acct,
                      high_water_mark_usd::float8 AS hwm
                 FROM portfolio_settings WHERE id = 1"#,
        )
        .fetch_optional(&self.pool)
        .await
        .context("portfolio_settings")?;
        let Some(row) = row else {
            return Ok(crate::risk::PortfolioSettings::default());
        };
        Ok(crate::risk::PortfolioSettings {
            account_size_usd: row.try_get::<Option<f64>, _>("acct").ok().flatten(),
            high_water_mark_usd: row.try_get::<Option<f64>, _>("hwm").ok().flatten(),
        })
    }

    /// Upsert operator-set account size + high-water mark. Either field may
    /// be left `None` (caller's intent: "don't touch this field").
    pub async fn upsert_portfolio_settings(
        &self,
        account_size_usd: Option<f64>,
        high_water_mark_usd: Option<f64>,
        updated_by: &str,
    ) -> Result<()> {
        sqlx::query(
            r#"INSERT INTO portfolio_settings (id, account_size_usd, high_water_mark_usd, updated_at, updated_by)
               VALUES (1, $1, $2, now(), $3)
               ON CONFLICT (id) DO UPDATE SET
                   account_size_usd = COALESCE(EXCLUDED.account_size_usd, portfolio_settings.account_size_usd),
                   high_water_mark_usd = COALESCE(EXCLUDED.high_water_mark_usd, portfolio_settings.high_water_mark_usd),
                   updated_at = now(),
                   updated_by = EXCLUDED.updated_by"#,
        )
        .bind(account_size_usd)
        .bind(high_water_mark_usd)
        .bind(updated_by)
        .execute(&self.pool)
        .await
        .context("upsert_portfolio_settings")?;
        Ok(())
    }

    /// Union of active tickers + active discovery pool members. This is broad
    /// discovery scope; expensive source loops should prefer
    /// `priority_scan_symbols` so the brain refreshes active names first.
    pub async fn scan_pool_symbols(&self) -> Result<Vec<String>> {
        let rows: Vec<(String,)> = sqlx::query_as(
            r#"SELECT symbol FROM (
                  SELECT symbol FROM ticker WHERE status = 'active'
                  UNION
                  SELECT symbol FROM discovery_pool WHERE dropped_at IS NULL
               ) s
               ORDER BY symbol"#,
        )
        .fetch_all(&self.pool)
        .await
        .context("scan_pool_symbols")?;
        Ok(rows.into_iter().map(|r| r.0).collect())
    }

    /// Tiered deep-research universe. Active tickers come first, then the
    /// highest-ranked proposed discovery candidates. This keeps expensive
    /// provider loops inside the freshness SLA instead of re-deep-scanning
    /// the whole screener pool every pass.
    pub async fn priority_scan_symbols(&self, limit: i64) -> Result<Vec<String>> {
        let rows: Vec<(String,)> = sqlx::query_as(
            r#"WITH ranked AS (
                  SELECT target_id AS symbol,
                         -1 AS source_rank,
                         CASE priority
                           WHEN 'blocking' THEN 0
                           WHEN 'high' THEN 1
                           WHEN 'medium' THEN 2
                           ELSE 3
                         END AS tier_rank,
                         100.0 AS fit_rank,
                         due_at AS last_ranked_at
                    FROM source_task
                   WHERE scope = 'symbol'
                     AND target_id <> ''
                     AND (
                         (
                             state IN ('queued', 'no_rows', 'failed', 'rate_limited', 'satisfied')
                             AND due_at <= now()
                         )
                         OR (
                             state = 'fetching'
                             AND updated_at < now() - interval '15 minutes'
                         )
                     )
                  UNION ALL
                  SELECT symbol,
                         0 AS source_rank,
                         tier AS tier_rank,
                         COALESCE(domain_fit::double precision, 0.0) AS fit_rank,
                         added_at AS last_ranked_at
                    FROM ticker
                   WHERE status = 'active'
                  UNION ALL
                  SELECT symbol,
                         1 AS source_rank,
                         COALESCE(proposed_tier, 3) AS tier_rank,
                         COALESCE(domain_fit, 0.0) AS fit_rank,
                         proposed_at AS last_ranked_at
                    FROM discovery_candidate
                   WHERE status = 'proposed'
                     AND COALESCE(proposed_tier, 3) <= 2
               )
               SELECT symbol
                 FROM ranked
             GROUP BY symbol
             ORDER BY
                  MIN(source_rank),
                  MIN(tier_rank),
                  MAX(fit_rank) DESC,
                  MAX(last_ranked_at) DESC,
                  symbol
                LIMIT $1"#,
        )
        .bind(limit.max(1))
        .fetch_all(&self.pool)
        .await
        .context("priority_scan_symbols")?;
        Ok(rows.into_iter().map(|r| r.0).collect())
    }

    pub async fn mark_source_tasks_fetching(
        &self,
        actions: &[&str],
        symbols: &[String],
        owner: &str,
    ) -> Result<u64> {
        self.mark_source_tasks_fetching_for_scope("symbol", actions, symbols, owner)
            .await
    }

    pub async fn mark_source_tasks_fetching_for_scope(
        &self,
        scope: &str,
        actions: &[&str],
        target_ids: &[String],
        owner: &str,
    ) -> Result<u64> {
        if actions.is_empty() || target_ids.is_empty() {
            return Ok(0);
        }
        let actions: Vec<String> = actions.iter().map(|a| (*a).to_string()).collect();
        let res = sqlx::query(
            r#"UPDATE source_task
                  SET state = 'fetching',
                      attempts = attempts + 1,
                      last_error = NULL,
                      updated_at = now(),
                      source_ref = source_ref || jsonb_build_object(
                          'claimed_by', $3,
                          'claimed_at', now()
                      )
                WHERE scope = $4
                  AND target_id = ANY($1::text[])
                  AND action = ANY($2::text[])
                  AND (
                      (
                          state IN ('queued', 'no_rows', 'failed', 'rate_limited', 'satisfied')
                          AND due_at <= now()
                      )
                      OR (
                          state = 'fetching'
                          AND updated_at < now() - interval '15 minutes'
                      )
                  )"#,
        )
        .bind(target_ids)
        .bind(&actions)
        .bind(owner)
        .bind(scope)
        .execute(&self.pool)
        .await
        .context("mark_source_tasks_fetching")?;
        Ok(res.rows_affected())
    }

    pub async fn complete_source_tasks_for_attempt(
        &self,
        action: &str,
        attempted_symbols: &[String],
        symbols_with_rows: &[String],
        owner: &str,
        fresh_for: ChronoDuration,
    ) -> Result<u64> {
        self.complete_source_tasks_for_scope(
            "symbol",
            action,
            attempted_symbols,
            symbols_with_rows,
            owner,
            fresh_for,
        )
        .await
    }

    pub async fn complete_source_tasks_for_scope(
        &self,
        scope: &str,
        action: &str,
        attempted_targets: &[String],
        targets_with_rows: &[String],
        owner: &str,
        fresh_for: ChronoDuration,
    ) -> Result<u64> {
        if attempted_targets.is_empty() {
            return Ok(0);
        }
        let fresh_until = Utc::now() + fresh_for;
        let retry_at = Utc::now() + ChronoDuration::minutes(30);
        let res = sqlx::query(
            r#"UPDATE source_task
                  SET state = CASE
                          WHEN target_id = ANY($3::text[]) THEN 'satisfied'
                          ELSE 'no_rows'
                      END,
                      due_at = CASE
                          WHEN target_id = ANY($3::text[]) THEN $5
                          ELSE $6
                      END,
                      next_retry_at = NULL,
                      last_error = NULL,
                      updated_at = now(),
                      source_ref = source_ref || jsonb_build_object(
                          'completed_by', $4,
                          'completed_at', now(),
                          'result', CASE
                              WHEN target_id = ANY($3::text[]) THEN 'rows_seen'
                              ELSE 'no_rows'
                          END
                      )
                WHERE scope = $7
                  AND target_id = ANY($1::text[])
                  AND action = $2"#,
        )
        .bind(attempted_targets)
        .bind(action)
        .bind(targets_with_rows)
        .bind(owner)
        .bind(fresh_until)
        .bind(retry_at)
        .bind(scope)
        .execute(&self.pool)
        .await
        .context("complete_source_tasks_for_attempt")?;
        Ok(res.rows_affected())
    }

    pub async fn fail_source_tasks_for_attempt(
        &self,
        action: &str,
        attempted_symbols: &[String],
        owner: &str,
        state: &str,
        error: &str,
        retry_after_at: Option<DateTime<Utc>>,
    ) -> Result<u64> {
        self.fail_source_tasks_for_scope(
            "symbol",
            action,
            attempted_symbols,
            owner,
            state,
            error,
            retry_after_at,
        )
        .await
    }

    pub async fn fail_source_tasks_for_scope(
        &self,
        scope: &str,
        action: &str,
        attempted_targets: &[String],
        owner: &str,
        state: &str,
        error: &str,
        retry_after_at: Option<DateTime<Utc>>,
    ) -> Result<u64> {
        if attempted_targets.is_empty() {
            return Ok(0);
        }
        let task_state = if state == "rate_limited" {
            "rate_limited"
        } else {
            "failed"
        };
        let retry_at = retry_after_at.unwrap_or_else(|| Utc::now() + ChronoDuration::minutes(30));
        let res = sqlx::query(
            r#"UPDATE source_task
                  SET state = $3,
                      due_at = $6,
                      next_retry_at = $6,
                      last_error = $5,
                      updated_at = now(),
                      source_ref = source_ref || jsonb_build_object(
                          'failed_by', $4,
                          'failed_at', now()
                      )
                WHERE scope = $7
                  AND target_id = ANY($1::text[])
                  AND action = $2"#,
        )
        .bind(attempted_targets)
        .bind(action)
        .bind(task_state)
        .bind(owner)
        .bind(error.chars().take(500).collect::<String>())
        .bind(retry_at)
        .bind(scope)
        .execute(&self.pool)
        .await
        .context("fail_source_tasks_for_attempt")?;
        Ok(res.rows_affected())
    }

    /// Active discovery pool symbols (not dropped). Used by the discovery
    /// scanner instead of `ticker` so it can fire signals on names we
    /// don't yet track (#88).
    pub async fn discovery_pool_symbols(&self) -> Result<Vec<String>> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT symbol FROM discovery_pool WHERE dropped_at IS NULL ORDER BY symbol",
        )
        .fetch_all(&self.pool)
        .await
        .context("discovery_pool_symbols")?;
        Ok(rows.into_iter().map(|r| r.0).collect())
    }

    /// For each symbol, return the OLDEST bar timestamp we have (None when
    /// we have no bars yet). Lets the price ingest decide cold-start vs
    /// incremental backfill per ticker.
    pub async fn oldest_bar_per_symbol(
        &self,
        symbols: &[String],
    ) -> Result<std::collections::HashMap<String, Option<DateTime<Utc>>>> {
        let mut out: std::collections::HashMap<String, Option<DateTime<Utc>>> =
            symbols.iter().map(|s| (s.clone(), None)).collect();
        if symbols.is_empty() {
            return Ok(out);
        }
        let rows = sqlx::query(
            r#"SELECT symbol, MIN(ts) AS min_ts
                 FROM price_bar
                WHERE symbol = ANY($1)
             GROUP BY symbol"#,
        )
        .bind(symbols)
        .fetch_all(&self.pool)
        .await
        .context("oldest_bar_per_symbol")?;
        for r in rows {
            let s: String = r.try_get("symbol")?;
            let ts: Option<DateTime<Utc>> = r.try_get("min_ts")?;
            out.insert(s, ts);
        }
        Ok(out)
    }

    /// All open positions in the shape the risk overlay consumes.
    // ---------- attention_item (#86) ----------

    /// Upsert an attention item. The partial-unique indexes mean a second
    /// open item for the same (kind, candidate_id) / (kind, thesis_id) /
    /// (kind, symbol) will collide; we no-op on conflict so producers can
    /// fire freely without dedup logic in each call site.
    pub async fn upsert_attention(
        &self,
        kind: &str,
        symbol: Option<&str>,
        thesis_id: Option<uuid::Uuid>,
        candidate_id: Option<i64>,
        severity: &str,
        title: &str,
        reason: Option<&str>,
        source: &str,
        source_ref: serde_json::Value,
    ) -> Result<()> {
        let (fsm_state, owner) = crate::attention::initial_assignment(kind, severity, source);
        sqlx::query(
            r#"INSERT INTO attention_item
                 (kind, symbol, thesis_id, candidate_id, severity, title,
                  reason, source, source_ref, fsm_state, owner, state_reason)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9::jsonb, $10, $11, $12)
               ON CONFLICT DO NOTHING"#,
        )
        .bind(kind)
        .bind(symbol)
        .bind(thesis_id)
        .bind(candidate_id)
        .bind(severity)
        .bind(title)
        .bind(reason)
        .bind(source)
        .bind(source_ref)
        .bind(fsm_state)
        .bind(owner)
        .bind(kind)
        .execute(&self.pool)
        .await
        .context("upsert_attention")?;
        Ok(())
    }

    /// Resolve attention items matching a filter. Idempotent (resolves only
    /// items still 'open'). Returns how many rows transitioned.
    pub async fn resolve_attention(
        &self,
        kind: &str,
        thesis_id: Option<uuid::Uuid>,
        candidate_id: Option<i64>,
        resolution_kind: &str,
        resolution_ref: serde_json::Value,
    ) -> Result<u64> {
        let rows: i64 = sqlx::query_scalar(
            r#"WITH matched AS (
                    SELECT id, fsm_state
                      FROM attention_item
                     WHERE status = 'open'
                       AND kind = $1
                       AND ($2::uuid IS NULL OR thesis_id = $2)
                       AND ($3::bigint IS NULL OR candidate_id = $3)
                     FOR UPDATE
                 ),
                 updated AS (
                    UPDATE attention_item ai
                       SET status = 'resolved',
                           fsm_state = 'resolved',
                           owner = 'system',
                           resolved_at = now(),
                           resolution_kind = $4,
                           resolution_ref = $5::jsonb,
                           next_retry_at = NULL,
                           resurface_at = NULL,
                           state_reason = $4
                      FROM matched m
                     WHERE ai.id = m.id
                 RETURNING ai.id,
                           m.fsm_state AS from_state,
                           ai.fsm_state AS to_state,
                           ai.owner,
                           ai.state_reason,
                           ai.next_retry_at,
                           ai.resurface_at,
                           ai.resolution_ref
                 ),
                 inserted AS (
                    INSERT INTO attention_state_history
                         (attention_id, from_state, to_state, owner, reason,
                          next_retry_at, resurface_at, source_ref)
                    SELECT id, from_state, to_state, owner, state_reason,
                           next_retry_at, resurface_at, resolution_ref
                      FROM updated
                 RETURNING 1
                 )
              SELECT COUNT(*) FROM updated"#,
        )
        .bind(kind)
        .bind(thesis_id)
        .bind(candidate_id)
        .bind(resolution_kind)
        .bind(resolution_ref)
        .fetch_one(&self.pool)
        .await
        .context("resolve_attention")?;
        Ok(rows as u64)
    }

    /// Mark items as dismissed (operator chose "not relevant"). Same filter
    /// shape as resolve_attention.
    pub async fn dismiss_attention(&self, id: i64, reason: Option<&str>) -> Result<bool> {
        let rows: i64 = if reason == Some("defer") {
            sqlx::query_scalar(
                r#"WITH matched AS (
                        SELECT id, fsm_state
                          FROM attention_item
                         WHERE id = $1 AND status = 'open'
                         FOR UPDATE
                     ),
                     updated AS (
                        UPDATE attention_item ai
                           SET status = 'open',
                               fsm_state = 'operator_deferred',
                               owner = 'operator',
                               resolved_at = NULL,
                               resolution_kind = 'deferred',
                               resolution_ref = jsonb_build_object('reason', 'defer'),
                               next_retry_at = NULL,
                               resurface_at = now() + interval '7 days',
                               state_reason = 'defer'
                          FROM matched m
                         WHERE ai.id = m.id
                     RETURNING ai.id,
                               m.fsm_state AS from_state,
                               ai.fsm_state AS to_state,
                               ai.owner,
                               ai.state_reason,
                               ai.next_retry_at,
                               ai.resurface_at,
                               ai.resolution_ref
                     ),
                     inserted AS (
                        INSERT INTO attention_state_history
                             (attention_id, from_state, to_state, owner, reason,
                              next_retry_at, resurface_at, source_ref)
                        SELECT id, from_state, to_state, owner, state_reason,
                               next_retry_at, resurface_at, resolution_ref
                          FROM updated
                     RETURNING 1
                     )
                  SELECT COUNT(*) FROM updated"#,
            )
            .bind(id)
            .fetch_one(&self.pool)
            .await
            .context("defer_attention")?
        } else {
            sqlx::query_scalar(
                r#"WITH matched AS (
                        SELECT id, fsm_state
                          FROM attention_item
                         WHERE id = $1 AND status = 'open'
                         FOR UPDATE
                     ),
                     updated AS (
                        UPDATE attention_item ai
                           SET status = 'dismissed',
                               fsm_state = 'dismissed',
                               owner = 'operator',
                               resolved_at = now(),
                               resolution_kind = 'dismissed',
                               resolution_ref = jsonb_build_object('reason', COALESCE($2::text, '')),
                               next_retry_at = NULL,
                               resurface_at = NULL,
                               state_reason = COALESCE(NULLIF($2::text, ''), 'dismissed')
                          FROM matched m
                         WHERE ai.id = m.id
                     RETURNING ai.id,
                               m.fsm_state AS from_state,
                               ai.fsm_state AS to_state,
                               ai.owner,
                               ai.state_reason,
                               ai.next_retry_at,
                               ai.resurface_at,
                               ai.resolution_ref
                     ),
                     inserted AS (
                        INSERT INTO attention_state_history
                             (attention_id, from_state, to_state, owner, reason,
                              next_retry_at, resurface_at, source_ref)
                        SELECT id, from_state, to_state, owner, state_reason,
                               next_retry_at, resurface_at, resolution_ref
                          FROM updated
                     RETURNING 1
                     )
                  SELECT COUNT(*) FROM updated"#,
            )
            .bind(id)
            .bind(reason)
            .fetch_one(&self.pool)
            .await
            .context("dismiss_attention")?
        };
        Ok(rows > 0)
    }

    pub async fn transition_attention(
        &self,
        id: i64,
        to_state: &str,
        owner: &str,
        reason: &str,
        next_retry_at: Option<DateTime<Utc>>,
        resurface_at: Option<DateTime<Utc>>,
        source_ref: serde_json::Value,
    ) -> Result<bool> {
        let status = crate::attention::status_for_state(to_state);
        let rows: i64 = sqlx::query_scalar(
            r#"WITH matched AS (
                    SELECT id, fsm_state
                      FROM attention_item
                     WHERE id = $1 AND status = 'open'
                     FOR UPDATE
                 ),
                 updated AS (
                    UPDATE attention_item ai
                       SET status = $2,
                           fsm_state = $3,
                           owner = $4,
                           resolved_at = CASE WHEN $2 <> 'open' THEN now() ELSE NULL END,
                           resolution_kind = CASE WHEN $2 <> 'open' THEN $5 ELSE NULL END,
                           resolution_ref = CASE WHEN $2 <> 'open' THEN $8::jsonb ELSE resolution_ref END,
                           next_retry_at = $6,
                           resurface_at = $7,
                           state_reason = $5,
                           source_ref = ai.source_ref || $8::jsonb
                      FROM matched m
                     WHERE ai.id = m.id
                 RETURNING ai.id,
                           m.fsm_state AS from_state,
                           ai.fsm_state AS to_state,
                           ai.owner,
                           ai.state_reason,
                           ai.next_retry_at,
                           ai.resurface_at,
                           $8::jsonb AS transition_ref
                 ),
                 inserted AS (
                    INSERT INTO attention_state_history
                         (attention_id, from_state, to_state, owner, reason,
                          next_retry_at, resurface_at, source_ref)
                    SELECT id, from_state, to_state, owner, state_reason,
                           next_retry_at, resurface_at, transition_ref
                      FROM updated
                 RETURNING 1
                 )
              SELECT COUNT(*) FROM updated"#,
        )
        .bind(id)
        .bind(status)
        .bind(to_state)
        .bind(owner)
        .bind(reason)
        .bind(next_retry_at)
        .bind(resurface_at)
        .bind(source_ref)
        .fetch_one(&self.pool)
        .await
        .context("transition_attention")?;
        Ok(rows > 0)
    }

    async fn resurface_due_attention(&self) -> Result<u64> {
        let rows: i64 = sqlx::query_scalar(
            r#"WITH matched AS (
                    SELECT id, fsm_state
                      FROM attention_item
                     WHERE status = 'open'
                       AND fsm_state = 'operator_deferred'
                       AND resurface_at IS NOT NULL
                       AND resurface_at <= now()
                     FOR UPDATE
                 ),
                 updated AS (
                    UPDATE attention_item ai
                       SET fsm_state = 'ready_for_review',
                           owner = 'operator',
                           resolution_kind = NULL,
                           resolution_ref = NULL,
                           resurface_at = NULL,
                           state_reason = 'resurfaced'
                      FROM matched m
                     WHERE ai.id = m.id
                 RETURNING ai.id,
                           m.fsm_state AS from_state,
                           ai.fsm_state AS to_state,
                           ai.owner,
                           ai.state_reason,
                           ai.next_retry_at,
                           ai.resurface_at,
                           jsonb_build_object('reason', 'resurfaced') AS transition_ref
                 ),
                 inserted AS (
                    INSERT INTO attention_state_history
                         (attention_id, from_state, to_state, owner, reason,
                          next_retry_at, resurface_at, source_ref)
                    SELECT id, from_state, to_state, owner, state_reason,
                           next_retry_at, resurface_at, transition_ref
                      FROM updated
                 RETURNING 1
                 )
              SELECT COUNT(*) FROM updated"#,
        )
        .fetch_one(&self.pool)
        .await
        .context("resurface_due_attention")?;
        Ok(rows as u64)
    }

    /// Open attention items, severity-then-recency ordering.
    pub async fn list_attention(&self, status: &str, limit: i64) -> Result<Vec<serde_json::Value>> {
        if status == "open" {
            self.resurface_due_attention().await?;
        }
        let rows = sqlx::query(
            r#"SELECT id, kind, symbol, thesis_id, candidate_id, severity,
                      status, fsm_state, owner, title, reason, source, source_ref,
                      created_at, resolved_at, resolution_kind,
                      next_retry_at, resurface_at, state_reason
                 FROM attention_item
                WHERE status = $1
                  AND (
                    $1 <> 'open'
                    OR fsm_state <> 'operator_deferred'
                    OR (resurface_at IS NOT NULL AND resurface_at <= now())
                  )
             ORDER BY
                CASE severity WHEN 'blocked' THEN 0 WHEN 'decision' THEN 1
                              WHEN 'review' THEN 2 ELSE 3 END,
                created_at DESC
                LIMIT $2"#,
        )
        .bind(status)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .context("list_attention")?;
        rows.into_iter()
            .map(|r| {
                let created_at: DateTime<Utc> = r.try_get("created_at")?;
                let resolved_at: Option<DateTime<Utc>> = r.try_get("resolved_at")?;
                let next_retry_at: Option<DateTime<Utc>> = r.try_get("next_retry_at")?;
                let resurface_at: Option<DateTime<Utc>> = r.try_get("resurface_at")?;
                Ok(serde_json::json!({
                    "id": r.try_get::<i64, _>("id")?,
                    "kind": r.try_get::<String, _>("kind")?,
                    "symbol": r.try_get::<Option<String>, _>("symbol")?,
                    "thesis_id": r.try_get::<Option<uuid::Uuid>, _>("thesis_id")?,
                    "candidate_id": r.try_get::<Option<i64>, _>("candidate_id")?,
                    "severity": r.try_get::<String, _>("severity")?,
                    "status": r.try_get::<String, _>("status")?,
                    "fsm_state": r.try_get::<String, _>("fsm_state")?,
                    "owner": r.try_get::<String, _>("owner")?,
                    "title": r.try_get::<String, _>("title")?,
                    "reason": r.try_get::<Option<String>, _>("reason")?,
                    "source": r.try_get::<String, _>("source")?,
                    "source_ref": r.try_get::<serde_json::Value, _>("source_ref")?,
                    "created_at": created_at,
                    "resolved_at": resolved_at,
                    "resolution_kind": r.try_get::<Option<String>, _>("resolution_kind")?,
                    "next_retry_at": next_retry_at,
                    "resurface_at": resurface_at,
                    "state_reason": r.try_get::<Option<String>, _>("state_reason")?,
                }))
            })
            .collect()
    }

    pub async fn symbol_workflow(&self, symbol: &str) -> Result<serde_json::Value> {
        self.resurface_due_attention().await?;
        let symbol = symbol.trim().to_ascii_uppercase();
        let row = sqlx::query(
            r#"WITH selected AS (
                    SELECT $1::text AS symbol
                )
                SELECT s.symbol,
                       t.tier AS active_tier,
                       dp.symbol IS NOT NULL AS in_pool,
                       ctx.version AS context_version,
                       COALESCE(evidence.item_count, 0) AS evidence_item_count,
                       COALESCE(evidence.open_count, 0) AS open_evidence,
                       COALESCE(evidence.blocking_count, 0) AS blocking_evidence,
                       COALESCE(tasks.due_count, 0) AS due_source_tasks,
                       latest.thesis_id AS latest_thesis_id,
                       latest.state AS thesis_state,
                       latest.direction AS thesis_direction,
                       latest.edge_rationale AS thesis_reason,
                       COALESCE(declines.decline_count, 0) AS decline_count,
                       declines.decline_reason AS decline_reason,
                       COALESCE(decisions.decision_count, 0) AS decision_count,
                       COALESCE(decisions.pending_manual_fill_count, 0) AS pending_manual_fill_count,
                       COALESCE(positions.open_position_count, 0) AS open_position_count,
                       COALESCE(automation.permission_count, 0) AS automation_permission_count,
                       COALESCE(automation.approved_count, 0) AS automation_approved_count,
                       COALESCE(automation.pending_count, 0) AS automation_pending_count,
                       COALESCE(automation.frozen_count, 0) AS automation_frozen_count,
                       COALESCE(attention.open_count, 0) AS open_attention_count,
                       attention.candidate_attention_id,
                       attention.review_packet_attention_id,
                       COALESCE(attention.items, '[]'::jsonb) AS attention_items
                  FROM selected s
             LEFT JOIN ticker t
                    ON t.symbol = s.symbol
                   AND t.status = 'active'
             LEFT JOIN discovery_pool dp
                    ON dp.symbol = s.symbol
                   AND dp.dropped_at IS NULL
             LEFT JOIN LATERAL (
                    SELECT tc.version
                      FROM ticker_context tc
                     WHERE tc.symbol = s.symbol
                  ORDER BY tc.version DESC
                     LIMIT 1
                ) ctx ON TRUE
             LEFT JOIN LATERAL (
                    SELECT count(*) FILTER (WHERE er.blocking_state <> 'satisfied') AS open_count,
                           count(*) FILTER (
                             WHERE er.priority = 'blocking'
                               AND er.blocking_state <> 'satisfied'
                           ) AS blocking_count,
                           (SELECT count(*) FROM evidence_item ei WHERE ei.symbol = s.symbol) AS item_count
                      FROM evidence_requirement er
                     WHERE er.symbol = s.symbol
                ) evidence ON TRUE
             LEFT JOIN LATERAL (
                    SELECT count(*) FILTER (
                             WHERE st.state IN ('queued', 'no_rows', 'failed', 'rate_limited', 'blocked')
                               AND st.due_at <= now()
                           ) AS due_count
                      FROM source_task st
                     WHERE st.scope = 'symbol'
                       AND st.target_id = s.symbol
                ) tasks ON TRUE
             LEFT JOIN LATERAL (
                    SELECT th.thesis_id,
                           th.state,
                           th.forecast->>'direction' AS direction,
                           th.edge_rationale
                      FROM thesis th
                     WHERE th.symbol = s.symbol
                       AND th.state NOT IN ('closed', 'disqualified')
                  ORDER BY th.updated_at DESC, th.created_at DESC
                     LIMIT 1
                ) latest ON TRUE
             LEFT JOIN LATERAL (
                    SELECT count(*) AS decline_count,
                           (array_agg(ai.reason ORDER BY ai.created_at DESC))[1] AS decline_reason
                     FROM attention_item ai
                     WHERE ai.symbol = s.symbol
                       AND ai.kind = 'thesis_incomplete'
                       AND ai.status = 'open'
                       AND (
                         ai.fsm_state <> 'operator_deferred'
                         OR (ai.resurface_at IS NOT NULL AND ai.resurface_at <= now())
                       )
                ) declines ON TRUE
             LEFT JOIN LATERAL (
                    SELECT count(*) AS decision_count,
                           count(*) FILTER (
                             WHERE d.action IN ('enter', 'resize')
                               AND d.user_choice = 'confirmed'
                           ) AS pending_manual_fill_count
                      FROM decision d
                      JOIN thesis th ON th.thesis_id = d.thesis_id
                     WHERE th.symbol = s.symbol
                ) decisions ON TRUE
             LEFT JOIN LATERAL (
                    SELECT count(*) AS open_position_count
                     FROM position p
                     WHERE p.symbol = s.symbol
                       AND p.closed_at IS NULL
                ) positions ON TRUE
             LEFT JOIN LATERAL (
                    SELECT count(*) AS permission_count,
                           count(*) FILTER (WHERE p.status = 'approved') AS approved_count,
                           count(*) FILTER (WHERE p.status = 'pending') AS pending_count,
                           count(*) FILTER (WHERE p.manual_freeze) AS frozen_count
                      FROM automation_trade_permission p
                     WHERE p.symbol = s.symbol
                       AND p.status IN ('pending', 'approved')
                ) automation ON TRUE
             LEFT JOIN LATERAL (
                    SELECT count(*) AS open_count,
                           (array_agg(ai.id ORDER BY
                                CASE ai.severity
                                  WHEN 'blocked' THEN 0
                                  WHEN 'decision' THEN 1
                                  WHEN 'review' THEN 2
                                  ELSE 3
                                END,
                                ai.created_at DESC))[1] AS review_packet_attention_id,
                           (array_agg(ai.id ORDER BY ai.created_at DESC)
                                FILTER (WHERE ai.kind = 'candidate_review'))[1] AS candidate_attention_id,
                           COALESCE(jsonb_agg(jsonb_build_object(
                                'id', ai.id,
                                'kind', ai.kind,
                                'title', ai.title,
                                'reason', ai.reason,
                                'severity', ai.severity,
                                'fsm_state', ai.fsm_state,
                                'owner', ai.owner,
                                'created_at', ai.created_at
                           ) ORDER BY
                                CASE ai.severity
                                  WHEN 'blocked' THEN 0
                                  WHEN 'decision' THEN 1
                                  WHEN 'review' THEN 2
                                  ELSE 3
                                END,
                                ai.created_at DESC), '[]'::jsonb) AS items
                      FROM attention_item ai
                     WHERE ai.symbol = s.symbol
                       AND ai.status = 'open'
                       AND (
                         ai.fsm_state <> 'operator_deferred'
                         OR (ai.resurface_at IS NOT NULL AND ai.resurface_at <= now())
                       )
                ) attention ON TRUE"#,
        )
        .bind(&symbol)
        .fetch_one(&self.pool)
        .await
        .context("symbol_workflow")?;

        let facts = SymbolWorkflowFacts {
            symbol: row.try_get("symbol")?,
            active_tier: row.try_get("active_tier")?,
            in_pool: row.try_get("in_pool")?,
            context_version: row.try_get("context_version")?,
            evidence_item_count: row.try_get("evidence_item_count")?,
            open_evidence: row.try_get("open_evidence")?,
            blocking_evidence: row.try_get("blocking_evidence")?,
            due_source_tasks: row.try_get("due_source_tasks")?,
            latest_thesis_id: row.try_get("latest_thesis_id")?,
            thesis_state: row.try_get("thesis_state")?,
            thesis_direction: row.try_get("thesis_direction")?,
            thesis_reason: row.try_get("thesis_reason")?,
            decline_count: row.try_get("decline_count")?,
            decline_reason: row.try_get("decline_reason")?,
            decision_count: row.try_get("decision_count")?,
            pending_manual_fill_count: row.try_get("pending_manual_fill_count")?,
            open_position_count: row.try_get("open_position_count")?,
            automation_permission_count: row.try_get("automation_permission_count")?,
            automation_approved_count: row.try_get("automation_approved_count")?,
            automation_pending_count: row.try_get("automation_pending_count")?,
            automation_frozen_count: row.try_get("automation_frozen_count")?,
            open_attention_count: row.try_get("open_attention_count")?,
            candidate_attention_id: row.try_get("candidate_attention_id")?,
            review_packet_attention_id: row.try_get("review_packet_attention_id")?,
            attention_items: row.try_get("attention_items")?,
        };
        Ok(symbol_workflow_response(&facts))
    }

    pub async fn thesis_declines_for_symbol(
        &self,
        symbol: &str,
        limit: i64,
    ) -> Result<Vec<serde_json::Value>> {
        let rows = sqlx::query(
            r#"SELECT id, symbol, candidate_id, severity, status, title, reason,
                      source_ref, created_at, resolved_at, resolution_kind
                 FROM attention_item
                WHERE symbol = $1
                  AND kind = 'thesis_incomplete'
             ORDER BY created_at DESC
                LIMIT $2"#,
        )
        .bind(symbol)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .context("thesis_declines_for_symbol")?;
        rows.into_iter()
            .map(|r| {
                let created_at: DateTime<Utc> = r.try_get("created_at")?;
                let resolved_at: Option<DateTime<Utc>> = r.try_get("resolved_at")?;
                Ok(serde_json::json!({
                    "id": r.try_get::<i64, _>("id")?,
                    "symbol": r.try_get::<Option<String>, _>("symbol")?,
                    "candidate_id": r.try_get::<Option<i64>, _>("candidate_id")?,
                    "severity": r.try_get::<String, _>("severity")?,
                    "status": r.try_get::<String, _>("status")?,
                    "title": r.try_get::<String, _>("title")?,
                    "reason": r.try_get::<Option<String>, _>("reason")?,
                    "source_ref": r.try_get::<serde_json::Value, _>("source_ref")?,
                    "created_at": created_at,
                    "resolved_at": resolved_at,
                    "resolution_kind": r.try_get::<Option<String>, _>("resolution_kind")?,
                }))
            })
            .collect()
    }

    pub async fn evidence_requirements_for_symbol(
        &self,
        symbol: &str,
    ) -> Result<Vec<serde_json::Value>> {
        let rows = sqlx::query(
            r#"SELECT id, symbol, requirement_key, source_type, reason, priority,
                      blocking_state, attempts, next_retry_at, last_error,
                      source_ref, created_at, updated_at, satisfied_at,
                      COALESCE((
                          SELECT jsonb_agg(
                              jsonb_build_object(
                                  'id', st.id,
                                  'action', st.action,
                                  'provider', st.provider,
                                  'state', st.state,
                                  'priority', st.priority,
                                  'due_at', st.due_at,
                                  'next_retry_at', st.next_retry_at,
                                  'attempts', st.attempts,
                                  'last_error', st.last_error,
                                  'updated_at', st.updated_at
                              )
                              ORDER BY
                                  CASE st.state
                                       WHEN 'queued' THEN 0
                                       WHEN 'rate_limited' THEN 1
                                       WHEN 'failed' THEN 2
                                       WHEN 'no_rows' THEN 3
                                       WHEN 'fetching' THEN 4
                                       ELSE 5
                                  END,
                                  st.due_at
                          )
                            FROM source_task st
                           WHERE st.scope = 'symbol'
                             AND st.target_id = evidence_requirement.symbol
                             AND st.requirement_key = evidence_requirement.requirement_key
                      ), '[]'::jsonb) AS source_tasks
                 FROM evidence_requirement
                WHERE symbol = $1
             ORDER BY
                  CASE priority
                       WHEN 'blocking' THEN 0
                       WHEN 'high' THEN 1
                       WHEN 'medium' THEN 2
                       ELSE 3
                  END,
                  CASE blocking_state
                       WHEN 'missing' THEN 0
                       WHEN 'partial' THEN 1
                       WHEN 'blocked' THEN 2
                       WHEN 'fetching' THEN 3
                       ELSE 4
                  END,
                  updated_at DESC"#,
        )
        .bind(symbol)
        .fetch_all(&self.pool)
        .await
        .context("evidence_requirements_for_symbol")?;
        rows.into_iter()
            .map(|r| {
                let created_at: DateTime<Utc> = r.try_get("created_at")?;
                let updated_at: DateTime<Utc> = r.try_get("updated_at")?;
                let next_retry_at: Option<DateTime<Utc>> = r.try_get("next_retry_at")?;
                let satisfied_at: Option<DateTime<Utc>> = r.try_get("satisfied_at")?;
                Ok(serde_json::json!({
                    "id": r.try_get::<i64, _>("id")?,
                    "symbol": r.try_get::<String, _>("symbol")?,
                    "requirement_key": r.try_get::<String, _>("requirement_key")?,
                    "source_type": r.try_get::<String, _>("source_type")?,
                    "reason": r.try_get::<String, _>("reason")?,
                    "priority": r.try_get::<String, _>("priority")?,
                    "blocking_state": r.try_get::<String, _>("blocking_state")?,
                    "attempts": r.try_get::<i32, _>("attempts")?,
                    "next_retry_at": next_retry_at,
                    "last_error": r.try_get::<Option<String>, _>("last_error")?,
                    "source_ref": r.try_get::<serde_json::Value, _>("source_ref")?,
                    "source_tasks": r.try_get::<serde_json::Value, _>("source_tasks")?,
                    "created_at": created_at,
                    "updated_at": updated_at,
                    "satisfied_at": satisfied_at,
                }))
            })
            .collect()
    }

    pub async fn research_evidence_for_symbol(
        &self,
        symbol: &str,
    ) -> Result<Vec<serde_json::Value>> {
        let rows = sqlx::query(
            r#"WITH ranked AS (
                  SELECT DISTINCT ON (lower(title), COALESCE(published_at, retrieved_at))
                         id, symbol, query, url, title, publisher, published_at,
                         retrieved_at, provider, source_type, credibility, summary, tags
                    FROM research_evidence
                   WHERE symbol = $1
                ORDER BY lower(title),
                         COALESCE(published_at, retrieved_at),
                         (url LIKE 'http://www.bing.com/%') ASC,
                         retrieved_at DESC
              )
              SELECT *
                FROM ranked
            ORDER BY credibility = 'primary' DESC,
                     published_at DESC NULLS LAST,
                     retrieved_at DESC
               LIMIT 50"#,
        )
        .bind(symbol)
        .fetch_all(&self.pool)
        .await
        .context("research_evidence_for_symbol")?;
        rows.into_iter()
            .map(|r| {
                let published_at: Option<DateTime<Utc>> = r.try_get("published_at")?;
                let retrieved_at: DateTime<Utc> = r.try_get("retrieved_at")?;
                Ok(serde_json::json!({
                    "id": r.try_get::<i64, _>("id")?,
                    "symbol": r.try_get::<String, _>("symbol")?,
                    "query": r.try_get::<String, _>("query")?,
                    "url": r.try_get::<String, _>("url")?,
                    "title": r.try_get::<String, _>("title")?,
                    "publisher": r.try_get::<Option<String>, _>("publisher")?,
                    "published_at": published_at,
                    "retrieved_at": retrieved_at,
                    "provider": r.try_get::<String, _>("provider")?,
                    "source_type": r.try_get::<String, _>("source_type")?,
                    "credibility": r.try_get::<String, _>("credibility")?,
                    "summary": r.try_get::<Option<String>, _>("summary")?,
                    "tags": r.try_get::<Vec<String>, _>("tags")?,
                }))
            })
            .collect()
    }

    pub async fn evidence_items_for_symbol(&self, symbol: &str) -> Result<Vec<serde_json::Value>> {
        let rows = sqlx::query(
            r#"SELECT id, symbol, kind, observed_at, source, source_id,
                      source_ref, summary, strength, polarity, url, created_at, updated_at
                 FROM evidence_item
                WHERE symbol = $1
             ORDER BY observed_at DESC, id DESC
                LIMIT 100"#,
        )
        .bind(symbol)
        .fetch_all(&self.pool)
        .await
        .context("evidence_items_for_symbol")?;
        rows.into_iter()
            .map(|r| {
                let observed_at: DateTime<Utc> = r.try_get("observed_at")?;
                let created_at: DateTime<Utc> = r.try_get("created_at")?;
                let updated_at: DateTime<Utc> = r.try_get("updated_at")?;
                Ok(serde_json::json!({
                    "id": r.try_get::<i64, _>("id")?,
                    "symbol": r.try_get::<String, _>("symbol")?,
                    "kind": r.try_get::<String, _>("kind")?,
                    "observed_at": observed_at,
                    "source": r.try_get::<String, _>("source")?,
                    "source_id": r.try_get::<String, _>("source_id")?,
                    "source_ref": r.try_get::<serde_json::Value, _>("source_ref")?,
                    "summary": r.try_get::<String, _>("summary")?,
                    "strength": r.try_get::<Option<f64>, _>("strength")?,
                    "polarity": r.try_get::<Option<f64>, _>("polarity")?,
                    "url": r.try_get::<Option<String>, _>("url")?,
                    "created_at": created_at,
                    "updated_at": updated_at,
                }))
            })
            .collect()
    }

    /// Recent decisions for a given symbol — joins through thesis to filter.
    pub async fn decisions_for_symbol(&self, symbol: &str) -> Result<Vec<serde_json::Value>> {
        let rows = sqlx::query(
            r#"SELECT d.decision_id, d.thesis_id, d.action, d.user_choice,
                      d.disagreement_reason, d.disagreement_detail,
                      d.human_conviction, d.reason,
                      d.sizing, d.at, t.state AS thesis_state,
                      t.forecast->>'direction' AS thesis_direction,
                      COALESCE(d.sizing->>'side', '') AS side,
                      COALESCE(d.sizing->>'instrument', t.instrument) AS instrument,
                      dr.decision_id IS NOT NULL AS has_replay
                 FROM decision d
                 JOIN thesis t USING (thesis_id)
            LEFT JOIN decision_replay dr ON dr.decision_id = d.decision_id
                WHERE t.symbol = $1
             ORDER BY d.at DESC LIMIT 100"#,
        )
        .bind(symbol)
        .fetch_all(&self.pool)
        .await
        .context("decisions_for_symbol")?;
        rows.into_iter()
            .map(|r| {
                let at: DateTime<Utc> = r.try_get("at")?;
                Ok(serde_json::json!({
                    "decision_id": r.try_get::<uuid::Uuid, _>("decision_id")?,
                    "thesis_id": r.try_get::<Option<uuid::Uuid>, _>("thesis_id")?,
                    "action": r.try_get::<String, _>("action")?,
                    "user_choice": r.try_get::<Option<String>, _>("user_choice")?,
                    "disagreement_reason": r.try_get::<Option<String>, _>("disagreement_reason")?,
                    "disagreement_detail": r.try_get::<Option<String>, _>("disagreement_detail")?,
                    "human_conviction": r.try_get::<Option<String>, _>("human_conviction")?,
                    "reason": r.try_get::<Option<String>, _>("reason")?,
                    "sizing": r.try_get::<Option<serde_json::Value>, _>("sizing")?,
                    "thesis_state": r.try_get::<String, _>("thesis_state")?,
                    "thesis_direction": r.try_get::<Option<String>, _>("thesis_direction")?,
                    "side": r.try_get::<String, _>("side")?,
                    "instrument": r.try_get::<Option<String>, _>("instrument")?,
                    "has_replay": r.try_get::<bool, _>("has_replay").unwrap_or(false),
                    "at": at,
                }))
            })
            .collect()
    }

    pub async fn decision_replay(
        &self,
        decision_id: uuid::Uuid,
    ) -> Result<Option<serde_json::Value>> {
        let row = sqlx::query(
            r#"SELECT dr.decision_id, dr.symbol, dr.thesis_id, dr.context_version,
                      dr.thesis_snapshot, dr.consensus_score, dr.risk_verdict,
                      dr.evidence_ids, dr.evidence_snapshot, dr.system_confidence,
                      dr.chart_range_seen, dr.captured_at,
                      to_jsonb(d) AS decision_snapshot
                 FROM decision_replay dr
                 JOIN decision d ON d.decision_id = dr.decision_id
                WHERE dr.decision_id = $1"#,
        )
        .bind(decision_id)
        .fetch_optional(&self.pool)
        .await
        .context("decision_replay")?;
        let Some(r) = row else {
            return Ok(None);
        };
        let captured_at: DateTime<Utc> = r.try_get("captured_at")?;
        let evidence_ids: Vec<i64> = r.try_get("evidence_ids").unwrap_or_default();
        Ok(Some(serde_json::json!({
            "decision_id": r.try_get::<uuid::Uuid, _>("decision_id")?,
            "symbol": r.try_get::<String, _>("symbol")?,
            "thesis_id": r.try_get::<Option<uuid::Uuid>, _>("thesis_id")?,
            "context_version": r.try_get::<Option<i32>, _>("context_version")?,
            "thesis_snapshot": r.try_get::<serde_json::Value, _>("thesis_snapshot")?,
            "consensus_score": r.try_get::<Option<f64>, _>("consensus_score")?,
            "risk_verdict": r.try_get::<serde_json::Value, _>("risk_verdict")?,
            "evidence_ids": evidence_ids,
            "evidence_snapshot": r.try_get::<serde_json::Value, _>("evidence_snapshot")?,
            "system_confidence": r.try_get::<Option<String>, _>("system_confidence")?,
            "chart_range_seen": r.try_get::<Option<String>, _>("chart_range_seen")?,
            "decision_snapshot": r.try_get::<serde_json::Value, _>("decision_snapshot")?,
            "captured_at": captured_at,
        })))
    }

    /// Returns timestamped events for a symbol — thesis state transitions,
    /// risk alerts, decisions — for chart marker overlays (#57 PR3).
    pub async fn symbol_events(
        &self,
        symbol: &str,
        lookback_days: i64,
    ) -> Result<Vec<serde_json::Value>> {
        let rows = sqlx::query(
            r#"
            -- thesis state transitions (one row per state hop)
            SELECT 'state_transition' AS kind,
                   tsh.at AS at,
                   t.thesis_id::text AS thesis_id,
                   tsh.to_state AS label,
                   COALESCE(tsh.rationale, '') AS detail
              FROM thesis_state_history tsh
              JOIN thesis t USING (thesis_id)
             WHERE t.symbol = $1 AND tsh.at > now() - ($2 || ' days')::interval
            UNION ALL
            -- risk + state-transition alerts
            SELECT a.kind AS kind,
                   a.created_at AS at,
                   COALESCE(a.thesis_id::text, '') AS thesis_id,
                   COALESCE(a.payload->>'kind', a.kind) AS label,
                   COALESCE(a.payload->>'reasons', '') AS detail
              FROM alert a
             WHERE a.symbol = $1 AND a.created_at > now() - ($2 || ' days')::interval
            UNION ALL
            -- recorded decisions
            SELECT 'decision' AS kind,
                   d.at AS at,
                   COALESCE(d.thesis_id::text, '') AS thesis_id,
                   CASE
                     WHEN d.action = 'enter' AND COALESCE(d.sizing->>'side', '') <> ''
                       THEN d.action || ' ' || (d.sizing->>'side')
                     WHEN d.action = 'enter' AND t.forecast->>'direction' = 'down'
                       THEN 'enter bearish'
                     WHEN d.action = 'enter' AND t.forecast->>'direction' = 'up'
                       THEN 'enter bullish'
                     ELSE d.action
                   END AS label,
                   concat_ws(
                       ' · ',
                       NULLIF(d.user_choice, ''),
                       NULLIF(d.human_conviction, ''),
                       NULLIF(d.disagreement_reason, '')
                   ) AS detail
              FROM decision d
              JOIN thesis t USING (thesis_id)
             WHERE t.symbol = $1 AND d.at > now() - ($2 || ' days')::interval
         ORDER BY at ASC"#,
        )
        .bind(symbol)
        .bind(lookback_days.to_string())
        .fetch_all(&self.pool)
        .await
        .context("symbol_events")?;
        rows.into_iter()
            .map(|r| {
                let at: DateTime<Utc> = r.try_get("at")?;
                Ok(serde_json::json!({
                    "kind": r.try_get::<String, _>("kind")?,
                    "time": at.format("%Y-%m-%d").to_string(),
                    "thesis_id": r.try_get::<String, _>("thesis_id")?,
                    "label": r.try_get::<String, _>("label")?,
                    "detail": r.try_get::<String, _>("detail")?,
                }))
            })
            .collect()
    }

    pub async fn derived_refresh_status(&self) -> Result<serde_json::Value> {
        let by_state_rows = sqlx::query(
            r#"SELECT state, count(*) AS n
                 FROM derived_refresh_task
             GROUP BY state
             ORDER BY state"#,
        )
        .fetch_all(&self.pool)
        .await
        .context("derived_refresh_status by_state")?;
        let by_state = by_state_rows
            .into_iter()
            .map(|r| {
                Ok(serde_json::json!({
                    "state": r.try_get::<String, _>("state")?,
                    "count": r.try_get::<i64, _>("n")?,
                }))
            })
            .collect::<Result<Vec<_>>>()?;

        let by_target_rows = sqlx::query(
            r#"SELECT target_kind, state, count(*) AS n
                 FROM derived_refresh_task
             GROUP BY target_kind, state
             ORDER BY target_kind, state"#,
        )
        .fetch_all(&self.pool)
        .await
        .context("derived_refresh_status by_target")?;
        let by_target = by_target_rows
            .into_iter()
            .map(|r| {
                Ok(serde_json::json!({
                    "target_kind": r.try_get::<String, _>("target_kind")?,
                    "state": r.try_get::<String, _>("state")?,
                    "count": r.try_get::<i64, _>("n")?,
                }))
            })
            .collect::<Result<Vec<_>>>()?;

        let due_count: i64 = sqlx::query_scalar(
            r#"SELECT count(*)
                 FROM derived_refresh_task
                WHERE state IN ('queued', 'failed')
                  AND due_at <= now()"#,
        )
        .fetch_one(&self.pool)
        .await
        .context("derived_refresh_status due_count")?;
        let queue_window = sqlx::query(
            r#"SELECT count(*) FILTER (WHERE state IN ('queued', 'failed')) AS queued_count,
                      count(*) FILTER (
                          WHERE state IN ('queued', 'failed')
                            AND due_at > now()
                      ) AS scheduled_count,
                      min(due_at) FILTER (WHERE state IN ('queued', 'failed')) AS next_due_at
                 FROM derived_refresh_task"#,
        )
        .fetch_one(&self.pool)
        .await
        .context("derived_refresh_status queue_window")?;
        let queued_count: i64 = queue_window.try_get("queued_count")?;
        let scheduled_count: i64 = queue_window.try_get("scheduled_count")?;
        let next_due_at: Option<DateTime<Utc>> = queue_window.try_get("next_due_at")?;
        let stale_running: i64 = sqlx::query_scalar(
            r#"SELECT count(*)
                 FROM derived_refresh_task
                WHERE state = 'running'
                  AND started_at < now() - interval '5 minutes'"#,
        )
        .fetch_one(&self.pool)
        .await
        .context("derived_refresh_status stale_running")?;

        let recent_rows = sqlx::query(
            r#"SELECT id, target_kind, target_id, symbol, reason, dependency_kind,
                      dependency_id, priority, state, generation, due_at, attempts,
                      last_error, started_at, completed_at, updated_at
                 FROM derived_refresh_task
             ORDER BY updated_at DESC, id DESC
                LIMIT 20"#,
        )
        .fetch_all(&self.pool)
        .await
        .context("derived_refresh_status recent")?;
        let recent = recent_rows
            .into_iter()
            .map(|r| {
                Ok(serde_json::json!({
                    "id": r.try_get::<i64, _>("id")?,
                    "target_kind": r.try_get::<String, _>("target_kind")?,
                    "target_id": r.try_get::<String, _>("target_id")?,
                    "symbol": r.try_get::<Option<String>, _>("symbol")?,
                    "reason": r.try_get::<String, _>("reason")?,
                    "dependency_kind": r.try_get::<String, _>("dependency_kind")?,
                    "dependency_id": r.try_get::<Option<String>, _>("dependency_id")?,
                    "priority": r.try_get::<String, _>("priority")?,
                    "state": r.try_get::<String, _>("state")?,
                    "generation": r.try_get::<i32, _>("generation")?,
                    "due_at": r.try_get::<DateTime<Utc>, _>("due_at")?,
                    "attempts": r.try_get::<i32, _>("attempts")?,
                    "last_error": r.try_get::<Option<String>, _>("last_error")?,
                    "started_at": r.try_get::<Option<DateTime<Utc>>, _>("started_at")?,
                    "completed_at": r.try_get::<Option<DateTime<Utc>>, _>("completed_at")?,
                    "updated_at": r.try_get::<DateTime<Utc>, _>("updated_at")?,
                }))
            })
            .collect::<Result<Vec<_>>>()?;

        Ok(serde_json::json!({
            "due_count": due_count,
            "queued_count": queued_count,
            "scheduled_count": scheduled_count,
            "next_due_at": next_due_at,
            "stale_running": stale_running,
            "by_state": by_state,
            "by_target": by_target,
            "recent": recent,
        }))
    }

    pub async fn process_due_derived_refresh_tasks(&self, limit: i64) -> Result<u64> {
        let tasks = self
            .claim_derived_refresh_tasks(limit.clamp(1, 100))
            .await?;
        let mut processed = 0;
        for task in tasks {
            let result = self.run_derived_refresh_task(&task).await;
            match result {
                Ok(source_ref) => {
                    self.complete_derived_refresh_task(task.id, task.generation, &source_ref)
                        .await?;
                }
                Err(err) => {
                    self.fail_derived_refresh_task(task.id, task.generation, &err.to_string())
                        .await?;
                }
            }
            processed += 1;
        }
        Ok(processed)
    }

    async fn claim_derived_refresh_tasks(&self, limit: i64) -> Result<Vec<DerivedRefreshTask>> {
        let rows = sqlx::query(
            r#"WITH candidates AS (
                   SELECT id
                     FROM derived_refresh_task
                    WHERE state IN ('queued', 'failed')
                      AND due_at <= now()
                 ORDER BY CASE priority
                            WHEN 'blocking' THEN 0
                            WHEN 'high' THEN 1
                            WHEN 'medium' THEN 2
                            ELSE 3
                          END,
                          updated_at ASC,
                          id ASC
                    LIMIT $1
                    FOR UPDATE SKIP LOCKED
               )
               UPDATE derived_refresh_task t
                  SET state = 'running',
                      attempts = attempts + 1,
                      started_at = now(),
                      updated_at = now(),
                      last_error = NULL
                 FROM candidates c
                WHERE t.id = c.id
            RETURNING t.id, t.generation, t.target_kind, t.target_id, t.symbol,
                      t.reason, t.dependency_kind, t.dependency_id"#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .context("claim_derived_refresh_tasks")?;

        rows.into_iter()
            .map(|r| {
                Ok(DerivedRefreshTask {
                    id: r.try_get("id")?,
                    generation: r.try_get("generation")?,
                    target_kind: r.try_get("target_kind")?,
                    target_id: r.try_get("target_id")?,
                    symbol: r.try_get("symbol")?,
                    reason: r.try_get("reason")?,
                    dependency_kind: r.try_get("dependency_kind")?,
                    dependency_id: r.try_get("dependency_id")?,
                })
            })
            .collect()
    }

    async fn run_derived_refresh_task(
        &self,
        task: &DerivedRefreshTask,
    ) -> Result<serde_json::Value> {
        match task.target_kind.as_str() {
            "brain_journal" => {
                let day = parse_derived_refresh_day(&task.target_id)?;
                let inserted = self.refresh_brain_journal_entries(day).await?;
                Ok(serde_json::json!({
                    "processed": "brain_journal_refresh",
                    "journal_date": day.format("%Y-%m-%d").to_string(),
                    "inserted": inserted,
                    "reason": &task.reason,
                    "dependency_kind": &task.dependency_kind,
                    "dependency_id": &task.dependency_id,
                }))
            }
            "trade_desk" => {
                let day = parse_derived_refresh_day(&task.target_id)?;
                Ok(serde_json::json!({
                    "processed": "trade_desk_live_projection",
                    "journal_date": day.format("%Y-%m-%d").to_string(),
                    "note": "daily trade desk is derived from active_tickers at read time",
                    "reason": &task.reason,
                    "dependency_kind": &task.dependency_kind,
                    "dependency_id": &task.dependency_id,
                }))
            }
            "brain_link" => {
                let symbol = task.symbol.as_deref().unwrap_or(&task.target_id);
                let snapshot = self.derived_symbol_snapshot(symbol).await?;
                Ok(serde_json::json!({
                    "processed": "brain_link_live_projection",
                    "symbol": symbol,
                    "snapshot": snapshot,
                    "reason": &task.reason,
                    "dependency_kind": &task.dependency_kind,
                    "dependency_id": &task.dependency_id,
                }))
            }
            "review_packet" => {
                let symbol = task.symbol.as_deref().unwrap_or(&task.target_id);
                let snapshot = self.derived_symbol_snapshot(symbol).await?;
                Ok(serde_json::json!({
                    "processed": "review_packet_live_projection",
                    "symbol": symbol,
                    "snapshot": snapshot,
                    "reason": &task.reason,
                    "dependency_kind": &task.dependency_kind,
                    "dependency_id": &task.dependency_id,
                }))
            }
            other => Err(anyhow::anyhow!(
                "unknown derived refresh target_kind {other}"
            )),
        }
    }

    async fn derived_symbol_snapshot(&self, symbol: &str) -> Result<serde_json::Value> {
        let row = sqlx::query(
            r#"SELECT
                  (SELECT thesis_id
                     FROM thesis
                    WHERE symbol = $1
                      AND state NOT IN ('closed', 'disqualified')
                 ORDER BY updated_at DESC, created_at DESC
                    LIMIT 1) AS thesis_id,
                  (SELECT state
                     FROM thesis
                    WHERE symbol = $1
                      AND state NOT IN ('closed', 'disqualified')
                 ORDER BY updated_at DESC, created_at DESC
                    LIMIT 1) AS thesis_state,
                  (SELECT forecast->>'direction'
                     FROM thesis
                    WHERE symbol = $1
                      AND state NOT IN ('closed', 'disqualified')
                 ORDER BY updated_at DESC, created_at DESC
                    LIMIT 1) AS thesis_direction,
                  (SELECT conviction_tier
                     FROM thesis
                    WHERE symbol = $1
                      AND state NOT IN ('closed', 'disqualified')
                 ORDER BY updated_at DESC, created_at DESC
                    LIMIT 1) AS conviction_tier,
                  (SELECT system_confidence
                     FROM thesis
                    WHERE symbol = $1
                      AND state NOT IN ('closed', 'disqualified')
                 ORDER BY updated_at DESC, created_at DESC
                    LIMIT 1) AS system_confidence,
                  (SELECT created_at
                     FROM ticker_context
                    WHERE symbol = $1
                 ORDER BY version DESC
                    LIMIT 1) AS context_at,
                  (SELECT max(updated_at)
                     FROM evidence_item
                    WHERE symbol = $1) AS evidence_at,
                  (SELECT count(*)
                     FROM attention_item
                    WHERE symbol = $1
                      AND status = 'open'
                      AND (
                        fsm_state <> 'operator_deferred'
                        OR (resurface_at IS NOT NULL AND resurface_at <= now())
                      )) AS open_attention,
                  COALESCE((
                    SELECT jsonb_agg(
                               jsonb_build_object(
                                 'key', bt.key,
                                 'name', bt.name,
                                 'scope', bt.scope,
                                 'role', btt.role,
                                 'mapping_conviction', btt.conviction,
                                 'live_conviction', brain_ticker_live_conviction(
                                     btt.conviction,
                                     latest.conviction_tier,
                                     latest.system_confidence,
                                     latest.forecast
                                 )
                               )
                               ORDER BY brain_ticker_live_conviction(
                                   btt.conviction,
                                   latest.conviction_tier,
                                   latest.system_confidence,
                                   latest.forecast
                               ) DESC,
                               bt.name
                           )
                      FROM brain_thesis_ticker btt
                      JOIN brain_thesis bt ON bt.id = btt.brain_thesis_id
                 LEFT JOIN LATERAL (
                           SELECT th.forecast, th.conviction_tier, th.system_confidence
                             FROM thesis th
                            WHERE th.symbol = btt.symbol
                              AND th.state NOT IN ('closed', 'disqualified')
                         ORDER BY th.updated_at DESC, th.created_at DESC
                            LIMIT 1
                      ) latest ON TRUE
                     WHERE btt.symbol = $1
                       AND bt.active = true
                  ), '[]'::jsonb) AS parent_themes"#,
        )
        .bind(symbol)
        .fetch_one(&self.pool)
        .await
        .context("derived_symbol_snapshot")?;
        Ok(serde_json::json!({
            "symbol": symbol,
            "thesis_id": row.try_get::<Option<uuid::Uuid>, _>("thesis_id")?,
            "thesis_state": row.try_get::<Option<String>, _>("thesis_state")?,
            "thesis_direction": row.try_get::<Option<String>, _>("thesis_direction")?,
            "conviction_tier": row.try_get::<Option<String>, _>("conviction_tier")?,
            "system_confidence": row.try_get::<Option<String>, _>("system_confidence")?,
            "context_at": row.try_get::<Option<DateTime<Utc>>, _>("context_at")?,
            "evidence_at": row.try_get::<Option<DateTime<Utc>>, _>("evidence_at")?,
            "open_attention": row.try_get::<i64, _>("open_attention").unwrap_or(0),
            "parent_themes": row.try_get::<serde_json::Value, _>("parent_themes")?,
        }))
    }

    async fn complete_derived_refresh_task(
        &self,
        id: i64,
        generation: i32,
        result: &serde_json::Value,
    ) -> Result<()> {
        sqlx::query(
            r#"UPDATE derived_refresh_task
                  SET state = CASE WHEN generation = $2 THEN 'satisfied' ELSE 'queued' END,
                      completed_at = CASE WHEN generation = $2 THEN now() ELSE completed_at END,
                      source_ref = source_ref || jsonb_build_object(
                          'last_result', $3::jsonb,
                          'last_processed_at', now()
                      ),
                      updated_at = now(),
                      last_error = NULL
                WHERE id = $1"#,
        )
        .bind(id)
        .bind(generation)
        .bind(result)
        .execute(&self.pool)
        .await
        .context("complete_derived_refresh_task")?;
        Ok(())
    }

    async fn fail_derived_refresh_task(&self, id: i64, generation: i32, error: &str) -> Result<()> {
        sqlx::query(
            r#"UPDATE derived_refresh_task
                  SET state = CASE WHEN generation = $2 THEN 'failed' ELSE 'queued' END,
                      due_at = CASE
                          WHEN generation = $2
                          THEN now() + (
                              LEAST(3600, GREATEST(30, attempts * 60))::text || ' seconds'
                          )::interval
                          ELSE due_at
                      END,
                      last_error = CASE WHEN generation = $2 THEN $3 ELSE last_error END,
                      source_ref = source_ref || jsonb_build_object(
                          'last_failure_at', now()
                      ),
                      updated_at = now()
                WHERE id = $1"#,
        )
        .bind(id)
        .bind(generation)
        .bind(error)
        .execute(&self.pool)
        .await
        .context("fail_derived_refresh_task")?;
        Ok(())
    }

    pub async fn refresh_brain_journal_entries(&self, day: NaiveDate) -> Result<u64> {
        let start_naive = day
            .and_hms_opt(0, 0, 0)
            .ok_or_else(|| anyhow::anyhow!("invalid journal day"))?;
        let start = DateTime::<Utc>::from_naive_utc_and_offset(start_naive, Utc);
        let end = start + ChronoDuration::days(1);

        let mut drafts = Vec::new();
        drafts.extend(self.brain_journal_attention_drafts(day, start, end).await?);
        drafts.extend(
            self.brain_journal_source_task_drafts(day, start, end)
                .await?,
        );
        drafts.extend(
            self.brain_journal_thesis_state_drafts(day, start, end)
                .await?,
        );
        drafts.extend(
            self.brain_journal_thesis_version_drafts(day, start, end)
                .await?,
        );
        drafts.extend(self.brain_journal_evidence_drafts(day, start, end).await?);
        drafts.extend(
            self.brain_journal_parent_thesis_drafts(day, start, end)
                .await?,
        );
        drafts.extend(
            self.brain_journal_dislocation_drafts(day, start, end)
                .await?,
        );

        let mut inserted = 0;
        for draft in drafts {
            inserted += self.insert_brain_journal_entry(&draft).await?;
        }
        Ok(inserted)
    }

    pub async fn brain_journal_for_date(
        &self,
        day: NaiveDate,
        page: i64,
        per_page: i64,
    ) -> Result<serde_json::Value> {
        let page = page.max(1);
        let per_page = per_page.clamp(10, 200);
        let offset = (page - 1) * per_page;
        let rows = sqlx::query(
            r#"SELECT id, journal_date, category, source_kind, source_id, event_key,
                    symbol, brain_thesis_id, thesis_id, title, summary, importance,
                    occurred_at, source_ref, created_at
               FROM brain_journal_entry
              WHERE journal_date = $1
           ORDER BY CASE category
                      WHEN 'changed' THEN 0
                      WHEN 'ignored_or_hated' THEN 1
                      WHEN 'crowded_or_extended' THEN 2
                      WHEN 'research' THEN 3
                      WHEN 'curious' THEN 4
                      WHEN 'blocked' THEN 5
                      ELSE 6
                    END,
                    importance DESC, occurred_at DESC, id DESC
              LIMIT $2 OFFSET $3"#,
        )
        .bind(day)
        .bind(per_page)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .context("brain_journal_for_date")?;

        let count_rows = sqlx::query(
            r#"SELECT category, count(*) AS n
                 FROM brain_journal_entry
                WHERE journal_date = $1
             GROUP BY category"#,
        )
        .bind(day)
        .fetch_all(&self.pool)
        .await
        .context("brain_journal_counts")?;
        let mut all_by_category = std::collections::BTreeMap::<String, i64>::new();
        for r in count_rows {
            all_by_category.insert(r.try_get("category")?, r.try_get("n")?);
        }
        let all_total: i64 = all_by_category.values().sum();

        let mut by_category = std::collections::BTreeMap::<String, i64>::new();
        let mut entries = Vec::with_capacity(rows.len());
        for r in rows {
            let category: String = r.try_get("category")?;
            *by_category.entry(category.clone()).or_default() += 1;
            let journal_date: NaiveDate = r.try_get("journal_date")?;
            let occurred_at: DateTime<Utc> = r.try_get("occurred_at")?;
            let created_at: DateTime<Utc> = r.try_get("created_at")?;
            entries.push(serde_json::json!({
                "id": r.try_get::<i64, _>("id")?,
                "date": journal_date.format("%Y-%m-%d").to_string(),
                "category": category,
                "source_kind": r.try_get::<String, _>("source_kind")?,
                "source_id": r.try_get::<String, _>("source_id")?,
                "event_key": r.try_get::<String, _>("event_key")?,
                "symbol": r.try_get::<Option<String>, _>("symbol")?,
                "brain_thesis_id": r.try_get::<Option<uuid::Uuid>, _>("brain_thesis_id")?,
                "thesis_id": r.try_get::<Option<uuid::Uuid>, _>("thesis_id")?,
                "title": r.try_get::<String, _>("title")?,
                "summary": r.try_get::<String, _>("summary")?,
                "importance": r.try_get::<i32, _>("importance")?,
                "occurred_at": occurred_at,
                "source_ref": r.try_get::<serde_json::Value, _>("source_ref")?,
                "created_at": created_at,
            }));
        }

        let visible_total = entries.len();
        let total_pages = if all_total == 0 {
            0
        } else {
            (all_total + per_page - 1) / per_page
        };
        let overview = self.brain_journal_overview(day, &all_by_category).await?;
        Ok(serde_json::json!({
            "as_of": Utc::now(),
            "date": day.format("%Y-%m-%d").to_string(),
            "synthesis": serde_json::Value::Null,
            "overview": overview,
            "summary": {
                "total": all_total,
                "visible": visible_total,
                "by_category": by_category,
                "all_by_category": all_by_category,
            },
            "pagination": {
                "page": page,
                "per_page": per_page,
                "total": all_total,
                "total_pages": total_pages,
                "has_previous": page > 1,
                "has_next": total_pages > 0 && page < total_pages,
            },
            "entries": entries,
        }))
    }

    async fn brain_journal_overview(
        &self,
        day: NaiveDate,
        counts: &std::collections::BTreeMap<String, i64>,
    ) -> Result<serde_json::Value> {
        let active = self
            .active_tickers()
            .await
            .context("journal_active_tickers")?;
        let mut top_candidates = Vec::new();
        let mut wait_for_setup = Vec::new();
        let mut risk_flags = Vec::new();
        let mut brief_consider = Vec::new();
        let mut brief_wait = Vec::new();
        let mut brief_avoid = Vec::new();
        let mut brief_research = Vec::new();

        for ticker in &active {
            let state = ticker.thesis_state.as_deref();
            let direction = ticker.thesis_direction.as_deref();
            let technical = ticker.technical_state.as_deref();
            let entry = ticker.entry_stance.as_deref();
            let freshness = Some(ticker.freshness_status.as_str());
            let score = journal_candidate_score(
                state,
                direction,
                technical,
                entry,
                freshness,
                ticker.tier,
                ticker.domain_fit,
            );
            let technical_read = journal_technical_read(
                technical,
                ticker.technical_setup_kind.as_deref(),
                entry,
                &ticker.technical_cross,
            );
            let item = serde_json::json!({
                "symbol": ticker.symbol.clone(),
                "score": score,
                "thesis_id": ticker.latest_thesis_id,
                "thesis_state": ticker.thesis_state.clone(),
                "thesis_direction": ticker.thesis_direction.clone(),
                "technical_state": ticker.technical_state.clone(),
                "technical_setup_kind": ticker.technical_setup_kind.clone(),
                "entry_stance": ticker.entry_stance.clone(),
                "technical_read": technical_read,
                "technical_panel_path": format!("/symbol/{}?p=technical", ticker.symbol),
                "technical_pct_vs_200d": ticker.technical_pct_vs_200d,
                "freshness_status": ticker.freshness_status.clone(),
                "open_attention": ticker.open_attention,
                "review_packet_attention_id": ticker.review_packet_attention_id,
                "open_evidence": ticker.open_evidence,
                "blocking_evidence": ticker.blocking_evidence,
                "due_source_tasks": ticker.due_source_tasks,
                "parent_themes": ticker.parent_themes.clone(),
                "reason": format!(
                    "{} thesis, {} direction, {} technicals, {} entry stance, {} freshness",
                    state.unwrap_or("no-state"),
                    direction.unwrap_or("no-direction"),
                    technical.unwrap_or("unknown"),
                    entry.unwrap_or("wait_data"),
                    ticker.freshness_status
                ),
            });

            let bullish = journal_direction_is_bullish(direction);
            let bearish = journal_direction_is_bearish(direction);
            let setup_wait = journal_waits_for_setup(technical, entry);
            let blocked_or_missing =
                matches!(ticker.freshness_status.as_str(), "blocked" | "missing")
                    || ticker.blocking_evidence > 0;
            let has_open_thesis = ticker.open_theses > 0;
            let trade_item = |stance| journal_trade_desk_item(ticker, score, stance);

            if !has_open_thesis {
                if ticker.open_attention > 0
                    || ticker.open_evidence > 0
                    || ticker.due_source_tasks > 0
                {
                    brief_research.push((score, trade_item("research")));
                }
                continue;
            }

            if setup_wait && (bullish || matches!(state, Some("actionable" | "armed"))) {
                wait_for_setup.push((score, item.clone()));
            } else if !bearish && !setup_wait && score >= 45 && !blocked_or_missing {
                top_candidates.push((score, item.clone()));
            }

            if bearish || matches!(technical, Some("deteriorating")) || blocked_or_missing {
                risk_flags.push((100 - score, item.clone()));
                brief_avoid.push((100 - score, trade_item("avoid")));
            } else if setup_wait || matches!(ticker.freshness_status.as_str(), "stale") {
                brief_wait.push((score, trade_item("wait")));
            } else if !bearish && score >= 45 {
                brief_consider.push((score, trade_item("consider")));
            } else {
                brief_research.push((score, trade_item("research")));
            }
        }

        top_candidates.sort_by(|a, b| b.0.cmp(&a.0));
        wait_for_setup.sort_by(|a, b| b.0.cmp(&a.0));
        risk_flags.sort_by(|a, b| b.0.cmp(&a.0));
        brief_consider.sort_by(|a, b| b.0.cmp(&a.0));
        brief_wait.sort_by(|a, b| b.0.cmp(&a.0));
        brief_avoid.sort_by(|a, b| b.0.cmp(&a.0));
        brief_research.sort_by(|a, b| b.0.cmp(&a.0));
        let top_candidates = top_candidates
            .into_iter()
            .take(6)
            .map(|(_, item)| item)
            .collect::<Vec<_>>();
        let wait_for_setup = wait_for_setup
            .into_iter()
            .take(6)
            .map(|(_, item)| item)
            .collect::<Vec<_>>();
        let risk_flags = risk_flags
            .into_iter()
            .take(6)
            .map(|(_, item)| item)
            .collect::<Vec<_>>();
        let decision_brief = serde_json::json!({
            "consider": brief_consider.into_iter().take(6).map(|(_, item)| item).collect::<Vec<_>>(),
            "wait": brief_wait.into_iter().take(6).map(|(_, item)| item).collect::<Vec<_>>(),
            "avoid": brief_avoid.into_iter().take(6).map(|(_, item)| item).collect::<Vec<_>>(),
            "research": brief_research.into_iter().take(6).map(|(_, item)| item).collect::<Vec<_>>(),
        });

        let market_state = sqlx::query(
            r#"SELECT as_of, regime, capitulation, indicators
                 FROM market_state
             ORDER BY as_of DESC
                LIMIT 1"#,
        )
        .fetch_optional(&self.pool)
        .await
        .context("journal_market_state")?
        .map(|r| {
            let as_of: DateTime<Utc> = r.try_get("as_of")?;
            Ok::<_, anyhow::Error>(serde_json::json!({
                "as_of": as_of,
                "regime": r.try_get::<String, _>("regime")?,
                "capitulation": r.try_get::<bool, _>("capitulation")?,
                "indicators": r.try_get::<serde_json::Value, _>("indicators")?,
            }))
        })
        .transpose()?;

        let macro_thesis = sqlx::query(
            r#"SELECT name, state, direction, summary, missing_evidence,
                      last_evaluated_at,
                      CASE
                        WHEN last_evaluated_at IS NULL THEN 'missing'
                        WHEN last_evaluated_at < now() - (freshness_target_minutes::text || ' minutes')::interval THEN 'stale'
                        ELSE 'fresh'
                      END AS freshness
                 FROM brain_thesis
                WHERE active = true AND scope = 'macro'
             ORDER BY updated_at DESC
                LIMIT 1"#,
        )
        .fetch_optional(&self.pool)
        .await
        .context("journal_macro_thesis")?
        .map(|r| {
            let last_evaluated_at: Option<DateTime<Utc>> = r.try_get("last_evaluated_at")?;
            Ok::<_, anyhow::Error>(serde_json::json!({
                "name": r.try_get::<String, _>("name")?,
                "state": r.try_get::<String, _>("state")?,
                "direction": r.try_get::<String, _>("direction")?,
                "summary": r.try_get::<String, _>("summary")?,
                "missing_evidence": r.try_get::<serde_json::Value, _>("missing_evidence")?,
                "last_evaluated_at": last_evaluated_at,
                "freshness": r.try_get::<String, _>("freshness")?,
            }))
        })
        .transpose()?;

        let theme_rows = sqlx::query(
            r#"SELECT bt.name, bt.scope, bt.state, bt.direction, bt.summary,
                      bt.missing_evidence,
                      (SELECT count(*) FROM brain_thesis_ticker btt WHERE btt.brain_thesis_id = bt.id) AS linked_tickers,
                      CASE
                        WHEN bt.last_evaluated_at IS NULL THEN 'missing'
                        WHEN bt.last_evaluated_at < now() - (bt.freshness_target_minutes::text || ' minutes')::interval THEN 'stale'
                        ELSE 'fresh'
                      END AS freshness
                 FROM brain_thesis bt
                WHERE bt.active = true
                  AND bt.scope <> 'macro'
             ORDER BY CASE
                        WHEN bt.state IN ('active', 'forming') THEN 0
                        WHEN bt.state = 'weakening' THEN 1
                        ELSE 2
                      END,
                      CASE
                        WHEN bt.last_evaluated_at IS NULL THEN 1
                        WHEN bt.last_evaluated_at < now() - (bt.freshness_target_minutes::text || ' minutes')::interval THEN 1
                        ELSE 0
                      END,
                      bt.updated_at DESC
                LIMIT 6"#,
        )
        .fetch_all(&self.pool)
        .await
        .context("journal_theme_rows")?;
        let themes = theme_rows
            .into_iter()
            .map(|r| {
                Ok(serde_json::json!({
                    "name": r.try_get::<String, _>("name")?,
                    "scope": r.try_get::<String, _>("scope")?,
                    "state": r.try_get::<String, _>("state")?,
                    "direction": r.try_get::<String, _>("direction")?,
                    "summary": r.try_get::<String, _>("summary")?,
                    "missing_evidence": r.try_get::<serde_json::Value, _>("missing_evidence")?,
                    "freshness": r.try_get::<String, _>("freshness")?,
                    "linked_tickers": r.try_get::<i64, _>("linked_tickers")?,
                }))
            })
            .collect::<Result<Vec<_>>>()?;

        let start_naive = day
            .and_hms_opt(0, 0, 0)
            .ok_or_else(|| anyhow::anyhow!("invalid journal day"))?;
        let start = DateTime::<Utc>::from_naive_utc_and_offset(start_naive, Utc);
        let end = start + ChronoDuration::days(1);
        let evidence_rows = sqlx::query(
            r#"SELECT symbol, kind, summary, source, url, strength, polarity, observed_at
                 FROM evidence_item
                WHERE (
                        created_at >= $1 AND created_at < $2
                      )
                  AND kind IN ('news', 'product_research', 'estimate_revision', 'rating_change', 'filing')
             ORDER BY COALESCE(strength, 0.5) DESC,
                      abs(COALESCE(polarity, 0.0)) DESC,
                      observed_at DESC
                LIMIT 8"#,
        )
        .bind(start)
        .bind(end)
        .fetch_all(&self.pool)
        .await
        .context("journal_news_recap")?;
        let news_recap = evidence_rows
            .into_iter()
            .map(|r| {
                let observed_at: DateTime<Utc> = r.try_get("observed_at")?;
                Ok(serde_json::json!({
                    "symbol": r.try_get::<String, _>("symbol")?,
                    "kind": r.try_get::<String, _>("kind")?,
                    "summary": r.try_get::<String, _>("summary")?,
                    "source": r.try_get::<String, _>("source")?,
                    "url": r.try_get::<Option<String>, _>("url")?,
                    "strength": r.try_get::<Option<f64>, _>("strength")?,
                    "polarity": r.try_get::<Option<f64>, _>("polarity")?,
                    "observed_at": observed_at,
                }))
            })
            .collect::<Result<Vec<_>>>()?;

        let focus_rows = sqlx::query(
            r#"SELECT category, source_kind, source_id, symbol, title, summary,
                      importance, occurred_at
                 FROM brain_journal_entry
                WHERE journal_date = $1
                  AND category IN ('research', 'curious', 'blocked')
             ORDER BY CASE category
                        WHEN 'blocked' THEN 0
                        WHEN 'research' THEN 1
                        ELSE 2
                      END,
                      importance DESC,
                      occurred_at DESC
                LIMIT 8"#,
        )
        .bind(day)
        .fetch_all(&self.pool)
        .await
        .context("journal_research_focus")?;
        let research_focus = focus_rows
            .into_iter()
            .map(|r| {
                let occurred_at: DateTime<Utc> = r.try_get("occurred_at")?;
                Ok(serde_json::json!({
                    "category": r.try_get::<String, _>("category")?,
                    "source_kind": r.try_get::<String, _>("source_kind")?,
                    "source_id": r.try_get::<String, _>("source_id")?,
                    "symbol": r.try_get::<Option<String>, _>("symbol")?,
                    "title": r.try_get::<String, _>("title")?,
                    "summary": r.try_get::<String, _>("summary")?,
                    "importance": r.try_get::<i32, _>("importance")?,
                    "occurred_at": occurred_at,
                }))
            })
            .collect::<Result<Vec<_>>>()?;

        let market_regime = market_state
            .as_ref()
            .and_then(|v| v.get("regime"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown");
        let macro_direction = macro_thesis
            .as_ref()
            .and_then(|v| v.get("direction"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown");
        let blocked = counts.get("blocked").copied().unwrap_or(0);
        let changed = counts.get("changed").copied().unwrap_or(0);
        let top_candidates_count = top_candidates.len();
        let wait_for_setup_count = wait_for_setup.len();
        let risk_flags_count = risk_flags.len();
        let news_recap_count = news_recap.len();
        let research_focus_count = research_focus.len();
        let headline = if top_candidates.is_empty() {
            format!(
                "No clean entry candidates surfaced; {wait_count} bullish or active names need setup and {blocked} blocker(s) need attention.",
                wait_count = wait_for_setup_count
            )
        } else {
            format!(
                "{} clean candidate(s), {} setup wait(s), {} changed item(s), {} blocker(s).",
                top_candidates_count, wait_for_setup_count, changed, blocked
            )
        };

        Ok(serde_json::json!({
            "as_of": Utc::now(),
            "headline": headline,
            "market": {
                "label": format!("market {market_regime} · macro {macro_direction}"),
                "regime": market_regime,
                "macro_direction": macro_direction,
                "state": macro_thesis
                    .as_ref()
                    .and_then(|v| v.get("state"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("missing"),
                "freshness": macro_thesis
                    .as_ref()
                    .and_then(|v| v.get("freshness"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("missing"),
                "summary": macro_thesis
                    .as_ref()
                    .and_then(|v| v.get("summary"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("No macro thesis is active."),
                "missing_evidence": macro_thesis
                    .as_ref()
                    .and_then(|v| v.get("missing_evidence"))
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!([])),
                "market_state": market_state,
            },
            "decision_brief": decision_brief,
            "top_candidates": top_candidates,
            "wait_for_setup": wait_for_setup,
            "risk_flags": risk_flags,
            "themes": themes,
            "news_recap": news_recap,
            "research_focus": research_focus,
            "counts": {
                "active_universe": active.len(),
                "top_candidates": top_candidates_count,
                "wait_for_setup": wait_for_setup_count,
                "risk_flags": risk_flags_count,
                "news_recap": news_recap_count,
                "research_focus": research_focus_count,
                "changed": changed,
                "blocked": blocked,
            },
        }))
    }

    async fn insert_brain_journal_entry(&self, draft: &BrainJournalDraft) -> Result<u64> {
        let res = sqlx::query(
            r#"INSERT INTO brain_journal_entry
                    (journal_date, category, source_kind, source_id, event_key, symbol,
                     brain_thesis_id, thesis_id, title, summary, importance, occurred_at,
                     source_ref)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
            ON CONFLICT (event_key) DO NOTHING"#,
        )
        .bind(draft.journal_date)
        .bind(&draft.category)
        .bind(&draft.source_kind)
        .bind(&draft.source_id)
        .bind(&draft.event_key)
        .bind(&draft.symbol)
        .bind(draft.brain_thesis_id)
        .bind(draft.thesis_id)
        .bind(&draft.title)
        .bind(&draft.summary)
        .bind(draft.importance)
        .bind(draft.occurred_at)
        .bind(&draft.source_ref)
        .execute(&self.pool)
        .await
        .context("insert_brain_journal_entry")?;
        Ok(res.rows_affected())
    }

    async fn brain_journal_attention_drafts(
        &self,
        day: NaiveDate,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<BrainJournalDraft>> {
        let rows = sqlx::query(
            r#"SELECT id, kind, symbol, thesis_id, candidate_id, severity, status,
                      title, reason, source, source_ref, created_at, resolved_at,
                      resolution_kind, fsm_state, owner
                 FROM attention_item
                WHERE created_at >= $1 AND created_at < $2
             ORDER BY created_at DESC
                LIMIT 80"#,
        )
        .bind(start)
        .bind(end)
        .fetch_all(&self.pool)
        .await
        .context("brain_journal_attention_drafts")?;

        rows.into_iter()
            .map(|r| {
                let id: i64 = r.try_get("id")?;
                let kind: String = r.try_get("kind")?;
                let severity: String = r.try_get("severity")?;
                let created_at: DateTime<Utc> = r.try_get("created_at")?;
                let title: String = r.try_get("title")?;
                let reason: Option<String> = r.try_get("reason")?;
                let (category, importance) = journal_attention_category(&kind, &severity);
                Ok(BrainJournalDraft {
                    journal_date: day,
                    category: category.to_string(),
                    source_kind: "attention".to_string(),
                    source_id: id.to_string(),
                    event_key: journal_event_key("attention", id, created_at),
                    symbol: r.try_get("symbol")?,
                    brain_thesis_id: None,
                    thesis_id: r.try_get("thesis_id")?,
                    title,
                    summary: reason.unwrap_or_else(|| {
                        format!("{} attention item from {}", journal_label(&kind), severity)
                    }),
                    importance,
                    occurred_at: created_at,
                    source_ref: serde_json::json!({
                        "attention_id": id,
                        "kind": kind,
                        "severity": severity,
                        "status": r.try_get::<String, _>("status")?,
                        "source": r.try_get::<String, _>("source")?,
                        "candidate_id": r.try_get::<Option<i64>, _>("candidate_id")?,
                        "fsm_state": r.try_get::<Option<String>, _>("fsm_state").ok().flatten(),
                        "owner": r.try_get::<Option<String>, _>("owner").ok().flatten(),
                        "resolved_at": r.try_get::<Option<DateTime<Utc>>, _>("resolved_at")?,
                        "resolution_kind": r.try_get::<Option<String>, _>("resolution_kind")?,
                        "source_ref": r.try_get::<serde_json::Value, _>("source_ref")?,
                    }),
                })
            })
            .collect()
    }

    async fn brain_journal_source_task_drafts(
        &self,
        day: NaiveDate,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<BrainJournalDraft>> {
        let rows = sqlx::query(
            r#"SELECT id, source_type, requirement_key, action, scope, target_id,
                      provider, state, priority, attempts, next_retry_at, last_error,
                      source_ref, source_ref->>'result' AS result, updated_at
                 FROM source_task
                WHERE updated_at >= $1 AND updated_at < $2
                  AND (
                    state IN ('failed', 'blocked', 'rate_limited', 'no_rows')
                    OR (state = 'satisfied' AND source_ref->>'result' = 'rows_seen')
                    OR (state IN ('queued', 'fetching') AND priority IN ('high', 'blocking'))
                  )
             ORDER BY updated_at DESC
                LIMIT 120"#,
        )
        .bind(start)
        .bind(end)
        .fetch_all(&self.pool)
        .await
        .context("brain_journal_source_task_drafts")?;

        rows.into_iter()
            .map(|r| {
                let id: i64 = r.try_get("id")?;
                let state: String = r.try_get("state")?;
                let priority: String = r.try_get("priority")?;
                let result: Option<String> = r.try_get("result")?;
                let updated_at: DateTime<Utc> = r.try_get("updated_at")?;
                let scope: String = r.try_get("scope")?;
                let target: String = r.try_get("target_id")?;
                let provider: String = r.try_get("provider")?;
                let action: String = r.try_get("action")?;
                let (category, importance) =
                    journal_source_task_category(&state, result.as_deref(), &priority);
                let symbol = if scope == "symbol" {
                    Some(target.clone())
                } else {
                    None
                };
                let title = match category {
                    "changed" => format!("Fresh {provider} data for {target}"),
                    "blocked" => format!("Data blocked: {target} {}", journal_label(&action)),
                    "research" if state == "fetching" => {
                        format!("Research in progress: {target} {}", journal_label(&action))
                    }
                    "research" => format!("Research queued: {target} {}", journal_label(&action)),
                    _ => format!("No new rows: {target} {}", journal_label(&action)),
                };
                let mut summary = format!(
                    "{} task {} with {} priority after {} attempt(s)",
                    provider,
                    state,
                    priority,
                    r.try_get::<i32, _>("attempts")?
                );
                if let Some(error) = r.try_get::<Option<String>, _>("last_error")? {
                    summary = format!("{summary}: {error}");
                } else if let Some(result) = result.as_deref() {
                    summary = format!("{summary}; result {result}");
                }
                Ok(BrainJournalDraft {
                    journal_date: day,
                    category: category.to_string(),
                    source_kind: "source_task".to_string(),
                    source_id: id.to_string(),
                    event_key: journal_event_key("source_task", id, updated_at),
                    symbol,
                    brain_thesis_id: None,
                    thesis_id: None,
                    title,
                    summary,
                    importance,
                    occurred_at: updated_at,
                    source_ref: serde_json::json!({
                        "source_task_id": id,
                        "source_type": r.try_get::<String, _>("source_type")?,
                        "requirement_key": r.try_get::<Option<String>, _>("requirement_key")?,
                        "action": action,
                        "scope": scope,
                        "target_id": target,
                        "provider": provider,
                        "state": state,
                        "priority": priority,
                        "result": result,
                        "next_retry_at": r.try_get::<Option<DateTime<Utc>>, _>("next_retry_at")?,
                        "source_ref": r.try_get::<serde_json::Value, _>("source_ref")?,
                    }),
                })
            })
            .collect()
    }

    async fn brain_journal_thesis_state_drafts(
        &self,
        day: NaiveDate,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<BrainJournalDraft>> {
        let rows = sqlx::query(
            r#"SELECT tsh.id, t.symbol, tsh.thesis_id, tsh.from_state, tsh.to_state,
                      tsh.rationale, tsh.at
                 FROM thesis_state_history tsh
                 JOIN thesis t ON t.thesis_id = tsh.thesis_id
                WHERE tsh.at >= $1 AND tsh.at < $2
             ORDER BY tsh.at DESC
                LIMIT 80"#,
        )
        .bind(start)
        .bind(end)
        .fetch_all(&self.pool)
        .await
        .context("brain_journal_thesis_state_drafts")?;

        rows.into_iter()
            .map(|r| {
                let id: i64 = r.try_get("id")?;
                let symbol: String = r.try_get("symbol")?;
                let to_state: String = r.try_get("to_state")?;
                let at: DateTime<Utc> = r.try_get("at")?;
                let rationale: Option<String> = r.try_get("rationale")?;
                Ok(BrainJournalDraft {
                    journal_date: day,
                    category: "changed".to_string(),
                    source_kind: "thesis_state".to_string(),
                    source_id: id.to_string(),
                    event_key: journal_event_key("thesis_state", id, at),
                    symbol: Some(symbol.clone()),
                    brain_thesis_id: None,
                    thesis_id: r.try_get("thesis_id")?,
                    title: format!("{symbol} thesis moved to {}", journal_label(&to_state)),
                    summary: rationale.unwrap_or_else(|| {
                        "State transition recorded by thesis lifecycle.".to_string()
                    }),
                    importance: journal_thesis_state_importance(&to_state),
                    occurred_at: at,
                    source_ref: serde_json::json!({
                        "thesis_state_history_id": id,
                        "from_state": r.try_get::<Option<String>, _>("from_state")?,
                        "to_state": to_state,
                    }),
                })
            })
            .collect()
    }

    async fn brain_journal_thesis_version_drafts(
        &self,
        day: NaiveDate,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<BrainJournalDraft>> {
        let rows = sqlx::query(
            r#"SELECT tvh.id, t.symbol, tvh.thesis_id, tvh.version, tvh.diff,
                      tvh.rationale, tvh.weakens_invalidation, tvh.at
                 FROM thesis_version_history tvh
                 JOIN thesis t ON t.thesis_id = tvh.thesis_id
                WHERE tvh.at >= $1 AND tvh.at < $2
             ORDER BY tvh.at DESC
                LIMIT 80"#,
        )
        .bind(start)
        .bind(end)
        .fetch_all(&self.pool)
        .await
        .context("brain_journal_thesis_version_drafts")?;

        rows.into_iter()
            .map(|r| {
                let id: i64 = r.try_get("id")?;
                let symbol: String = r.try_get("symbol")?;
                let version: i32 = r.try_get("version")?;
                let at: DateTime<Utc> = r.try_get("at")?;
                let weakens: bool = r.try_get("weakens_invalidation")?;
                Ok(BrainJournalDraft {
                    journal_date: day,
                    category: "changed".to_string(),
                    source_kind: "thesis_version".to_string(),
                    source_id: id.to_string(),
                    event_key: journal_event_key("thesis_version", id, at),
                    symbol: Some(symbol.clone()),
                    brain_thesis_id: None,
                    thesis_id: r.try_get("thesis_id")?,
                    title: format!("{symbol} thesis updated to v{version}"),
                    summary: r
                        .try_get::<Option<String>, _>("rationale")?
                        .unwrap_or_else(|| {
                            "Thesis content changed; review the version diff.".to_string()
                        }),
                    importance: if weakens { 88 } else { 72 },
                    occurred_at: at,
                    source_ref: serde_json::json!({
                        "thesis_version_history_id": id,
                        "version": version,
                        "weakens_invalidation": weakens,
                        "diff": r.try_get::<serde_json::Value, _>("diff")?,
                    }),
                })
            })
            .collect()
    }

    async fn brain_journal_evidence_drafts(
        &self,
        day: NaiveDate,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<BrainJournalDraft>> {
        let rows = sqlx::query(
            r#"SELECT id, symbol, kind, observed_at, source, source_id, source_ref,
                      summary, strength, polarity, url, created_at
                 FROM evidence_item
                WHERE created_at >= $1 AND created_at < $2
             ORDER BY created_at DESC
                LIMIT 120"#,
        )
        .bind(start)
        .bind(end)
        .fetch_all(&self.pool)
        .await
        .context("brain_journal_evidence_drafts")?;

        rows.into_iter()
            .map(|r| {
                let id: i64 = r.try_get("id")?;
                let symbol: String = r.try_get("symbol")?;
                let kind: String = r.try_get("kind")?;
                let created_at: DateTime<Utc> = r.try_get("created_at")?;
                let strength: Option<f64> = r.try_get("strength")?;
                let polarity: Option<f64> = r.try_get("polarity")?;
                let category = if kind == "product_research" {
                    "curious"
                } else {
                    "changed"
                };
                let mut importance = (strength.unwrap_or(0.5) * 70.0).round() as i32;
                if polarity.unwrap_or(0.0).abs() >= 0.5 {
                    importance += 10;
                }
                Ok(BrainJournalDraft {
                    journal_date: day,
                    category: category.to_string(),
                    source_kind: "evidence".to_string(),
                    source_id: id.to_string(),
                    event_key: journal_event_key("evidence", id, created_at),
                    symbol: Some(symbol.clone()),
                    brain_thesis_id: None,
                    thesis_id: None,
                    title: format!("{symbol} evidence: {}", journal_label(&kind)),
                    summary: r.try_get("summary")?,
                    importance: importance.clamp(35, 85),
                    occurred_at: created_at,
                    source_ref: serde_json::json!({
                        "evidence_item_id": id,
                        "kind": kind,
                        "observed_at": r.try_get::<DateTime<Utc>, _>("observed_at")?,
                        "source": r.try_get::<String, _>("source")?,
                        "source_id": r.try_get::<String, _>("source_id")?,
                        "strength": strength,
                        "polarity": polarity,
                        "url": r.try_get::<Option<String>, _>("url")?,
                        "source_ref": r.try_get::<serde_json::Value, _>("source_ref")?,
                    }),
                })
            })
            .collect()
    }

    async fn brain_journal_parent_thesis_drafts(
        &self,
        day: NaiveDate,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<BrainJournalDraft>> {
        let rows = sqlx::query(
            r#"SELECT btvh.id, btvh.brain_thesis_id, bt.scope, bt.key, bt.name,
                      btvh.version, btvh.diff, btvh.rationale, btvh.at
                 FROM brain_thesis_version_history btvh
                 JOIN brain_thesis bt ON bt.id = btvh.brain_thesis_id
                WHERE btvh.at >= $1 AND btvh.at < $2
             ORDER BY btvh.at DESC
                LIMIT 80"#,
        )
        .bind(start)
        .bind(end)
        .fetch_all(&self.pool)
        .await
        .context("brain_journal_parent_thesis_drafts")?;

        rows.into_iter()
            .map(|r| {
                let id: i64 = r.try_get("id")?;
                let name: String = r.try_get("name")?;
                let scope: String = r.try_get("scope")?;
                let version: i32 = r.try_get("version")?;
                let at: DateTime<Utc> = r.try_get("at")?;
                Ok(BrainJournalDraft {
                    journal_date: day,
                    category: "changed".to_string(),
                    source_kind: "brain_thesis".to_string(),
                    source_id: id.to_string(),
                    event_key: journal_event_key("brain_thesis", id, at),
                    symbol: None,
                    brain_thesis_id: r.try_get("brain_thesis_id")?,
                    thesis_id: None,
                    title: format!("{} parent thesis updated: {name}", journal_label(&scope)),
                    summary: r
                        .try_get::<Option<String>, _>("rationale")?
                        .unwrap_or_else(|| format!("{name} moved to parent thesis v{version}.")),
                    importance: if scope == "macro" { 85 } else { 74 },
                    occurred_at: at,
                    source_ref: serde_json::json!({
                        "brain_thesis_version_history_id": id,
                        "scope": scope,
                        "key": r.try_get::<String, _>("key")?,
                        "version": version,
                        "diff": r.try_get::<serde_json::Value, _>("diff")?,
                    }),
                })
            })
            .collect()
    }

    async fn brain_journal_dislocation_drafts(
        &self,
        day: NaiveDate,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<BrainJournalDraft>> {
        let rows = sqlx::query(
            r#"SELECT id, name, source_ref, updated_at
                 FROM brain_thesis
                WHERE active = true
                  AND scope = 'macro'
                  AND updated_at >= $1 AND updated_at < $2
                  AND source_ref #> '{maintainer,dislocation_map,buckets}' IS NOT NULL
             ORDER BY updated_at DESC
                LIMIT 5"#,
        )
        .bind(start)
        .bind(end)
        .fetch_all(&self.pool)
        .await
        .context("brain_journal_dislocation_drafts")?;

        let mut drafts = Vec::new();
        for r in rows {
            let brain_thesis_id: uuid::Uuid = r.try_get("id")?;
            let name: String = r.try_get("name")?;
            let source_ref: serde_json::Value = r.try_get("source_ref")?;
            let updated_at: DateTime<Utc> = r.try_get("updated_at")?;
            let buckets = source_ref
                .pointer("/maintainer/dislocation_map/buckets")
                .and_then(serde_json::Value::as_object);
            let Some(buckets) = buckets else {
                continue;
            };
            for (bucket, label, category) in [
                ("loved_mania", "Loved / mania", "crowded_or_extended"),
                ("ignored_indifference", "Ignored", "ignored_or_hated"),
                ("hated_avoided", "Hated / avoided", "ignored_or_hated"),
            ] {
                let items = buckets
                    .get(bucket)
                    .and_then(serde_json::Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                if items.is_empty() {
                    continue;
                }
                let names: Vec<String> = items
                    .iter()
                    .filter_map(|item| {
                        item.get("name")
                            .or_else(|| item.get("sector"))
                            .and_then(serde_json::Value::as_str)
                            .map(ToOwned::to_owned)
                    })
                    .take(4)
                    .collect();
                let first_note = items
                    .iter()
                    .filter_map(|item| {
                        item.get("interpretation")
                            .and_then(serde_json::Value::as_str)
                            .or_else(|| {
                                item.get("reasons")
                                    .and_then(serde_json::Value::as_array)
                                    .and_then(|reasons| reasons.first())
                                    .and_then(serde_json::Value::as_str)
                            })
                    })
                    .next()
                    .unwrap_or("Macro dislocation map flagged this bucket.")
                    .to_string();
                drafts.push(BrainJournalDraft {
                    journal_date: day,
                    category: category.to_string(),
                    source_kind: "brain_thesis".to_string(),
                    source_id: format!("{brain_thesis_id}:{bucket}"),
                    event_key: journal_event_key(
                        "brain_dislocation",
                        format!("{brain_thesis_id}:{bucket}"),
                        updated_at,
                    ),
                    symbol: None,
                    brain_thesis_id: Some(brain_thesis_id),
                    thesis_id: None,
                    title: format!("{label}: {}", names.join(", ")),
                    summary: format!("{name} flags this pocket: {first_note}"),
                    importance: if category == "crowded_or_extended" {
                        78
                    } else {
                        82
                    },
                    occurred_at: updated_at,
                    source_ref: serde_json::json!({
                        "brain_thesis_id": brain_thesis_id,
                        "bucket": bucket,
                        "items": items,
                    }),
                });
            }
        }
        Ok(drafts)
    }

    /// Daily-or-higher candles for `symbol` over the last `lookback_days`, oldest first.
    /// Shaped for lightweight-charts (each row has `time` as ISO date + OHLCV).
    ///
    /// `price_bar` can contain multiple timestamps on the same UTC date when
    /// backfills and refreshes come from different feeds. The chart library
    /// requires strictly increasing unique times, so collapse bars to one
    /// candle per date at the API boundary.
    pub async fn candles_for(
        &self,
        symbol: &str,
        lookback_days: i64,
        interval: &str,
    ) -> Result<Vec<serde_json::Value>> {
        let rows = sqlx::query(
            r#"WITH daily AS (
                 SELECT (date_trunc('day', ts AT TIME ZONE 'UTC'))::date AS day,
                        (array_agg(open::float8 ORDER BY ts ASC))[1] AS open,
                        max(high::float8) AS high,
                        min(low::float8) AS low,
                        (array_agg(close::float8 ORDER BY ts DESC))[1] AS close,
                        sum(volume::float8) AS volume
                   FROM price_bar
                  WHERE symbol = $1
                    AND ts > now() - ($2 || ' days')::interval
               GROUP BY 1
             ), bucketed AS (
                 SELECT CASE
                          WHEN $3 = '1W' THEN date_trunc('week', day::timestamp)::date
                          WHEN $3 = '3W' THEN (DATE '1970-01-05' + ((((day - DATE '1970-01-05') / 21)::int) * 21))
                          WHEN $3 = '1M' THEN date_trunc('month', day::timestamp)::date
                          ELSE day
                        END AS bucket,
                        day, open, high, low, close, volume
                   FROM daily
             )
             SELECT bucket AS day,
                    (array_agg(open ORDER BY day ASC))[1] AS open,
                    max(high) AS high,
                    min(low) AS low,
                    (array_agg(close ORDER BY day DESC))[1] AS close,
                    sum(volume) AS volume
               FROM bucketed
              GROUP BY bucket
              ORDER BY bucket ASC"#,
        )
        .bind(symbol)
        .bind(lookback_days.to_string())
        .bind(interval)
        .fetch_all(&self.pool)
        .await
        .context("candles_for")?;
        rows.into_iter()
            .map(|r| {
                let day: chrono::NaiveDate = r.try_get("day")?;
                Ok(serde_json::json!({
                    "time": day.format("%Y-%m-%d").to_string(),
                    "open": r.try_get::<f64, _>("open")?,
                    "high": r.try_get::<f64, _>("high")?,
                    "low": r.try_get::<f64, _>("low")?,
                    "close": r.try_get::<f64, _>("close")?,
                    "volume": r.try_get::<f64, _>("volume")?,
                }))
            })
            .collect()
    }

    pub async fn latest_intraday_bar_ts(
        &self,
        symbol: &str,
        native_interval: &str,
    ) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
        let row = sqlx::query(
            "SELECT max(ts) AS ts FROM price_bar_intraday WHERE symbol = $1 AND interval = $2",
        )
        .bind(symbol)
        .bind(native_interval)
        .fetch_one(&self.pool)
        .await
        .context("latest_intraday_bar_ts")?;
        Ok(row.try_get("ts")?)
    }

    pub async fn intraday_bar_coverage(
        &self,
        symbol: &str,
        native_interval: &str,
    ) -> Result<IntradayBarCoverage> {
        let row = sqlx::query(
            r#"SELECT min(ts) AS oldest, max(ts) AS latest, count(*)::int8 AS bars
                 FROM price_bar_intraday
                WHERE symbol = $1 AND interval = $2"#,
        )
        .bind(symbol)
        .bind(native_interval)
        .fetch_one(&self.pool)
        .await
        .context("intraday_bar_coverage")?;
        Ok(IntradayBarCoverage {
            oldest: row.try_get("oldest")?,
            latest: row.try_get("latest")?,
            bars: row.try_get::<i64, _>("bars")?,
        })
    }

    pub async fn intraday_candles_for(
        &self,
        symbol: &str,
        native_interval: &str,
        lookback_days: i64,
        bucket_minutes: i64,
    ) -> Result<Vec<serde_json::Value>> {
        let rows = sqlx::query(
            r#"WITH bucketed AS (
                 SELECT to_timestamp(floor(extract(epoch FROM ts) / ($4::float8 * 60.0)) * ($4::float8 * 60.0)) AS bucket,
                        ts, open::float8 AS open, high::float8 AS high, low::float8 AS low,
                        close::float8 AS close, volume::float8 AS volume
                   FROM price_bar_intraday
                  WHERE symbol = $1
                    AND interval = $2
                    AND ts > now() - ($3 || ' days')::interval
             )
             SELECT bucket,
                    (array_agg(open ORDER BY ts ASC))[1] AS open,
                    max(high) AS high,
                    min(low) AS low,
                    (array_agg(close ORDER BY ts DESC))[1] AS close,
                    sum(volume) AS volume
               FROM bucketed
              GROUP BY bucket
              ORDER BY bucket ASC"#,
        )
        .bind(symbol)
        .bind(native_interval)
        .bind(lookback_days.to_string())
        .bind(bucket_minutes)
        .fetch_all(&self.pool)
        .await
        .context("intraday_candles_for")?;
        rows.into_iter()
            .map(|r| {
                let bucket: chrono::DateTime<chrono::Utc> = r.try_get("bucket")?;
                Ok(serde_json::json!({
                    "time": bucket.to_rfc3339(),
                    "open": r.try_get::<f64, _>("open")?,
                    "high": r.try_get::<f64, _>("high")?,
                    "low": r.try_get::<f64, _>("low")?,
                    "close": r.try_get::<f64, _>("close")?,
                    "volume": r.try_get::<f64, _>("volume")?,
                }))
            })
            .collect()
    }

    pub async fn daily_technical_bars_for(
        &self,
        symbol: &str,
        lookback_days: i64,
    ) -> Result<Vec<TechnicalBar>> {
        let rows = sqlx::query(
            r#"WITH daily AS (
                 SELECT (date_trunc('day', ts AT TIME ZONE 'UTC'))::date AS day,
                        (array_agg(ts ORDER BY ts DESC))[1] AS ts,
                        max(high::float8) AS high,
                        min(low::float8) AS low,
                        (array_agg(close::float8 ORDER BY ts DESC))[1] AS close,
                        sum(volume::float8) AS volume
                   FROM price_bar
                  WHERE symbol = $1
                    AND ts > now() - ($2 || ' days')::interval
               GROUP BY 1
             )
             SELECT ts, close, high, low, volume
               FROM daily
              ORDER BY day ASC"#,
        )
        .bind(symbol)
        .bind(lookback_days.to_string())
        .fetch_all(&self.pool)
        .await
        .context("daily_technical_bars_for")?;
        rows.into_iter()
            .map(|r| {
                Ok(TechnicalBar {
                    ts: r.try_get("ts")?,
                    close: r.try_get("close")?,
                    high: r.try_get("high")?,
                    low: r.try_get("low")?,
                    volume: r.try_get("volume")?,
                })
            })
            .collect()
    }

    pub async fn record_technical_timing_observations(
        &self,
        state: &TechnicalState,
        benchmarks: &[(&str, Vec<TechnicalBar>)],
    ) -> Result<usize> {
        if !technical_timing::tracks_state(&state.state, &state.setup.kind) {
            return Ok(0);
        }
        let Some(observed_at) = state.as_of else {
            return Ok(0);
        };
        let Some(daily) = state.daily.as_ref() else {
            return Ok(0);
        };
        if daily.close <= 0.0 {
            return Ok(0);
        }

        let horizon_bars = 20_i64;
        let input_snapshot = serde_json::to_value(state).context("encode technical state")?;
        let mut inserted = 0_usize;
        if benchmarks.is_empty() {
            inserted += self
                .insert_technical_timing_observation(
                    state,
                    observed_at,
                    daily.close,
                    "none",
                    None,
                    serde_json::json!({ "status": "missing" }),
                    horizon_bars,
                    &input_snapshot,
                )
                .await?;
        } else {
            for (benchmark_symbol, bars) in benchmarks {
                let benchmark = bars.last();
                inserted += self
                    .insert_technical_timing_observation(
                        state,
                        observed_at,
                        daily.close,
                        benchmark_symbol,
                        benchmark.map(|bar| bar.close).filter(|close| *close > 0.0),
                        serde_json::json!({
                            "symbol": benchmark_symbol,
                            "as_of": benchmark.map(|bar| bar.ts),
                            "close": benchmark.map(|bar| bar.close),
                        }),
                        horizon_bars,
                        &input_snapshot,
                    )
                    .await?;
            }
        }
        Ok(inserted)
    }

    async fn insert_technical_timing_observation(
        &self,
        state: &TechnicalState,
        observed_at: DateTime<Utc>,
        close: f64,
        benchmark_symbol: &str,
        benchmark_close: Option<f64>,
        benchmark_snapshot: serde_json::Value,
        horizon_bars: i64,
        input_snapshot: &serde_json::Value,
    ) -> Result<usize> {
        let evaluation_due_at = technical_timing::due_at(observed_at, horizon_bars);
        let row: Option<uuid::Uuid> = sqlx::query_scalar(
            r#"INSERT INTO technical_timing_observation
                 (symbol, observed_at, technical_state, setup_kind, entry_stance,
                  close, benchmark_symbol, benchmark_close, horizon_bars,
                  evaluation_due_at, input_snapshot, benchmark_snapshot)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                       $11::jsonb, $12::jsonb)
            ON CONFLICT DO NOTHING
            RETURNING observation_id"#,
        )
        .bind(&state.symbol)
        .bind(observed_at)
        .bind(&state.state)
        .bind(&state.setup.kind)
        .bind(&state.setup.entry_stance)
        .bind(close)
        .bind(benchmark_symbol)
        .bind(benchmark_close)
        .bind(i32::try_from(horizon_bars).unwrap_or(20))
        .bind(evaluation_due_at)
        .bind(input_snapshot)
        .bind(&benchmark_snapshot)
        .fetch_optional(&self.pool)
        .await
        .context("insert technical timing observation")?;
        Ok(usize::from(row.is_some()))
    }

    pub async fn score_due_technical_timing_observations(&self, limit: i64) -> Result<usize> {
        let rows = sqlx::query(
            r#"SELECT observation_id,
                      symbol,
                      observed_at,
                      close::float8 AS close,
                      benchmark_symbol,
                      benchmark_close::float8 AS benchmark_close,
                      horizon_bars
                 FROM technical_timing_observation
                WHERE evaluated_at IS NULL
                  AND evaluation_due_at <= now()
             ORDER BY evaluation_due_at ASC, created_at ASC
                LIMIT $1"#,
        )
        .bind(limit.clamp(1, 1000))
        .fetch_all(&self.pool)
        .await
        .context("select due technical timing observations")?;

        let mut scored = 0_usize;
        for row in rows {
            let observation_id: uuid::Uuid = row.try_get("observation_id")?;
            let symbol: String = row.try_get("symbol")?;
            let observed_at: DateTime<Utc> = row.try_get("observed_at")?;
            let close: f64 = row.try_get("close")?;
            let horizon_bars: i32 = row.try_get("horizon_bars")?;
            let benchmark_symbol: String = row.try_get("benchmark_symbol")?;
            let benchmark_close: Option<f64> = row.try_get("benchmark_close")?;
            let future = self
                .daily_technical_bars_after(&symbol, observed_at, i64::from(horizon_bars))
                .await?;
            let benchmark_future = if benchmark_symbol == "none" {
                Vec::new()
            } else {
                self.daily_technical_bars_after(
                    &benchmark_symbol,
                    observed_at,
                    i64::from(horizon_bars),
                )
                .await?
            };
            let Some(outcome) = technical_timing::evaluate_outcome(
                close,
                &future,
                usize::try_from(horizon_bars).unwrap_or(20),
                benchmark_close,
                &benchmark_future,
            ) else {
                continue;
            };

            sqlx::query(
                r#"UPDATE technical_timing_observation
                      SET forward_return_pct = $2,
                          max_drawdown_pct = $3,
                          benchmark_return_pct = $4,
                          benchmark_max_drawdown_pct = $5,
                          excess_return_pct = $6,
                          evaluated_at = now()
                    WHERE observation_id = $1"#,
            )
            .bind(observation_id)
            .bind(outcome.forward_return_pct)
            .bind(outcome.max_drawdown_pct)
            .bind(outcome.benchmark_return_pct)
            .bind(outcome.benchmark_max_drawdown_pct)
            .bind(outcome.excess_return_pct)
            .execute(&self.pool)
            .await
            .context("update technical timing outcome")?;
            scored += 1;
        }
        Ok(scored)
    }

    pub async fn daily_technical_bars_after(
        &self,
        symbol: &str,
        after: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<TechnicalBar>> {
        let rows = sqlx::query(
            r#"WITH daily AS (
                 SELECT (date_trunc('day', ts AT TIME ZONE 'UTC'))::date AS day,
                        (array_agg(ts ORDER BY ts DESC))[1] AS ts,
                        max(high::float8) AS high,
                        min(low::float8) AS low,
                        (array_agg(close::float8 ORDER BY ts DESC))[1] AS close,
                        sum(volume::float8) AS volume
                   FROM price_bar
                  WHERE symbol = $1
                    AND ts > $2
               GROUP BY 1
             )
             SELECT ts, close, high, low, volume
               FROM daily
              ORDER BY day ASC
              LIMIT $3"#,
        )
        .bind(symbol)
        .bind(after)
        .bind(limit.clamp(1, 1000))
        .fetch_all(&self.pool)
        .await
        .context("daily_technical_bars_after")?;
        rows.into_iter()
            .map(|r| {
                Ok(TechnicalBar {
                    ts: r.try_get("ts")?,
                    close: r.try_get("close")?,
                    high: r.try_get("high")?,
                    low: r.try_get("low")?,
                    volume: r.try_get("volume")?,
                })
            })
            .collect()
    }

    pub async fn intraday_technical_bars_for(
        &self,
        symbol: &str,
        native_interval: &str,
        lookback_days: i64,
        bucket_minutes: i64,
    ) -> Result<Vec<TechnicalBar>> {
        let rows = sqlx::query(
            r#"WITH bucketed AS (
                 SELECT to_timestamp(floor(extract(epoch FROM ts) / ($4::float8 * 60.0)) * ($4::float8 * 60.0)) AS bucket,
                        ts,
                        close::float8 AS close,
                        high::float8 AS high,
                        low::float8 AS low,
                        volume::float8 AS volume
                   FROM price_bar_intraday
                  WHERE symbol = $1
                    AND interval = $2
                    AND ts > now() - ($3 || ' days')::interval
             )
             SELECT bucket,
                    (array_agg(close ORDER BY ts DESC))[1] AS close,
                    max(high) AS high,
                    min(low) AS low,
                    sum(volume) AS volume
               FROM bucketed
              GROUP BY bucket
              ORDER BY bucket ASC"#,
        )
        .bind(symbol)
        .bind(native_interval)
        .bind(lookback_days.to_string())
        .bind(bucket_minutes)
        .fetch_all(&self.pool)
        .await
        .context("intraday_technical_bars_for")?;
        rows.into_iter()
            .map(|r| {
                Ok(TechnicalBar {
                    ts: r.try_get("bucket")?,
                    close: r.try_get("close")?,
                    high: r.try_get("high")?,
                    low: r.try_get("low")?,
                    volume: r.try_get("volume")?,
                })
            })
            .collect()
    }

    pub async fn open_positions_for_risk(&self) -> Result<Vec<crate::risk::Position>> {
        let rows = sqlx::query(
            r#"SELECT p.symbol,
                      COALESCE(t.cluster_id, '') AS cluster,
                      p.instrument,
                      COALESCE(p.delta_notional, 0)::float8 AS delta_notional,
                      COALESCE(p.premium_at_risk, 0)::float8 AS premium_at_risk
                 FROM position p
                 LEFT JOIN ticker t ON t.symbol = p.symbol
                WHERE p.closed_at IS NULL"#,
        )
        .fetch_all(&self.pool)
        .await
        .context("open_positions_for_risk")?;
        rows.into_iter()
            .map(|row| {
                Ok(crate::risk::Position {
                    symbol: row.try_get("symbol")?,
                    cluster: row.try_get("cluster")?,
                    instrument: row.try_get("instrument")?,
                    delta_notional: row.try_get::<f64, _>("delta_notional")?,
                    premium_at_risk: row.try_get::<f64, _>("premium_at_risk")?,
                })
            })
            .collect()
    }

    /// Sum of realized PnL across closed positions. Used by the risk overlay
    /// to compute realized drawdown (#26). Treats NULL as 0.
    pub async fn realized_pnl_total(&self) -> Result<f64> {
        let row = sqlx::query(
            r#"SELECT COALESCE(SUM(realized_pnl), 0)::float8 AS total
                 FROM position WHERE closed_at IS NOT NULL"#,
        )
        .fetch_one(&self.pool)
        .await
        .context("realized_pnl_total")?;
        Ok(row.try_get::<f64, _>("total")?)
    }

    /// Inserts an alert and returns its id.
    pub async fn insert_alert(
        &self,
        kind: AlertKind,
        symbol: Option<&str>,
        payload: &[u8],
    ) -> Result<i64> {
        let payload_str = std::str::from_utf8(payload).context("payload utf-8")?;
        let payload_json: serde_json::Value = serde_json::from_str(payload_str).unwrap_or_default();
        let inferred_symbol = symbol.map(str::to_string).or_else(|| {
            payload_json
                .get("symbol")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        });
        let thesis_id = payload_json
            .get("thesis_id")
            .and_then(|v| v.as_str())
            .and_then(|s| uuid::Uuid::parse_str(s).ok());
        let row = sqlx::query(
            r#"INSERT INTO alert (kind, symbol, thesis_id, payload)
               VALUES ($1, $2, $3, $4::jsonb)
            RETURNING id"#,
        )
        .bind(kind.as_str())
        .bind(inferred_symbol)
        .bind(thesis_id)
        .bind(payload_str)
        .fetch_one(&self.pool)
        .await
        .context("insert_alert")?;
        let id: i64 = row.try_get("id")?;
        Ok(id)
    }

    /// Marks an alert acknowledged. Idempotent — re-acking is a no-op.
    /// Returns true if a row was updated, false if no such alert existed.
    pub async fn acknowledge_alert(&self, id: i64) -> Result<bool> {
        let res = sqlx::query("UPDATE alert SET acknowledged = true WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .context("acknowledge_alert")?;
        Ok(res.rows_affected() > 0)
    }

    /// Returns the most recent alerts for the UI feed. When
    /// `only_unacked` is true (the default for the live-feed view), filters
    /// out alerts the user has already dismissed.
    pub async fn recent_alerts_filtered(
        &self,
        limit: i64,
        only_unacked: bool,
    ) -> Result<Vec<Alert>> {
        let rows = if only_unacked {
            sqlx::query(
                r#"SELECT id, thesis_id, symbol, kind, payload, acknowledged, created_at
                     FROM alert WHERE acknowledged = false
                 ORDER BY created_at DESC LIMIT $1"#,
            )
        } else {
            sqlx::query(
                r#"SELECT id, thesis_id, symbol, kind, payload, acknowledged, created_at
                     FROM alert ORDER BY created_at DESC LIMIT $1"#,
            )
        }
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .context("recent_alerts_filtered")?;

        rows.into_iter().map(decode_alert).collect()
    }

    /// Returns the most recent alerts for the UI feed.
    pub async fn recent_alerts(&self, limit: i64) -> Result<Vec<Alert>> {
        let rows = sqlx::query(
            r#"SELECT id, thesis_id, symbol, kind, payload, acknowledged, created_at
                 FROM alert ORDER BY created_at DESC LIMIT $1"#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .context("recent_alerts")?;

        rows.into_iter().map(decode_alert).collect()
    }

    pub async fn list_price_alert_rules(
        &self,
        symbol: Option<&str>,
        status: Option<&str>,
        limit: i64,
    ) -> Result<Vec<PriceAlertRule>> {
        let rows = sqlx::query(
            r#"SELECT id, symbol, thesis_id, origin, intent, direction,
                      target_price::float8 AS target_price, label, rationale,
                      semantic_key, status, source_ref, expires_at, created_at,
                      updated_at, triggered_at, disabled_at
                 FROM price_alert_rule
                WHERE ($1::text IS NULL OR symbol = upper($1))
                  AND ($2::text IS NULL OR status = $2)
             ORDER BY created_at DESC
                LIMIT $3"#,
        )
        .bind(symbol)
        .bind(status)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .context("list_price_alert_rules")?;
        rows.into_iter().map(decode_price_alert_rule).collect()
    }

    pub async fn ensure_builtin_automation_strategies(
        &self,
        definitions: &[BuiltinStrategyDefinition],
    ) -> Result<()> {
        for definition in definitions {
            sqlx::query(
                r#"INSERT INTO automation_strategy_definition
                     (strategy_id, strategy_version, family, display_name,
                      description, config_hash, config, status)
                   VALUES ($1, $2, $3, $4, $5, $6, $7::jsonb, $8)
                   ON CONFLICT (strategy_id, strategy_version) DO NOTHING"#,
            )
            .bind(definition.strategy_id)
            .bind(definition.strategy_version)
            .bind(definition.family)
            .bind(definition.display_name)
            .bind(definition.description)
            .bind(definition.config_hash())
            .bind(&definition.config)
            .bind(definition.status)
            .execute(&self.pool)
            .await
            .with_context(|| {
                format!(
                    "ensure automation strategy {} {}",
                    definition.strategy_id, definition.strategy_version
                )
            })?;
        }
        Ok(())
    }

    pub async fn automation_strategy_candidates(
        &self,
        limit: i64,
    ) -> Result<Vec<AutomationStrategyCandidate>> {
        let rows = sqlx::query(
            r#"SELECT p.permission_id,
                      p.symbol,
                      p.strategy_id,
                      p.strategy_version,
                      p.status AS permission_status,
                      p.instrument_scope,
                      p.environment_scope,
                      p.manual_freeze,
                      p.expires_at,
                      p.max_allocation_pct::float8 AS max_allocation_pct,
                      p.max_notional_usd::float8 AS max_notional_usd,
                      p.max_quantity::float8 AS max_quantity,
                      s.family,
                      s.display_name,
                      s.status AS strategy_status,
                      s.config_hash,
                      s.config,
                      latest.desired_position_id AS latest_desired_position_id,
                      latest.target_side AS latest_target_side,
                      latest.target_weight_pct::float8 AS latest_target_weight_pct,
                      latest.emitted_at AS latest_emitted_at
                 FROM automation_trade_permission p
                 JOIN automation_strategy_definition s
                   ON s.strategy_id = p.strategy_id
                  AND s.strategy_version = p.strategy_version
            LEFT JOIN LATERAL (
                    SELECT desired_position_id, target_side, target_weight_pct, emitted_at
                      FROM desired_strategy_position d
                     WHERE d.permission_id = p.permission_id
                  ORDER BY emitted_at DESC
                     LIMIT 1
                 ) latest ON TRUE
                WHERE s.status = 'shadow'
                  AND p.environment_scope = 'shadow'
                  AND p.status IN ('approved', 'pending', 'expired')
             ORDER BY p.updated_at ASC, p.symbol, p.strategy_id
                LIMIT $1"#,
        )
        .bind(limit.clamp(1, 500))
        .fetch_all(&self.pool)
        .await
        .context("automation_strategy_candidates")?;

        rows.into_iter()
            .map(|row| {
                let latest_target_side: Option<String> = row.try_get("latest_target_side")?;
                let latest_desired = match (
                    row.try_get::<Option<uuid::Uuid>, _>("latest_desired_position_id")?,
                    latest_target_side,
                ) {
                    (Some(desired_position_id), Some(side)) => Some(LatestDesiredPosition {
                        desired_position_id,
                        target_side: TargetSide::try_from(side.as_str())?,
                        target_weight_pct: row.try_get("latest_target_weight_pct")?,
                        emitted_at: row.try_get("latest_emitted_at")?,
                    }),
                    _ => None,
                };
                Ok(AutomationStrategyCandidate {
                    definition: StrategyDefinitionInput {
                        strategy_id: row.try_get("strategy_id")?,
                        strategy_version: row.try_get("strategy_version")?,
                        family: row.try_get("family")?,
                        display_name: row.try_get("display_name")?,
                        status: row.try_get("strategy_status")?,
                        config_hash: row.try_get("config_hash")?,
                        config: row.try_get("config")?,
                    },
                    permission: TradePermissionInput {
                        permission_id: row.try_get("permission_id")?,
                        symbol: row.try_get("symbol")?,
                        strategy_id: row.try_get("strategy_id")?,
                        strategy_version: row.try_get("strategy_version")?,
                        status: row.try_get("permission_status")?,
                        instrument_scope: row.try_get("instrument_scope")?,
                        environment_scope: row.try_get("environment_scope")?,
                        manual_freeze: row.try_get("manual_freeze")?,
                        expires_at: row.try_get("expires_at")?,
                        max_allocation_pct: row.try_get("max_allocation_pct")?,
                        max_notional_usd: row.try_get("max_notional_usd")?,
                        max_quantity: row.try_get("max_quantity")?,
                    },
                    latest_desired,
                })
            })
            .collect()
    }

    pub async fn automation_control_state(&self) -> Result<AutomationControlState> {
        let row = sqlx::query(
            r#"SELECT kill_switch_enabled, kill_switch_reason
                 FROM automation_control_state
                WHERE id = true"#,
        )
        .fetch_optional(&self.pool)
        .await
        .context("automation_control_state")?;
        let Some(row) = row else {
            return Ok(AutomationControlState::default());
        };
        Ok(AutomationControlState {
            kill_switch_enabled: row.try_get("kill_switch_enabled")?,
            kill_switch_reason: row.try_get("kill_switch_reason")?,
        })
    }

    pub async fn automation_allocation_limits(&self) -> Result<AllocationLimits> {
        let row = sqlx::query(
            r#"SELECT max_strategy_allocation_pct::float8 AS max_strategy,
                      max_symbol_allocation_pct::float8 AS max_symbol,
                      max_portfolio_allocation_pct::float8 AS max_portfolio
                 FROM automation_allocation_policy
                WHERE id = true"#,
        )
        .fetch_optional(&self.pool)
        .await
        .context("automation_allocation_limits")?;
        let Some(row) = row else {
            return Ok(AllocationLimits {
                max_strategy_allocation_pct: Some(0.10),
                max_symbol_allocation_pct: Some(0.15),
                max_portfolio_allocation_pct: Some(0.80),
            });
        };
        Ok(AllocationLimits {
            max_strategy_allocation_pct: row.try_get("max_strategy")?,
            max_symbol_allocation_pct: row.try_get("max_symbol")?,
            max_portfolio_allocation_pct: row.try_get("max_portfolio")?,
        })
    }

    pub async fn automation_sleeve_allocations(&self) -> Result<Vec<SleeveAllocation>> {
        let rows = sqlx::query(
            r#"SELECT sleeve_id,
                      symbol,
                      sleeve_kind,
                      status,
                      COALESCE(allocated_notional_usd, current_notional_usd, 0)::float8 AS allocated_notional_usd,
                      COALESCE(current_notional_usd, 0)::float8 AS current_notional_usd,
                      COALESCE(realized_pnl, 0)::float8 AS realized_pnl,
                      COALESCE(unrealized_pnl, 0)::float8 AS unrealized_pnl
                 FROM automation_strategy_sleeve
                WHERE closed_at IS NULL"#,
        )
        .fetch_all(&self.pool)
        .await
        .context("automation_sleeve_allocations")?;
        rows.into_iter()
            .map(|row| {
                Ok(SleeveAllocation {
                    sleeve_id: Some(row.try_get("sleeve_id")?),
                    symbol: row.try_get("symbol")?,
                    sleeve_kind: row.try_get("sleeve_kind")?,
                    status: row.try_get("status")?,
                    allocated_notional_usd: row.try_get("allocated_notional_usd")?,
                    current_notional_usd: row.try_get("current_notional_usd")?,
                    realized_pnl: row.try_get("realized_pnl")?,
                    unrealized_pnl: row.try_get("unrealized_pnl")?,
                })
            })
            .collect()
    }

    pub async fn open_strategy_sleeve_id(
        &self,
        permission_id: uuid::Uuid,
    ) -> Result<Option<uuid::Uuid>> {
        sqlx::query_scalar(
            r#"SELECT sleeve_id
                 FROM automation_strategy_sleeve
                WHERE permission_id = $1
                  AND sleeve_kind = 'strategy'
                  AND closed_at IS NULL
             ORDER BY updated_at DESC
                LIMIT 1"#,
        )
        .bind(permission_id)
        .fetch_optional(&self.pool)
        .await
        .context("open_strategy_sleeve_id")
    }

    pub async fn automation_sleeve_policy_state(
        &self,
        permission_id: uuid::Uuid,
    ) -> Result<SleevePolicyState> {
        let row = sqlx::query(
            r#"SELECT sleeve_id,
                      status,
                      current_side,
                      current_quantity::float8 AS current_quantity,
                      current_notional_usd::float8 AS current_notional_usd,
                      allocated_notional_usd::float8 AS allocated_notional_usd,
                      realized_pnl::float8 AS realized_pnl,
                      unrealized_pnl::float8 AS unrealized_pnl,
                      last_mark_price::float8 AS last_mark_price,
                      last_mark_at,
                      opened_at,
                      closed_at,
                      updated_at
                 FROM automation_strategy_sleeve
                WHERE permission_id = $1
             ORDER BY (closed_at IS NULL) DESC, updated_at DESC
                LIMIT 1"#,
        )
        .bind(permission_id)
        .fetch_optional(&self.pool)
        .await
        .context("automation_sleeve_policy_state")?;
        let Some(row) = row else {
            return Ok(SleevePolicyState {
                status: "missing".to_string(),
                current_side: TargetSide::Flat,
                allocated_notional_usd: None,
                snapshot: serde_json::json!({"status": "missing"}),
            });
        };
        let current_side_raw: String = row.try_get("current_side")?;
        let current_side =
            TargetSide::try_from(current_side_raw.as_str()).unwrap_or(TargetSide::Flat);
        Ok(SleevePolicyState {
            status: row.try_get("status")?,
            current_side,
            allocated_notional_usd: row.try_get("allocated_notional_usd")?,
            snapshot: serde_json::json!({
                "sleeve_id": row.try_get::<uuid::Uuid, _>("sleeve_id")?,
                "status": row.try_get::<String, _>("status")?,
                "current_side": current_side.as_str(),
                "current_quantity": row.try_get::<f64, _>("current_quantity")?,
                "current_notional_usd": row.try_get::<f64, _>("current_notional_usd")?,
                "allocated_notional_usd": row.try_get::<Option<f64>, _>("allocated_notional_usd")?,
                "realized_pnl": row.try_get::<f64, _>("realized_pnl")?,
                "unrealized_pnl": row.try_get::<f64, _>("unrealized_pnl")?,
                "last_mark_price": row.try_get::<Option<f64>, _>("last_mark_price")?,
                "last_mark_at": row.try_get::<Option<DateTime<Utc>>, _>("last_mark_at")?,
                "opened_at": row.try_get::<DateTime<Utc>, _>("opened_at")?,
                "closed_at": row.try_get::<Option<DateTime<Utc>>, _>("closed_at")?,
                "updated_at": row.try_get::<DateTime<Utc>, _>("updated_at")?,
            }),
        })
    }

    pub async fn automation_broker_policy_state(&self, symbol: &str) -> Result<BrokerPolicyState> {
        let row = sqlx::query(
            r#"SELECT COUNT(*)::int8 AS open_positions,
                      COUNT(*) FILTER (WHERE source = 'broker')::int8 AS broker_positions,
                      COALESCE(SUM(CASE WHEN COALESCE(side, 'long') = 'short' THEN -qty ELSE qty END), 0)::float8 AS net_quantity,
                      COALESCE(SUM(delta_notional), 0)::float8 AS delta_notional,
                      COALESCE(SUM(premium_at_risk), 0)::float8 AS premium_at_risk,
                      MAX(broker_last_sync_at) AS latest_sync_at
                 FROM position
                WHERE symbol = upper($1)
                  AND closed_at IS NULL"#,
        )
        .bind(symbol.trim())
        .fetch_one(&self.pool)
        .await
        .context("automation_broker_policy_state")?;
        let open_positions: i64 = row.try_get("open_positions")?;
        let broker_positions: i64 = row.try_get("broker_positions")?;
        let latest_sync_at: Option<DateTime<Utc>> = row.try_get("latest_sync_at")?;
        let status = if open_positions == 0 {
            "no_open_position"
        } else if broker_positions > 0 {
            "broker_seen"
        } else {
            "shadow_only"
        };
        Ok(BrokerPolicyState {
            status: status.to_string(),
            mismatch: false,
            latest_sync_at,
            snapshot: serde_json::json!({
                "status": status,
                "open_positions": open_positions,
                "broker_positions": broker_positions,
                "net_quantity": row.try_get::<f64, _>("net_quantity")?,
                "delta_notional": row.try_get::<f64, _>("delta_notional")?,
                "premium_at_risk": row.try_get::<f64, _>("premium_at_risk")?,
                "latest_sync_at": latest_sync_at,
            }),
        })
    }

    pub async fn ticker_cluster_id(&self, symbol: &str) -> Result<String> {
        let row = sqlx::query(
            r#"SELECT COALESCE(cluster_id, '') AS cluster_id
                 FROM ticker
                WHERE symbol = upper($1)"#,
        )
        .bind(symbol.trim())
        .fetch_optional(&self.pool)
        .await
        .context("ticker_cluster_id")?;
        Ok(row
            .and_then(|row| row.try_get::<String, _>("cluster_id").ok())
            .unwrap_or_default())
    }

    pub async fn insert_desired_strategy_position(
        &self,
        write: &DesiredPositionWrite,
    ) -> Result<DesiredPositionReceipt> {
        let mut tx = self.pool.begin().await.context("begin automation tx")?;
        let inserted_sleeve: Option<uuid::Uuid> = sqlx::query_scalar(
            r#"INSERT INTO automation_strategy_sleeve
                 (sleeve_kind, permission_id, symbol, strategy_id, strategy_version, status, source_ref)
               SELECT 'strategy', $1, $2, $3, $4, 'active',
                      jsonb_build_object('created_by', 'shadow_strategy_runner')
                WHERE NOT EXISTS (
                      SELECT 1
                        FROM automation_strategy_sleeve
                       WHERE permission_id = $1
                         AND closed_at IS NULL
                )
            RETURNING sleeve_id"#,
        )
        .bind(write.permission_id)
        .bind(&write.symbol)
        .bind(&write.strategy_id)
        .bind(&write.strategy_version)
        .fetch_optional(&mut *tx)
        .await
        .context("insert automation sleeve")?;

        let sleeve_id = match inserted_sleeve {
            Some(sleeve_id) => sleeve_id,
            None => sqlx::query_scalar(
                r#"SELECT sleeve_id
                         FROM automation_strategy_sleeve
                        WHERE permission_id = $1
                          AND closed_at IS NULL
                     ORDER BY updated_at DESC
                        LIMIT 1"#,
            )
            .bind(write.permission_id)
            .fetch_one(&mut *tx)
            .await
            .context("select automation sleeve")?,
        };

        let supersedes_desired_position_id: Option<uuid::Uuid> = sqlx::query_scalar(
            r#"SELECT desired_position_id
                 FROM desired_strategy_position
                WHERE permission_id = $1
             ORDER BY emitted_at DESC
                LIMIT 1"#,
        )
        .bind(write.permission_id)
        .fetch_optional(&mut *tx)
        .await
        .context("latest desired position")?;

        let reason_codes = serde_json::json!(write.reason_codes);
        let desired_position_id: uuid::Uuid = sqlx::query_scalar(
            r#"INSERT INTO desired_strategy_position
                 (permission_id, sleeve_id, symbol, thesis_id, strategy_id,
                  strategy_version, strategy_config_hash, target_side,
                  target_weight_pct, target_notional_usd, rationale, reason_codes, feature_snapshot,
                  signal_ref, supersedes_desired_position_id)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                       NULLIF($11, ''), $12::jsonb, $13::jsonb, $14::jsonb, $15)
            RETURNING desired_position_id"#,
        )
        .bind(write.permission_id)
        .bind(sleeve_id)
        .bind(&write.symbol)
        .bind(write.thesis_id)
        .bind(&write.strategy_id)
        .bind(&write.strategy_version)
        .bind(&write.strategy_config_hash)
        .bind(write.target_side.as_str())
        .bind(write.target_weight_pct)
        .bind(write.target_notional_usd)
        .bind(&write.rationale)
        .bind(&reason_codes)
        .bind(&write.feature_snapshot)
        .bind(&write.signal_ref)
        .bind(supersedes_desired_position_id)
        .fetch_one(&mut *tx)
        .await
        .context("insert desired strategy position")?;

        sqlx::query(
            r#"UPDATE automation_strategy_sleeve
                  SET allocated_notional_usd = $2,
                      updated_at = now()
                WHERE sleeve_id = $1"#,
        )
        .bind(sleeve_id)
        .bind(write.target_notional_usd)
        .execute(&mut *tx)
        .await
        .context("update automation sleeve allocation")?;

        sqlx::query(
            r#"INSERT INTO automation_sleeve_event
                 (sleeve_id, event_kind, source_ref)
               VALUES ($1, 'desired_position',
                       jsonb_build_object(
                         'desired_position_id', $2::uuid,
                         'target_side', $3::text,
                         'reason_codes', $4::jsonb
                       ))"#,
        )
        .bind(sleeve_id)
        .bind(desired_position_id)
        .bind(write.target_side.as_str())
        .bind(&reason_codes)
        .execute(&mut *tx)
        .await
        .context("insert automation sleeve event")?;

        let blocked_reasons = serde_json::json!(write.proof.blocked_reasons);
        let proof_id: uuid::Uuid = sqlx::query_scalar(
            r#"INSERT INTO automation_proof
                 (desired_position_id, permission_id, sleeve_id, symbol,
                  strategy_id, strategy_version, strategy_config_hash,
                  environment_scope, result, blocked_reasons, input_snapshot,
                  permission_snapshot, risk_result, data_freshness, session_state,
                  capital_allocation, broker_reconciliation)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10::jsonb,
                       $11::jsonb, $12::jsonb, $13::jsonb, $14::jsonb, $15::jsonb,
                       $16::jsonb, $17::jsonb)
            RETURNING proof_id"#,
        )
        .bind(desired_position_id)
        .bind(write.permission_id)
        .bind(sleeve_id)
        .bind(&write.symbol)
        .bind(&write.strategy_id)
        .bind(&write.strategy_version)
        .bind(&write.strategy_config_hash)
        .bind(&write.environment_scope)
        .bind(&write.proof.result)
        .bind(&blocked_reasons)
        .bind(&write.proof.input_snapshot)
        .bind(&write.proof.permission_snapshot)
        .bind(&write.proof.risk_result)
        .bind(&write.proof.data_freshness)
        .bind(&write.proof.session_state)
        .bind(&write.proof.capital_allocation)
        .bind(&write.proof.broker_reconciliation)
        .fetch_one(&mut *tx)
        .await
        .context("insert automation proof")?;

        let validation_snapshot =
            serde_json::to_value(&write.validation).context("serialize validation plan")?;
        let signal_key = format!(
            "{}:{}:{}:{}:{}",
            write.symbol,
            write.strategy_id,
            write.strategy_version,
            write.target_side.as_str(),
            write.validation.evaluation_due_at.to_rfc3339()
        );
        let observation_id: uuid::Uuid = sqlx::query_scalar(
            r#"INSERT INTO automation_strategy_signal_observation
                 (desired_position_id, proof_id, permission_id, sleeve_id, symbol,
                  strategy_id, strategy_version, strategy_config_hash, signal_key,
                  target_side, prior_target_side, churn_event, reason_codes,
                  feature_snapshot, validation_snapshot, evaluation_due_at)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9,
                       $10, $11, $12, $13::jsonb, $14::jsonb, $15::jsonb, $16)
            RETURNING observation_id"#,
        )
        .bind(desired_position_id)
        .bind(proof_id)
        .bind(write.permission_id)
        .bind(sleeve_id)
        .bind(&write.symbol)
        .bind(&write.strategy_id)
        .bind(&write.strategy_version)
        .bind(&write.strategy_config_hash)
        .bind(signal_key)
        .bind(write.target_side.as_str())
        .bind(write.prior_target_side.map(TargetSide::as_str))
        .bind(write.validation.churn_event)
        .bind(&reason_codes)
        .bind(&write.feature_snapshot)
        .bind(&validation_snapshot)
        .bind(write.validation.evaluation_due_at)
        .fetch_one(&mut *tx)
        .await
        .context("insert automation signal observation")?;

        tx.commit().await.context("commit automation tx")?;
        Ok(DesiredPositionReceipt {
            desired_position_id,
            proof_id,
            observation_id,
        })
    }

    pub async fn simulate_desired_reconciliation(
        &self,
        desired_position_id: uuid::Uuid,
    ) -> Result<SimulatedReconciliationReceipt> {
        let mut tx = self
            .pool
            .begin()
            .await
            .context("begin simulator reconciliation tx")?;
        let row = sqlx::query(
            r#"SELECT d.desired_position_id,
                      d.permission_id,
                      d.sleeve_id,
                      d.symbol,
                      d.thesis_id,
                      d.target_side,
                      d.target_notional_usd::float8 AS target_notional_usd,
                      p.environment_scope,
                      ap.proof_id,
                      sl.current_side,
                      sl.current_quantity::float8 AS current_quantity,
                      sl.current_notional_usd::float8 AS current_notional_usd
                 FROM desired_strategy_position d
                 JOIN automation_trade_permission p
                   ON p.permission_id = d.permission_id
                 JOIN automation_strategy_sleeve sl
                   ON sl.sleeve_id = d.sleeve_id
                 JOIN LATERAL (
                    SELECT proof_id
                      FROM automation_proof ap
                     WHERE ap.desired_position_id = d.desired_position_id
                  ORDER BY ap.evaluated_at DESC
                     LIMIT 1
                 ) ap ON TRUE
                WHERE d.desired_position_id = $1
                FOR UPDATE OF sl"#,
        )
        .bind(desired_position_id)
        .fetch_one(&mut *tx)
        .await
        .context("load desired position for simulator reconciliation")?;

        let symbol: String = row.try_get("symbol")?;
        let market_price = sqlx::query_scalar::<_, Option<f64>>(
            r#"SELECT close::float8
                 FROM price_bar
                WHERE symbol = $1
                  AND close IS NOT NULL
             ORDER BY ts DESC
                LIMIT 1"#,
        )
        .bind(&symbol)
        .fetch_optional(&mut *tx)
        .await
        .context("latest price for simulator reconciliation")?
        .flatten()
        .unwrap_or(0.0);

        let proof_id: uuid::Uuid = row.try_get("proof_id")?;
        let permission_id: uuid::Uuid = row.try_get("permission_id")?;
        let sleeve_id: uuid::Uuid = row.try_get("sleeve_id")?;
        let thesis_id: Option<uuid::Uuid> = row.try_get("thesis_id")?;
        let current_side_raw: String = row.try_get("current_side")?;
        let current_side =
            TargetSide::try_from(current_side_raw.as_str()).unwrap_or(TargetSide::Flat);
        let current_quantity: f64 = row.try_get("current_quantity")?;
        let current_notional_usd: f64 = row.try_get("current_notional_usd")?;
        let current_avg_price = if current_quantity > 0.000_001 {
            current_notional_usd / current_quantity
        } else {
            0.0
        };
        let target_side_raw: String = row.try_get("target_side")?;
        let target_side = TargetSide::try_from(target_side_raw.as_str())?;
        let input = ReconciliationInput {
            desired_position_id,
            proof_id,
            sleeve_id,
            symbol: symbol.clone(),
            environment_scope: row.try_get("environment_scope")?,
            target_side,
            target_notional_usd: row
                .try_get::<Option<f64>, _>("target_notional_usd")?
                .unwrap_or(0.0),
            market_price,
            current_position: SimulatedPosition {
                side: current_side,
                quantity: current_quantity,
                avg_price: current_avg_price,
            },
            now: Utc::now(),
            config: SimulationConfig::default(),
        };
        let outcome = reconcile_simulated(&input);
        let reconciliation_status = if input.environment_scope == "shadow"
            || outcome.status == "noop"
            || outcome.status == "blocked"
            || outcome.status == "incident"
        {
            outcome.status.clone()
        } else {
            "needs_order".to_string()
        };
        let blocked_reasons = serde_json::json!(outcome.blocked_reasons);
        let source_ref = serde_json::json!({
            "source": "digital_broker_simulator",
            "desired_position_id": desired_position_id,
            "proof_id": proof_id,
            "scope": input.environment_scope,
            "shadow_fill_applied": input.environment_scope == "shadow",
        });
        let reconciliation_id: Option<uuid::Uuid> = sqlx::query_scalar(
            r#"INSERT INTO automation_execution_reconciliation
                 (desired_position_id, proof_id, sleeve_id, symbol, environment_scope,
                  status, idempotency_key, target_snapshot, broker_snapshot,
                  delta_snapshot, order_plan, blocked_reasons, source_ref)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8::jsonb, $9::jsonb,
                       $10::jsonb, $11::jsonb, $12::jsonb, $13::jsonb)
            ON CONFLICT (idempotency_key) DO NOTHING
            RETURNING reconciliation_id"#,
        )
        .bind(desired_position_id)
        .bind(proof_id)
        .bind(sleeve_id)
        .bind(&symbol)
        .bind(&input.environment_scope)
        .bind(&reconciliation_status)
        .bind(&outcome.idempotency_key)
        .bind(&outcome.target_snapshot)
        .bind(&outcome.broker_snapshot)
        .bind(&outcome.delta_snapshot)
        .bind(&outcome.order_plan)
        .bind(&blocked_reasons)
        .bind(&source_ref)
        .fetch_optional(&mut *tx)
        .await
        .context("insert simulator reconciliation")?;

        let Some(reconciliation_id) = reconciliation_id else {
            let existing = sqlx::query(
                r#"SELECT reconciliation_id, status
                     FROM automation_execution_reconciliation
                    WHERE idempotency_key = $1"#,
            )
            .bind(&outcome.idempotency_key)
            .fetch_one(&mut *tx)
            .await
            .context("select duplicate simulator reconciliation")?;
            tx.commit()
                .await
                .context("commit duplicate simulator reconciliation")?;
            return Ok(SimulatedReconciliationReceipt {
                reconciliation_id: existing.try_get("reconciliation_id")?,
                status: existing.try_get("status")?,
                duplicate: true,
                fills: 0,
                incident: false,
            });
        };

        if input.environment_scope != "shadow" && reconciliation_status != "incident" {
            tx.commit()
                .await
                .context("commit paper/live needs-order reconciliation")?;
            return Ok(SimulatedReconciliationReceipt {
                reconciliation_id,
                status: reconciliation_status,
                duplicate: false,
                fills: 0,
                incident: false,
            });
        }

        if let Some(incident) = outcome.incident.as_ref() {
            sqlx::query(
                r#"INSERT INTO automation_incident
                     (severity, kind, symbol, permission_id, sleeve_id,
                      desired_position_id, proof_id, reconciliation_id,
                      title, detail, source_ref)
                   VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11::jsonb)"#,
            )
            .bind(&incident.severity)
            .bind(&incident.kind)
            .bind(&symbol)
            .bind(permission_id)
            .bind(sleeve_id)
            .bind(desired_position_id)
            .bind(proof_id)
            .bind(reconciliation_id)
            .bind(&incident.title)
            .bind(&incident.detail)
            .bind(&source_ref)
            .execute(&mut *tx)
            .await
            .context("insert simulator incident")?;
        }

        if input.environment_scope != "shadow" {
            tx.commit()
                .await
                .context("commit paper/live incident reconciliation")?;
            return Ok(SimulatedReconciliationReceipt {
                reconciliation_id,
                status: reconciliation_status,
                duplicate: false,
                fills: 0,
                incident: true,
            });
        }

        let mut inserted_fills = 0_usize;
        for (idx, fill) in outcome.fills.iter().enumerate() {
            let position_row = sqlx::query(
                r#"SELECT position_id,
                          qty::float8 AS qty,
                          avg_price::float8 AS avg_price
                     FROM position
                    WHERE source = 'broker'
                      AND broker = 'simulator'
                      AND broker_account = 'digital'
                      AND symbol = $1
                      AND side = $2
                      AND instrument = 'equity'
                      AND closed_at IS NULL
                 ORDER BY opened_at DESC
                    LIMIT 1
                    FOR UPDATE"#,
            )
            .bind(&symbol)
            .bind(fill.side.as_str())
            .fetch_optional(&mut *tx)
            .await
            .context("select simulator broker position")?;

            let is_entry = matches!(fill.leg_kind.as_str(), "enter" | "increase");
            let position_id = if is_entry {
                if let Some(position_row) = position_row {
                    let position_id: uuid::Uuid = position_row.try_get("position_id")?;
                    let old_qty: f64 = position_row.try_get("qty")?;
                    let old_avg: f64 = position_row.try_get("avg_price")?;
                    let new_qty = old_qty + fill.quantity;
                    let new_avg = if new_qty > 0.000_001 {
                        ((old_qty * old_avg) + (fill.quantity * fill.price)) / new_qty
                    } else {
                        0.0
                    };
                    sqlx::query(
                        r#"UPDATE position
                              SET qty = $2,
                                  avg_price = $3,
                                  delta_notional = $4,
                                  broker_last_sync_at = now()
                            WHERE position_id = $1"#,
                    )
                    .bind(position_id)
                    .bind(new_qty)
                    .bind(new_avg)
                    .bind(new_qty * fill.price)
                    .execute(&mut *tx)
                    .await
                    .context("update simulator broker position")?;
                    position_id
                } else {
                    sqlx::query_scalar(
                        r#"INSERT INTO position
                             (thesis_id, symbol, side, instrument, qty, avg_price,
                              delta_notional, premium_at_risk, opened_at, source,
                              broker, broker_account, broker_contract, broker_last_sync_at)
                           VALUES ($1, $2, $3, 'equity', $4, $5, $6, 0,
                                   now(), 'broker', 'simulator', 'digital',
                                   $7::jsonb, now())
                        RETURNING position_id"#,
                    )
                    .bind(thesis_id)
                    .bind(&symbol)
                    .bind(fill.side.as_str())
                    .bind(fill.quantity)
                    .bind(fill.price)
                    .bind(fill.notional_usd)
                    .bind(serde_json::json!({
                        "source": "digital_broker_simulator",
                        "reconciliation_id": reconciliation_id,
                    }))
                    .fetch_one(&mut *tx)
                    .await
                    .context("insert simulator broker position")?
                }
            } else if let Some(position_row) = position_row {
                let position_id: uuid::Uuid = position_row.try_get("position_id")?;
                let old_qty: f64 = position_row.try_get("qty")?;
                let remaining_qty = (old_qty - fill.quantity).max(0.0);
                if remaining_qty > 0.000_001 {
                    sqlx::query(
                        r#"UPDATE position
                              SET qty = $2,
                                  delta_notional = $3,
                                  realized_pnl = COALESCE(realized_pnl, 0) + $4,
                                  broker_last_sync_at = now()
                            WHERE position_id = $1"#,
                    )
                    .bind(position_id)
                    .bind(remaining_qty)
                    .bind(remaining_qty * fill.price)
                    .bind(fill.realized_pnl_delta)
                    .execute(&mut *tx)
                    .await
                    .context("reduce simulator broker position")?;
                } else {
                    sqlx::query(
                        r#"UPDATE position
                              SET qty = 0,
                                  delta_notional = 0,
                                  closed_at = now(),
                                  realized_pnl = COALESCE(realized_pnl, 0) + $2,
                                  broker_last_sync_at = now()
                            WHERE position_id = $1"#,
                    )
                    .bind(position_id)
                    .bind(fill.realized_pnl_delta)
                    .execute(&mut *tx)
                    .await
                    .context("close simulator broker position")?;
                }
                position_id
            } else {
                sqlx::query(
                    r#"INSERT INTO automation_incident
                         (severity, kind, symbol, permission_id, sleeve_id,
                          desired_position_id, proof_id, reconciliation_id,
                          title, detail, source_ref)
                       VALUES ('critical', 'simulated_broker_position_missing',
                               $1, $2, $3, $4, $5, $6,
                               'Simulated broker position missing',
                               $7, $8::jsonb)"#,
                )
                .bind(&symbol)
                .bind(permission_id)
                .bind(sleeve_id)
                .bind(desired_position_id)
                .bind(proof_id)
                .bind(reconciliation_id)
                .bind(format!(
                    "No open simulator {} position existed for {} {} fill.",
                    fill.side.as_str(),
                    symbol,
                    fill.leg_kind
                ))
                .bind(&source_ref)
                .execute(&mut *tx)
                .await
                .context("insert missing simulator broker position incident")?;
                continue;
            };

            let broker_execution_id = format!("{reconciliation_id}:{idx}");
            let fill_id: Option<uuid::Uuid> = sqlx::query_scalar(
                r#"INSERT INTO position_fill
                     (position_id, thesis_id, symbol, side, instrument, qty,
                      price, fees, filled_at, source, notes, raw,
                      broker, broker_account, broker_execution_id)
                   VALUES ($1, $2, $3, $4, 'equity', $5, $6, 0, now(),
                           'broker', 'automation simulator fill', $7::jsonb,
                           'simulator', 'digital', $8)
                ON CONFLICT DO NOTHING
                RETURNING fill_id"#,
            )
            .bind(position_id)
            .bind(thesis_id)
            .bind(&symbol)
            .bind(fill.side.as_str())
            .bind(fill.quantity)
            .bind(fill.price)
            .bind(serde_json::json!({
                "source": "digital_broker_simulator",
                "reconciliation_id": reconciliation_id,
                "desired_position_id": desired_position_id,
                "proof_id": proof_id,
                "leg_kind": fill.leg_kind,
                "notional_usd": fill.notional_usd,
                "realized_pnl_delta": fill.realized_pnl_delta,
            }))
            .bind(&broker_execution_id)
            .fetch_optional(&mut *tx)
            .await
            .context("insert simulator position fill")?;

            if let Some(fill_id) = fill_id {
                inserted_fills += 1;
                sqlx::query(
                    r#"INSERT INTO automation_sleeve_fill_attribution
                         (sleeve_id, position_fill_id, position_id, symbol, side,
                          quantity, notional_usd, realized_pnl_delta, source_ref)
                       VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9::jsonb)"#,
                )
                .bind(sleeve_id)
                .bind(fill_id)
                .bind(position_id)
                .bind(&symbol)
                .bind(fill.side.as_str())
                .bind(fill.quantity)
                .bind(fill.notional_usd)
                .bind(fill.realized_pnl_delta)
                .bind(serde_json::json!({
                    "source": "digital_broker_simulator",
                    "reconciliation_id": reconciliation_id,
                    "broker_execution_id": broker_execution_id,
                    "leg_kind": fill.leg_kind,
                }))
                .execute(&mut *tx)
                .await
                .context("insert simulator sleeve fill attribution")?;
            }
        }

        let final_notional = outcome.final_position.quantity * market_price;
        sqlx::query(
            r#"UPDATE automation_strategy_sleeve
                  SET current_side = $2,
                      current_quantity = $3,
                      current_notional_usd = $4,
                      realized_pnl = realized_pnl + $5,
                      unrealized_pnl = $6,
                      last_mark_price = NULLIF($7, 0),
                      last_mark_at = now(),
                      updated_at = now()
                WHERE sleeve_id = $1"#,
        )
        .bind(sleeve_id)
        .bind(outcome.final_position.side.as_str())
        .bind(outcome.final_position.quantity)
        .bind(final_notional)
        .bind(outcome.realized_pnl_delta)
        .bind(outcome.unrealized_pnl)
        .bind(market_price)
        .execute(&mut *tx)
        .await
        .context("update simulator sleeve state")?;

        sqlx::query(
            r#"INSERT INTO automation_sleeve_event
                 (sleeve_id, event_kind, quantity_delta, notional_delta,
                  realized_pnl_delta, source_ref)
               VALUES ($1, 'reconciled', $2, $3, $4, $5::jsonb)"#,
        )
        .bind(sleeve_id)
        .bind(outcome.final_position.quantity - current_quantity)
        .bind(final_notional - current_notional_usd)
        .bind(outcome.realized_pnl_delta)
        .bind(serde_json::json!({
            "source": "digital_broker_simulator",
            "reconciliation_id": reconciliation_id,
            "status": outcome.status,
            "fills": inserted_fills,
        }))
        .execute(&mut *tx)
        .await
        .context("insert simulator sleeve event")?;

        tx.commit()
            .await
            .context("commit simulator reconciliation tx")?;
        Ok(SimulatedReconciliationReceipt {
            reconciliation_id,
            status: outcome.status,
            duplicate: false,
            fills: inserted_fills,
            incident: outcome.incident.is_some(),
        })
    }

    pub async fn insert_blocked_automation_proof(
        &self,
        write: &BlockedProofWrite,
    ) -> Result<uuid::Uuid> {
        let sleeve_id: Option<uuid::Uuid> = sqlx::query_scalar(
            r#"SELECT sleeve_id
                 FROM automation_strategy_sleeve
                WHERE permission_id = $1
                  AND closed_at IS NULL
             ORDER BY updated_at DESC
                LIMIT 1"#,
        )
        .bind(write.permission_id)
        .fetch_optional(&self.pool)
        .await
        .context("select automation sleeve for blocked proof")?;

        let blocked_reasons = serde_json::json!(write.proof.blocked_reasons);
        let proof_id = sqlx::query_scalar(
            r#"INSERT INTO automation_proof
                 (desired_position_id, permission_id, sleeve_id, symbol,
                  strategy_id, strategy_version, strategy_config_hash,
                  environment_scope, result, blocked_reasons, input_snapshot,
                  permission_snapshot, risk_result, data_freshness, session_state,
                  capital_allocation, broker_reconciliation)
               VALUES (NULL, $1, $2, $3, $4, $5, $6, $7, $8, $9::jsonb,
                       $10::jsonb, $11::jsonb, $12::jsonb, $13::jsonb,
                       $14::jsonb, $15::jsonb, $16::jsonb)
            RETURNING proof_id"#,
        )
        .bind(write.permission_id)
        .bind(sleeve_id)
        .bind(&write.symbol)
        .bind(&write.strategy_id)
        .bind(&write.strategy_version)
        .bind(&write.strategy_config_hash)
        .bind(&write.environment_scope)
        .bind(&write.proof.result)
        .bind(&blocked_reasons)
        .bind(&write.proof.input_snapshot)
        .bind(&write.proof.permission_snapshot)
        .bind(&write.proof.risk_result)
        .bind(&write.proof.data_freshness)
        .bind(&write.proof.session_state)
        .bind(&write.proof.capital_allocation)
        .bind(&write.proof.broker_reconciliation)
        .fetch_one(&self.pool)
        .await
        .context("insert blocked automation proof")?;
        Ok(proof_id)
    }

    pub async fn evaluate_automation_readiness(&self, lookback_days: i64) -> Result<usize> {
        let lookback_days = lookback_days.clamp(1, 3660);
        let rows = sqlx::query(
            r#"SELECT strategy_id, strategy_version, status
                 FROM automation_strategy_definition
                WHERE status <> 'retired'
             ORDER BY strategy_id, strategy_version"#,
        )
        .fetch_all(&self.pool)
        .await
        .context("select automation strategies for readiness")?;

        let thresholds = ReadinessThresholds::default();
        let now = Utc::now();
        let mut evaluated = 0_usize;
        for row in rows {
            let strategy_id: String = row.try_get("strategy_id")?;
            let strategy_version: String = row.try_get("strategy_version")?;
            let status: String = row.try_get("status")?;
            let current_stage = AutomationStage::try_from(status.as_str())
                .with_context(|| format!("decode strategy stage {status}"))?;
            let target_stage = match current_stage {
                AutomationStage::Draft => Some(AutomationStage::Shadow),
                AutomationStage::Shadow => Some(AutomationStage::Paper),
                AutomationStage::Paper => Some(AutomationStage::CanaryLive),
                AutomationStage::CanaryLive => Some(AutomationStage::ExpandedLive),
                AutomationStage::ExpandedLive
                | AutomationStage::Frozen
                | AutomationStage::Retired => None,
            };
            let approval = match target_stage {
                Some(target) => {
                    self.automation_promotion_approval(
                        &strategy_id,
                        &strategy_version,
                        current_stage,
                        target,
                    )
                    .await?
                }
                None => None,
            };
            let metrics = self
                .automation_readiness_metrics(&strategy_id, &strategy_version, lookback_days)
                .await?;
            let decision = evaluate_readiness(&ReadinessInput {
                current_stage,
                metrics: metrics.clone(),
                approval: approval.as_ref().map(|row| row.approval.clone()),
                thresholds: thresholds.clone(),
                now,
            });
            let evaluation_id = self
                .insert_automation_readiness_evaluation(
                    &strategy_id,
                    &strategy_version,
                    lookback_days,
                    &metrics,
                    &thresholds,
                    &decision,
                    approval.as_ref().map(|row| row.approval_id),
                )
                .await?;
            self.insert_automation_strategy_lifecycle_event(
                &strategy_id,
                &strategy_version,
                "readiness_evaluated",
                Some(current_stage.as_str()),
                decision.target_stage.map(AutomationStage::as_str),
                Some(evaluation_id),
                approval.as_ref().map(|row| row.approval_id),
                "system",
                None,
                serde_json::json!({"lookback_days": lookback_days}),
            )
            .await?;
            if decision.status.as_str() == "ready" {
                if let (Some(target), Some(approval)) = (decision.target_stage, approval.as_ref()) {
                    self.promote_automation_strategy_after_readiness(
                        &strategy_id,
                        &strategy_version,
                        current_stage,
                        target,
                        evaluation_id,
                        approval.approval_id,
                    )
                    .await?;
                }
            }
            if decision.freeze_live_permissions {
                self.freeze_live_automation_permissions(
                    &strategy_id,
                    &strategy_version,
                    "readiness gate blocked live-capable strategy",
                    evaluation_id,
                )
                .await?;
            }
            evaluated += 1;
        }
        Ok(evaluated)
    }

    async fn automation_readiness_metrics(
        &self,
        strategy_id: &str,
        strategy_version: &str,
        lookback_days: i64,
    ) -> Result<ReadinessMetrics> {
        let observation = sqlx::query(
            r#"SELECT COUNT(*)::int8 AS observations_total,
                      COUNT(*) FILTER (WHERE evaluated_at IS NOT NULL)::int8 AS outcomes_scored,
                      COUNT(*) FILTER (
                        WHERE evaluated_at IS NOT NULL
                          AND target_side <> 'flat'
                      )::int8 AS directional_outcomes_scored,
                      AVG(CASE
                            WHEN evaluated_at IS NULL OR target_side = 'flat' THEN NULL
                            WHEN target_side = 'long' AND forward_return_pct > 0 THEN 1.0
                            WHEN target_side = 'short' AND forward_return_pct < 0 THEN 1.0
                            ELSE 0.0
                          END)::float8 AS signal_quality_rate,
                      AVG(forward_return_pct::float8)
                        FILTER (WHERE evaluated_at IS NOT NULL AND target_side <> 'flat')
                        AS mean_forward_return_pct,
                      AVG(max_drawdown_pct::float8)
                        FILTER (WHERE evaluated_at IS NOT NULL AND target_side <> 'flat')
                        AS mean_max_drawdown_pct,
                      AVG(CASE WHEN churn_event THEN 1.0 ELSE 0.0 END)::float8 AS churn_rate,
                      AVG(forward_return_pct::float8)
                        FILTER (WHERE evaluated_at IS NOT NULL AND target_side <> 'flat')
                        AS baseline_excess_return_pct
                 FROM automation_strategy_signal_observation
                WHERE strategy_id = $1
                  AND strategy_version = $2
                  AND created_at > now() - ($3 || ' days')::interval"#,
        )
        .bind(strategy_id)
        .bind(strategy_version)
        .bind(lookback_days.to_string())
        .fetch_one(&self.pool)
        .await
        .context("automation readiness observation metrics")?;

        let proof = sqlx::query(
            r#"SELECT COUNT(*)::int8 AS proof_total,
                      AVG(CASE WHEN result IN ('passed', 'warning') THEN 1.0 ELSE 0.0 END)::float8
                        AS proof_pass_rate
                 FROM automation_proof
                WHERE strategy_id = $1
                  AND strategy_version = $2
                  AND evaluated_at > now() - ($3 || ' days')::interval"#,
        )
        .bind(strategy_id)
        .bind(strategy_version)
        .bind(lookback_days.to_string())
        .fetch_one(&self.pool)
        .await
        .context("automation readiness proof metrics")?;

        let incident = sqlx::query(
            r#"SELECT COUNT(*)::int8 AS incident_total,
                      COUNT(*) FILTER (
                        WHERE i.status <> 'resolved'
                          AND i.severity = 'critical'
                      )::int8 AS open_critical_incidents
                 FROM automation_incident i
                 JOIN automation_trade_permission p
                   ON p.permission_id = i.permission_id
                WHERE p.strategy_id = $1
                  AND p.strategy_version = $2
                  AND i.created_at > now() - ($3 || ' days')::interval"#,
        )
        .bind(strategy_id)
        .bind(strategy_version)
        .bind(lookback_days.to_string())
        .fetch_one(&self.pool)
        .await
        .context("automation readiness incident metrics")?;

        let paper = sqlx::query(
            r#"SELECT COUNT(*)::int8 AS paper_orders_total,
                      CASE WHEN COUNT(*) = 0 THEN NULL
                           ELSE (
                             COUNT(*) FILTER (WHERE bo.status IN ('filled', 'partially_filled'))::float8
                             / COUNT(*)::float8
                           )
                      END AS paper_fill_quality_rate,
                      AVG(ABS((e.fill_price::float8 - COALESCE(bo.limit_price::float8, e.fill_price::float8))
                              / NULLIF(e.fill_price::float8, 0)) * 10000.0)
                        FILTER (WHERE e.fill_price IS NOT NULL)
                        AS mean_slippage_bps
                 FROM automation_broker_order bo
                 JOIN desired_strategy_position d
                   ON d.desired_position_id = bo.desired_position_id
            LEFT JOIN LATERAL (
                    SELECT fill_price
                      FROM automation_broker_order_event e
                     WHERE e.order_id = bo.order_id
                       AND e.fill_price IS NOT NULL
                  ORDER BY e.created_at DESC
                     LIMIT 1
                 ) e ON TRUE
                WHERE d.strategy_id = $1
                  AND d.strategy_version = $2
                  AND bo.created_at > now() - ($3 || ' days')::interval"#,
        )
        .bind(strategy_id)
        .bind(strategy_version)
        .bind(lookback_days.to_string())
        .fetch_one(&self.pool)
        .await
        .context("automation readiness paper metrics")?;

        let proof_total: i64 = proof.try_get("proof_total")?;
        let incident_total: i64 = incident.try_get("incident_total")?;
        let incident_rate = (proof_total > 0).then_some(incident_total as f64 / proof_total as f64);

        Ok(ReadinessMetrics {
            observations_total: observation.try_get("observations_total")?,
            outcomes_scored: observation.try_get("outcomes_scored")?,
            directional_outcomes_scored: observation.try_get("directional_outcomes_scored")?,
            signal_quality_rate: observation.try_get("signal_quality_rate").ok(),
            mean_forward_return_pct: observation.try_get("mean_forward_return_pct").ok(),
            mean_max_drawdown_pct: observation.try_get("mean_max_drawdown_pct").ok(),
            churn_rate: observation.try_get("churn_rate").ok(),
            proof_pass_rate: proof.try_get("proof_pass_rate").ok(),
            incident_rate,
            open_critical_incidents: incident.try_get("open_critical_incidents")?,
            paper_orders_total: paper.try_get("paper_orders_total")?,
            paper_fill_quality_rate: paper.try_get("paper_fill_quality_rate").ok(),
            mean_slippage_bps: paper.try_get("mean_slippage_bps").ok(),
            baseline_excess_return_pct: observation.try_get("baseline_excess_return_pct").ok(),
        })
    }

    async fn automation_promotion_approval(
        &self,
        strategy_id: &str,
        strategy_version: &str,
        from_stage: AutomationStage,
        to_stage: AutomationStage,
    ) -> Result<Option<PromotionApprovalRow>> {
        let row = sqlx::query(
            r#"SELECT approval_id,
                      from_stage,
                      to_stage,
                      status,
                      approved_at,
                      expires_at
                 FROM automation_strategy_promotion_approval
                WHERE strategy_id = $1
                  AND strategy_version = $2
                  AND from_stage = $3
                  AND to_stage = $4
                  AND status = 'approved'
             ORDER BY approved_at DESC
                LIMIT 1"#,
        )
        .bind(strategy_id)
        .bind(strategy_version)
        .bind(from_stage.as_str())
        .bind(to_stage.as_str())
        .fetch_optional(&self.pool)
        .await
        .context("automation promotion approval")?;
        row.map(|row| {
            let from_stage: String = row.try_get("from_stage")?;
            let to_stage: String = row.try_get("to_stage")?;
            Ok(PromotionApprovalRow {
                approval_id: row.try_get("approval_id")?,
                approval: PromotionApproval {
                    from_stage: AutomationStage::try_from(from_stage.as_str())?,
                    to_stage: AutomationStage::try_from(to_stage.as_str())?,
                    status: row.try_get("status")?,
                    approved_at: row.try_get("approved_at")?,
                    expires_at: row.try_get("expires_at")?,
                },
            })
        })
        .transpose()
    }

    async fn insert_automation_readiness_evaluation(
        &self,
        strategy_id: &str,
        strategy_version: &str,
        lookback_days: i64,
        metrics: &ReadinessMetrics,
        thresholds: &ReadinessThresholds,
        decision: &crate::automation::ReadinessDecision,
        approval_id: Option<uuid::Uuid>,
    ) -> Result<uuid::Uuid> {
        let evaluation_id = sqlx::query_scalar(
            r#"INSERT INTO automation_strategy_readiness_evaluation
                 (strategy_id, strategy_version, lifecycle_stage, target_stage,
                  status, readiness_score, approval_id, approval_required,
                  approval_valid, freeze_live_permissions, metrics, blockers,
                  warnings, thresholds, lookback_days, source_ref)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                       $11::jsonb, $12::jsonb, $13::jsonb, $14::jsonb, $15,
                       $16::jsonb)
            RETURNING evaluation_id"#,
        )
        .bind(strategy_id)
        .bind(strategy_version)
        .bind(decision.current_stage.as_str())
        .bind(decision.target_stage.map(AutomationStage::as_str))
        .bind(decision.status.as_str())
        .bind(decision.readiness_score)
        .bind(approval_id)
        .bind(decision.approval_required)
        .bind(decision.approval_valid)
        .bind(decision.freeze_live_permissions)
        .bind(serde_json::to_value(metrics)?)
        .bind(serde_json::json!(decision.blockers))
        .bind(serde_json::json!(decision.warnings))
        .bind(serde_json::to_value(thresholds)?)
        .bind(i32::try_from(lookback_days).unwrap_or(90))
        .bind(serde_json::json!({"source": "automation_readiness_evaluator"}))
        .fetch_one(&self.pool)
        .await
        .context("insert automation readiness evaluation")?;
        Ok(evaluation_id)
    }

    async fn promote_automation_strategy_after_readiness(
        &self,
        strategy_id: &str,
        strategy_version: &str,
        from_stage: AutomationStage,
        to_stage: AutomationStage,
        evaluation_id: uuid::Uuid,
        approval_id: uuid::Uuid,
    ) -> Result<()> {
        let updated = sqlx::query(
            r#"UPDATE automation_strategy_definition
                  SET status = $3
                WHERE strategy_id = $1
                  AND strategy_version = $2
                  AND status = $4"#,
        )
        .bind(strategy_id)
        .bind(strategy_version)
        .bind(to_stage.as_str())
        .bind(from_stage.as_str())
        .execute(&self.pool)
        .await
        .context("promote automation strategy")?
        .rows_affected();
        if updated == 0 {
            return Ok(());
        }
        sqlx::query(
            r#"UPDATE automation_strategy_promotion_approval
                  SET status = 'used',
                      updated_at = now()
                WHERE approval_id = $1"#,
        )
        .bind(approval_id)
        .execute(&self.pool)
        .await
        .context("mark automation promotion approval used")?;
        self.insert_automation_strategy_lifecycle_event(
            strategy_id,
            strategy_version,
            "promoted",
            Some(from_stage.as_str()),
            Some(to_stage.as_str()),
            Some(evaluation_id),
            Some(approval_id),
            "system",
            Some("readiness gate passed with valid operator approval"),
            serde_json::json!({}),
        )
        .await
    }

    async fn freeze_live_automation_permissions(
        &self,
        strategy_id: &str,
        strategy_version: &str,
        reason: &str,
        evaluation_id: uuid::Uuid,
    ) -> Result<()> {
        let rows = sqlx::query(
            r#"UPDATE automation_trade_permission
                  SET manual_freeze = true,
                      freeze_reason = $3,
                      updated_at = now()
                WHERE strategy_id = $1
                  AND strategy_version = $2
                  AND status = 'approved'
                  AND environment_scope IN ('canary_live', 'expanded_live')
                  AND manual_freeze = false
            RETURNING permission_id"#,
        )
        .bind(strategy_id)
        .bind(strategy_version)
        .bind(reason)
        .fetch_all(&self.pool)
        .await
        .context("freeze live automation permissions")?;
        for row in rows {
            let permission_id: uuid::Uuid = row.try_get("permission_id")?;
            sqlx::query(
                r#"INSERT INTO automation_permission_event
                     (permission_id, event_kind, manual_freeze, actor, reason, source_ref)
                   VALUES ($1, 'freeze_set', true, 'system', $2, $3::jsonb)"#,
            )
            .bind(permission_id)
            .bind(reason)
            .bind(serde_json::json!({
                "source": "automation_readiness_evaluator",
                "evaluation_id": evaluation_id,
            }))
            .execute(&self.pool)
            .await
            .context("insert readiness freeze permission event")?;
        }
        Ok(())
    }

    async fn insert_automation_strategy_lifecycle_event(
        &self,
        strategy_id: &str,
        strategy_version: &str,
        event_kind: &str,
        from_status: Option<&str>,
        to_status: Option<&str>,
        evaluation_id: Option<uuid::Uuid>,
        approval_id: Option<uuid::Uuid>,
        actor: &str,
        reason: Option<&str>,
        source_ref: serde_json::Value,
    ) -> Result<()> {
        sqlx::query(
            r#"INSERT INTO automation_strategy_lifecycle_event
                 (strategy_id, strategy_version, event_kind, from_status, to_status,
                  evaluation_id, approval_id, actor, reason, source_ref)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10::jsonb)"#,
        )
        .bind(strategy_id)
        .bind(strategy_version)
        .bind(event_kind)
        .bind(from_status)
        .bind(to_status)
        .bind(evaluation_id)
        .bind(approval_id)
        .bind(actor)
        .bind(reason)
        .bind(source_ref)
        .execute(&self.pool)
        .await
        .context("insert automation strategy lifecycle event")?;
        Ok(())
    }

    pub async fn set_automation_strategy_stage_manual(
        &self,
        strategy_id: &str,
        strategy_version: &str,
        to_stage: AutomationStage,
        actor: &str,
        reason: Option<&str>,
    ) -> Result<bool> {
        let Some(current): Option<String> = sqlx::query_scalar(
            r#"SELECT status
                 FROM automation_strategy_definition
                WHERE strategy_id = $1
                  AND strategy_version = $2"#,
        )
        .bind(strategy_id)
        .bind(strategy_version)
        .fetch_optional(&self.pool)
        .await
        .context("select automation strategy stage")?
        else {
            return Ok(false);
        };
        let from_stage = AutomationStage::try_from(current.as_str())?;
        let change = manual_stage_change(from_stage, to_stage);
        if !change.allowed {
            return Ok(false);
        }
        let updated = sqlx::query(
            r#"UPDATE automation_strategy_definition
                  SET status = $3,
                      retired_at = CASE WHEN $3 = 'retired' THEN now() ELSE retired_at END
                WHERE strategy_id = $1
                  AND strategy_version = $2
                  AND status = $4"#,
        )
        .bind(strategy_id)
        .bind(strategy_version)
        .bind(to_stage.as_str())
        .bind(from_stage.as_str())
        .execute(&self.pool)
        .await
        .context("manual automation strategy stage change")?
        .rows_affected();
        if updated == 0 {
            return Ok(false);
        }
        self.insert_automation_strategy_lifecycle_event(
            strategy_id,
            strategy_version,
            change.event_kind,
            Some(from_stage.as_str()),
            Some(to_stage.as_str()),
            None,
            None,
            actor,
            reason,
            serde_json::json!({"source": "manual_stage_change"}),
        )
        .await?;
        Ok(true)
    }

    pub async fn automation_timeline(
        &self,
        symbol: Option<&str>,
        strategy_id: Option<&str>,
        limit: Option<i64>,
    ) -> Result<serde_json::Value> {
        let symbol = symbol.map(str::trim).filter(|s| !s.is_empty());
        let strategy_id = strategy_id.map(str::trim).filter(|s| !s.is_empty());
        let limit = limit.unwrap_or(80).clamp(1, 200);
        let rows = sqlx::query(
            r#"WITH events AS (
                SELECT 'permission_event'::text AS source_kind,
                       pe.id::text AS source_id,
                       pe.occurred_at,
                       p.symbol,
                       p.strategy_id,
                       p.strategy_version,
                       pe.permission_id,
                       NULL::uuid AS sleeve_id,
                       NULL::uuid AS desired_position_id,
                       NULL::uuid AS proof_id,
                       NULL::uuid AS reconciliation_id,
                       concat('Permission ', replace(pe.event_kind, '_', ' ')) AS title,
                       COALESCE(NULLIF(pe.reason, ''), concat('Permission event recorded by ', pe.actor, '.')) AS summary,
                       COALESCE(pe.to_status, p.status) AS status,
                       jsonb_build_object(
                         'table', 'automation_permission_event',
                         'event_kind', pe.event_kind,
                         'from_status', pe.from_status,
                         'to_status', pe.to_status,
                         'manual_freeze', pe.manual_freeze,
                         'actor', pe.actor,
                         'reason', pe.reason,
                         'source_ref', pe.source_ref
                       ) AS payload
                  FROM automation_permission_event pe
                  JOIN automation_trade_permission p ON p.permission_id = pe.permission_id

                UNION ALL

                SELECT 'desired_position'::text AS source_kind,
                       d.desired_position_id::text AS source_id,
                       d.emitted_at AS occurred_at,
                       d.symbol,
                       d.strategy_id,
                       d.strategy_version,
                       d.permission_id,
                       d.sleeve_id,
                       d.desired_position_id,
                       NULL::uuid AS proof_id,
                       NULL::uuid AS reconciliation_id,
                       concat('Desired ', d.target_side, ' exposure') AS title,
                       COALESCE(NULLIF(d.rationale, ''), concat('Strategy emitted desired ', d.target_side, ' target.')) AS summary,
                       d.target_side AS status,
                       jsonb_build_object(
                         'table', 'desired_strategy_position',
                         'target_side', d.target_side,
                         'target_quantity', d.target_quantity::float8,
                         'target_notional_usd', d.target_notional_usd::float8,
                         'target_weight_pct', d.target_weight_pct::float8,
                         'rationale', d.rationale,
                         'reason_codes', d.reason_codes,
                         'feature_snapshot', d.feature_snapshot,
                         'signal_ref', d.signal_ref,
                         'supersedes_desired_position_id', d.supersedes_desired_position_id
                       ) AS payload
                  FROM desired_strategy_position d

                UNION ALL

                SELECT 'proof'::text AS source_kind,
                       ap.proof_id::text AS source_id,
                       ap.evaluated_at AS occurred_at,
                       ap.symbol,
                       ap.strategy_id,
                       ap.strategy_version,
                       ap.permission_id,
                       ap.sleeve_id,
                       ap.desired_position_id,
                       ap.proof_id,
                       NULL::uuid AS reconciliation_id,
                       concat('Proof ', ap.result) AS title,
                       CASE
                         WHEN ap.result = 'blocked' THEN 'Automation proof blocked the desired exposure.'
                         WHEN ap.result = 'warning' THEN 'Automation proof passed with warnings.'
                         ELSE 'Automation proof passed.'
                       END AS summary,
                       ap.result AS status,
                       jsonb_build_object(
                         'table', 'automation_proof',
                         'environment_scope', ap.environment_scope,
                         'strategy_config_hash', ap.strategy_config_hash,
                         'blocked_reasons', ap.blocked_reasons,
                         'risk_result', ap.risk_result,
                         'data_freshness', ap.data_freshness,
                         'session_state', ap.session_state,
                         'capital_allocation', ap.capital_allocation,
                         'broker_reconciliation', ap.broker_reconciliation
                       ) AS payload
                  FROM automation_proof ap

                UNION ALL

                SELECT 'reconciliation'::text AS source_kind,
                       ar.reconciliation_id::text AS source_id,
                       ar.updated_at AS occurred_at,
                       ar.symbol,
                       d.strategy_id,
                       d.strategy_version,
                       d.permission_id,
                       ar.sleeve_id,
                       ar.desired_position_id,
                       ar.proof_id,
                       ar.reconciliation_id,
                       concat('Reconciliation ', ar.status) AS title,
                       CASE
                         WHEN ar.status = 'needs_order' THEN 'Desired exposure needs a broker order.'
                         WHEN ar.status = 'reconciled' THEN 'Desired exposure reconciled to sleeve/broker state.'
                         WHEN ar.status = 'blocked' THEN 'Reconciliation blocked execution.'
                         ELSE 'Automation reconciliation updated.'
                       END AS summary,
                       ar.status,
                       jsonb_build_object(
                         'table', 'automation_execution_reconciliation',
                         'environment_scope', ar.environment_scope,
                         'idempotency_key', ar.idempotency_key,
                         'target_snapshot', ar.target_snapshot,
                         'broker_snapshot', ar.broker_snapshot,
                         'delta_snapshot', ar.delta_snapshot,
                         'order_plan', ar.order_plan,
                         'blocked_reasons', ar.blocked_reasons,
                         'source_ref', ar.source_ref,
                         'created_at', ar.created_at
                       ) AS payload
                  FROM automation_execution_reconciliation ar
                  JOIN desired_strategy_position d ON d.desired_position_id = ar.desired_position_id

                UNION ALL

                SELECT 'paper_order'::text AS source_kind,
                       bo.order_id::text AS source_id,
                       bo.updated_at AS occurred_at,
                       bo.symbol,
                       d.strategy_id,
                       d.strategy_version,
                       d.permission_id,
                       bo.sleeve_id,
                       bo.desired_position_id,
                       bo.proof_id,
                       bo.reconciliation_id,
                       concat('Paper ', bo.order_role, ' ', bo.action) AS title,
                       concat('Paper order ', bo.status, ' for ', bo.quantity::float8, ' share(s).') AS summary,
                       bo.status,
                       jsonb_build_object(
                         'table', 'automation_broker_order',
                         'broker', bo.broker,
                         'broker_account', bo.broker_account,
                         'client_order_id', bo.client_order_id,
                         'broker_order_id', bo.broker_order_id,
                         'parent_client_order_id', bo.parent_client_order_id,
                         'order_role', bo.order_role,
                         'action', bo.action,
                         'position_side', bo.position_side,
                         'order_type', bo.order_type,
                         'quantity', bo.quantity::float8,
                         'limit_price', bo.limit_price::float8,
                         'stop_price', bo.stop_price::float8,
                         'transmit', bo.transmit,
                         'created_at', bo.created_at
                       ) AS payload
                  FROM automation_broker_order bo
                  JOIN desired_strategy_position d ON d.desired_position_id = bo.desired_position_id

                UNION ALL

                SELECT 'paper_order_event'::text AS source_kind,
                       e.event_id::text AS source_id,
                       e.created_at AS occurred_at,
                       e.symbol,
                       d.strategy_id,
                       d.strategy_version,
                       d.permission_id,
                       ar.sleeve_id,
                       ar.desired_position_id,
                       ar.proof_id,
                       e.reconciliation_id,
                       concat('Paper order ', replace(e.event_kind, '_', ' ')) AS title,
                       COALESCE(NULLIF(e.message, ''), concat('Paper order event ', e.status, '.')) AS summary,
                       e.status,
                       jsonb_build_object(
                         'table', 'automation_broker_order_event',
                         'order_id', e.order_id,
                         'broker', e.broker,
                         'broker_account', e.broker_account,
                         'client_order_id', e.client_order_id,
                         'broker_order_id', e.broker_order_id,
                         'event_kind', e.event_kind,
                         'filled_quantity', e.filled_quantity::float8,
                         'fill_price', e.fill_price::float8,
                         'message', e.message
                       ) AS payload
                  FROM automation_broker_order_event e
                  JOIN automation_execution_reconciliation ar ON ar.reconciliation_id = e.reconciliation_id
                  JOIN desired_strategy_position d ON d.desired_position_id = ar.desired_position_id

                UNION ALL

                SELECT 'sleeve_event'::text AS source_kind,
                       se.id::text AS source_id,
                       se.occurred_at,
                       sl.symbol,
                       sl.strategy_id,
                       sl.strategy_version,
                       sl.permission_id,
                       se.sleeve_id,
                       NULL::uuid AS desired_position_id,
                       NULL::uuid AS proof_id,
                       NULL::uuid AS reconciliation_id,
                       concat('Sleeve ', replace(se.event_kind, '_', ' ')) AS title,
                       'Automation sleeve state changed.' AS summary,
                       COALESCE(se.to_status, sl.status) AS status,
                       jsonb_build_object(
                         'table', 'automation_sleeve_event',
                         'event_kind', se.event_kind,
                         'from_status', se.from_status,
                         'to_status', se.to_status,
                         'quantity_delta', se.quantity_delta::float8,
                         'notional_delta', se.notional_delta::float8,
                         'realized_pnl_delta', se.realized_pnl_delta::float8,
                         'source_ref', se.source_ref
                       ) AS payload
                  FROM automation_sleeve_event se
                  JOIN automation_strategy_sleeve sl ON sl.sleeve_id = se.sleeve_id

                UNION ALL

                SELECT 'incident'::text AS source_kind,
                       i.incident_id::text AS source_id,
                       i.created_at AS occurred_at,
                       i.symbol,
                       COALESCE(p.strategy_id, d.strategy_id) AS strategy_id,
                       COALESCE(p.strategy_version, d.strategy_version) AS strategy_version,
                       i.permission_id,
                       i.sleeve_id,
                       i.desired_position_id,
                       i.proof_id,
                       i.reconciliation_id,
                       i.title,
                       COALESCE(NULLIF(i.detail, ''), 'Automation incident recorded.') AS summary,
                       i.status,
                       jsonb_build_object(
                         'table', 'automation_incident',
                         'severity', i.severity,
                         'kind', i.kind,
                         'detail', i.detail,
                         'source_ref', i.source_ref,
                         'acknowledged_at', i.acknowledged_at,
                         'resolved_at', i.resolved_at,
                         'resolved_by', i.resolved_by
                       ) AS payload
                  FROM automation_incident i
             LEFT JOIN automation_trade_permission p ON p.permission_id = i.permission_id
             LEFT JOIN desired_strategy_position d ON d.desired_position_id = i.desired_position_id
            )
            SELECT source_kind, source_id, occurred_at, symbol, strategy_id,
                   strategy_version, permission_id, sleeve_id, desired_position_id,
                   proof_id, reconciliation_id, title, summary, status, payload
              FROM events
             WHERE ($1::text IS NULL OR symbol = upper($1::text))
               AND ($2::text IS NULL OR strategy_id = $2::text)
          ORDER BY occurred_at DESC, source_kind ASC, source_id DESC
             LIMIT $3"#,
        )
        .bind(symbol)
        .bind(strategy_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .context("automation_timeline")?;

        let events = rows
            .into_iter()
            .map(|row| {
                let occurred_at: DateTime<Utc> = row.try_get("occurred_at")?;
                Ok(serde_json::json!({
                    "source_kind": row.try_get::<String, _>("source_kind")?,
                    "source_id": row.try_get::<String, _>("source_id")?,
                    "occurred_at": occurred_at,
                    "symbol": row.try_get::<Option<String>, _>("symbol")?,
                    "strategy_id": row.try_get::<Option<String>, _>("strategy_id")?,
                    "strategy_version": row.try_get::<Option<String>, _>("strategy_version")?,
                    "permission_id": row.try_get::<Option<uuid::Uuid>, _>("permission_id")?,
                    "sleeve_id": row.try_get::<Option<uuid::Uuid>, _>("sleeve_id")?,
                    "desired_position_id": row.try_get::<Option<uuid::Uuid>, _>("desired_position_id")?,
                    "proof_id": row.try_get::<Option<uuid::Uuid>, _>("proof_id")?,
                    "reconciliation_id": row.try_get::<Option<uuid::Uuid>, _>("reconciliation_id")?,
                    "title": row.try_get::<String, _>("title")?,
                    "summary": row.try_get::<String, _>("summary")?,
                    "status": row.try_get::<Option<String>, _>("status")?,
                    "payload": row.try_get::<serde_json::Value, _>("payload")?,
                }))
            })
            .collect::<Result<Vec<_>>>()?;

        Ok(serde_json::json!({
            "as_of": Utc::now(),
            "filters": {
                "symbol": symbol.map(|s| s.to_ascii_uppercase()),
                "strategy_id": strategy_id,
                "limit": limit,
            },
            "events": events,
        }))
    }

    pub async fn approve_automation_permission(
        &self,
        symbol: &str,
        strategy_id: &str,
        strategy_version: &str,
        environment_scope: &str,
        max_allocation_pct: Option<f64>,
        max_notional_usd: Option<f64>,
        source: serde_json::Value,
    ) -> Result<serde_json::Value> {
        if !matches!(
            environment_scope,
            "shadow" | "paper" | "canary_live" | "expanded_live"
        ) {
            anyhow::bail!("unsupported automation environment_scope {environment_scope}");
        }
        if matches!(environment_scope, "canary_live" | "expanded_live") {
            anyhow::bail!("live automation approvals are not exposed through review packets yet");
        }

        self.ensure_builtin_automation_strategies(&builtin_strategy_definitions())
            .await?;

        let symbol = symbol.trim().to_ascii_uppercase();
        let mut tx = self
            .pool
            .begin()
            .await
            .context("begin automation approval")?;
        let existing = sqlx::query(
            r#"SELECT permission_id, status, manual_freeze
                 FROM automation_trade_permission
                WHERE symbol = $1
                  AND strategy_id = $2
                  AND strategy_version = $3
                  AND environment_scope = $4
                  AND status IN ('pending', 'approved')
             ORDER BY updated_at DESC
                LIMIT 1
                  FOR UPDATE"#,
        )
        .bind(&symbol)
        .bind(strategy_id)
        .bind(strategy_version)
        .bind(environment_scope)
        .fetch_optional(&mut *tx)
        .await
        .context("select automation permission")?;

        let source_ref = serde_json::json!({
            "source": "operator_review_packet",
            "requested": source,
        });

        let (permission_id, prior_status, inserted) = if let Some(row) = existing {
            let permission_id: uuid::Uuid = row.try_get("permission_id")?;
            let prior_status: String = row.try_get("status")?;
            sqlx::query(
                r#"UPDATE automation_trade_permission
                      SET status = 'approved',
                          instrument_scope = 'equity_long_only',
                          manual_freeze = false,
                          freeze_reason = NULL,
                          approved_by = 'operator',
                          approved_at = COALESCE(approved_at, now()),
                          expires_at = COALESCE(expires_at, now() + interval '90 days'),
                          max_allocation_pct = COALESCE($2::numeric, max_allocation_pct),
                          max_notional_usd = COALESCE($3::numeric, max_notional_usd),
                          source_ref = source_ref || $4::jsonb,
                          updated_at = now()
                    WHERE permission_id = $1"#,
            )
            .bind(permission_id)
            .bind(max_allocation_pct)
            .bind(max_notional_usd)
            .bind(&source_ref)
            .execute(&mut *tx)
            .await
            .context("update automation permission")?;
            (permission_id, Some(prior_status), false)
        } else {
            let permission_id: uuid::Uuid = sqlx::query_scalar(
                r#"INSERT INTO automation_trade_permission
                     (symbol, strategy_id, strategy_version, status, instrument_scope,
                      environment_scope, max_allocation_pct, max_notional_usd,
                      manual_freeze, approved_by, approved_at, expires_at, source_ref)
                   VALUES ($1, $2, $3, 'approved', 'equity_long_only',
                           $4, $5::numeric, $6::numeric,
                           false, 'operator', now(), now() + interval '90 days', $7::jsonb)
                RETURNING permission_id"#,
            )
            .bind(&symbol)
            .bind(strategy_id)
            .bind(strategy_version)
            .bind(environment_scope)
            .bind(max_allocation_pct)
            .bind(max_notional_usd)
            .bind(&source_ref)
            .fetch_one(&mut *tx)
            .await
            .context("insert automation permission")?;
            (permission_id, None, true)
        };

        sqlx::query(
            r#"INSERT INTO automation_permission_event
                 (permission_id, event_kind, from_status, to_status,
                  manual_freeze, actor, reason, source_ref)
               VALUES ($1, 'approved', $2, 'approved',
                       false, 'operator', $3, $4::jsonb)"#,
        )
        .bind(permission_id)
        .bind(prior_status.as_deref())
        .bind(if inserted {
            "operator approved ticker for shadow automation"
        } else {
            "operator refreshed automation approval"
        })
        .bind(&source_ref)
        .execute(&mut *tx)
        .await
        .context("insert automation permission event")?;

        let sleeve_id: uuid::Uuid = if let Some(row) = sqlx::query(
            r#"SELECT sleeve_id
                 FROM automation_strategy_sleeve
                WHERE permission_id = $1
                  AND sleeve_kind = 'strategy'
                  AND closed_at IS NULL
             ORDER BY opened_at DESC
                LIMIT 1
                  FOR UPDATE"#,
        )
        .bind(permission_id)
        .fetch_optional(&mut *tx)
        .await
        .context("select automation sleeve")?
        {
            let sleeve_id: uuid::Uuid = row.try_get("sleeve_id")?;
            sqlx::query(
                r#"UPDATE automation_strategy_sleeve
                      SET status = 'active',
                          updated_at = now()
                    WHERE sleeve_id = $1"#,
            )
            .bind(sleeve_id)
            .execute(&mut *tx)
            .await
            .context("update automation sleeve")?;
            sleeve_id
        } else {
            let sleeve_id: uuid::Uuid = sqlx::query_scalar(
                r#"INSERT INTO automation_strategy_sleeve
                     (sleeve_kind, permission_id, symbol, strategy_id,
                      strategy_version, status, source_ref)
                   VALUES ('strategy', $1, $2, $3, $4, 'active', $5::jsonb)
                RETURNING sleeve_id"#,
            )
            .bind(permission_id)
            .bind(&symbol)
            .bind(strategy_id)
            .bind(strategy_version)
            .bind(&source_ref)
            .fetch_one(&mut *tx)
            .await
            .context("insert automation sleeve")?;
            sqlx::query(
                r#"INSERT INTO automation_sleeve_event
                     (sleeve_id, event_kind, to_status, source_ref)
                   VALUES ($1, 'created', 'active', $2::jsonb)"#,
            )
            .bind(sleeve_id)
            .bind(&source_ref)
            .execute(&mut *tx)
            .await
            .context("insert automation sleeve event")?;
            sleeve_id
        };

        tx.commit().await.context("commit automation approval")?;

        Ok(serde_json::json!({
            "permission_id": permission_id,
            "sleeve_id": sleeve_id,
            "symbol": symbol,
            "strategy_id": strategy_id,
            "strategy_version": strategy_version,
            "environment_scope": environment_scope,
            "status": "approved",
            "inserted": inserted,
        }))
    }

    pub async fn automation_status(&self, symbol: Option<&str>) -> Result<serde_json::Value> {
        let control = self.automation_control_state().await?;
        let paper_order_adapter = sqlx::query_scalar::<_, serde_json::Value>(
            r#"SELECT jsonb_build_object(
                     'enabled', enabled,
                     'broker', broker,
                     'account_mode', account_mode,
                     'broker_account', broker_account,
                     'max_position_snapshot_age_seconds', max_position_snapshot_age_seconds,
                     'updated_by', updated_by,
                     'updated_at', updated_at
                   )
                 FROM automation_paper_order_config
                WHERE config_id = 1"#,
        )
        .fetch_optional(&self.pool)
        .await
        .context("automation paper order config")?
        .unwrap_or_else(|| {
            serde_json::json!({
                "enabled": false,
                "broker": "ibkr",
                "account_mode": "paper",
                "broker_account": null,
                "max_position_snapshot_age_seconds": 120,
                "updated_by": null,
                "updated_at": null,
            })
        });
        let rows = sqlx::query(
            r#"SELECT p.status AS permission_status,
                      p.manual_freeze,
                      (p.expires_at IS NOT NULL AND p.expires_at <= now()) AS is_expired,
                      p.environment_scope,
                      COALESCE(incident.open_incident_count, 0)::int8 AS open_incident_count,
                      proof.result AS latest_proof_result,
                      readiness.status AS latest_readiness_status,
                      jsonb_build_object(
                        'permission_id', p.permission_id,
                        'symbol', p.symbol,
                        'strategy_id', p.strategy_id,
                        'strategy_version', p.strategy_version,
                        'strategy_display_name', s.display_name,
                        'strategy_family', s.family,
                        'strategy_status', s.status,
                        'permission_status', p.status,
                        'derived_status', CASE
                          WHEN p.expires_at IS NOT NULL AND p.expires_at <= now() THEN 'expired'
                          WHEN p.manual_freeze THEN 'frozen'
                          ELSE p.status
                        END,
                        'instrument_scope', p.instrument_scope,
                        'environment_scope', p.environment_scope,
                        'manual_freeze', p.manual_freeze,
                        'freeze_reason', p.freeze_reason,
                        'approved_by', p.approved_by,
                        'approved_at', p.approved_at,
                        'expires_at', p.expires_at,
                        'max_allocation_pct', p.max_allocation_pct::float8,
                        'max_notional_usd', p.max_notional_usd::float8,
                        'max_quantity', p.max_quantity::float8,
                        'created_at', p.created_at,
                        'updated_at', p.updated_at,
                        'sleeve', sleeve.row,
                        'desired_position', desired.row,
                        'latest_proof', proof.row,
                        'readiness', readiness.row,
                        'reconciliation', reconciliation.row,
                        'paper_orders', paper_order.row,
                        'broker_position', broker.row,
                        'incidents', COALESCE(incident.rows, '[]'::jsonb)
                      ) AS permission
                 FROM automation_trade_permission p
                 JOIN automation_strategy_definition s
                   ON s.strategy_id = p.strategy_id
                  AND s.strategy_version = p.strategy_version
            LEFT JOIN LATERAL (
                    SELECT sl.sleeve_id,
                           jsonb_build_object(
                             'sleeve_id', sl.sleeve_id,
                             'sleeve_kind', sl.sleeve_kind,
                             'status', sl.status,
                             'current_side', sl.current_side,
                             'current_quantity', sl.current_quantity::float8,
                             'current_notional_usd', sl.current_notional_usd::float8,
                             'allocated_notional_usd', sl.allocated_notional_usd::float8,
                             'realized_pnl', sl.realized_pnl::float8,
                             'unrealized_pnl', sl.unrealized_pnl::float8,
                             'last_mark_price', sl.last_mark_price::float8,
                             'last_mark_at', sl.last_mark_at,
                             'opened_at', sl.opened_at,
                             'closed_at', sl.closed_at,
                             'updated_at', sl.updated_at
                           ) AS row
                      FROM automation_strategy_sleeve sl
                     WHERE sl.permission_id = p.permission_id
                  ORDER BY (sl.closed_at IS NULL) DESC, sl.updated_at DESC
                     LIMIT 1
                 ) sleeve ON TRUE
            LEFT JOIN LATERAL (
                    SELECT d.desired_position_id,
                           jsonb_build_object(
                             'desired_position_id', d.desired_position_id,
                             'strategy_config_hash', d.strategy_config_hash,
                             'target_side', d.target_side,
                             'target_quantity', d.target_quantity::float8,
                             'target_notional_usd', d.target_notional_usd::float8,
                             'target_weight_pct', d.target_weight_pct::float8,
                             'rationale', d.rationale,
                             'reason_codes', d.reason_codes,
                             'feature_snapshot', d.feature_snapshot,
                             'signal_ref', d.signal_ref,
                             'emitted_at', d.emitted_at
                           ) AS row
                      FROM desired_strategy_position d
                     WHERE d.permission_id = p.permission_id
                  ORDER BY d.emitted_at DESC
                     LIMIT 1
                 ) desired ON TRUE
            LEFT JOIN LATERAL (
                    SELECT ap.result,
                           jsonb_build_object(
                             'proof_id', ap.proof_id,
                             'strategy_config_hash', ap.strategy_config_hash,
                             'result', ap.result,
                             'blocked_reasons', ap.blocked_reasons,
                             'risk_result', ap.risk_result,
                             'data_freshness', ap.data_freshness,
                             'session_state', ap.session_state,
                             'capital_allocation', ap.capital_allocation,
                             'broker_reconciliation', ap.broker_reconciliation,
                             'evaluated_at', ap.evaluated_at
                           ) AS row
                      FROM automation_proof ap
                     WHERE ap.permission_id = p.permission_id
                  ORDER BY ap.evaluated_at DESC
                     LIMIT 1
                 ) proof ON TRUE
            LEFT JOIN LATERAL (
                    SELECT are.status,
                           jsonb_build_object(
                             'evaluation_id', are.evaluation_id,
                             'lifecycle_stage', are.lifecycle_stage,
                             'target_stage', are.target_stage,
                             'status', are.status,
                             'readiness_score', are.readiness_score::float8,
                             'approval_required', are.approval_required,
                             'approval_valid', are.approval_valid,
                             'freeze_live_permissions', are.freeze_live_permissions,
                             'metrics', are.metrics,
                             'blockers', are.blockers,
                             'warnings', are.warnings,
                             'lookback_days', are.lookback_days,
                             'evaluated_at', are.evaluated_at,
                             'approval', CASE
                               WHEN pa.approval_id IS NULL THEN NULL
                               ELSE jsonb_build_object(
                                 'approval_id', pa.approval_id,
                                 'from_stage', pa.from_stage,
                                 'to_stage', pa.to_stage,
                                 'status', pa.status,
                                 'approved_by', pa.approved_by,
                                 'approved_at', pa.approved_at,
                                 'expires_at', pa.expires_at,
                                 'reason', pa.reason
                               )
                             END
                           ) AS row
                      FROM automation_strategy_readiness_evaluation are
                 LEFT JOIN automation_strategy_promotion_approval pa
                        ON pa.approval_id = are.approval_id
                     WHERE are.strategy_id = p.strategy_id
                       AND are.strategy_version = p.strategy_version
                  ORDER BY are.evaluated_at DESC
                     LIMIT 1
                 ) readiness ON TRUE
            LEFT JOIN LATERAL (
                    SELECT ar.reconciliation_id,
                           jsonb_build_object(
                             'reconciliation_id', ar.reconciliation_id,
                             'status', ar.status,
                             'idempotency_key', ar.idempotency_key,
                             'target_snapshot', ar.target_snapshot,
                             'broker_snapshot', ar.broker_snapshot,
                             'delta_snapshot', ar.delta_snapshot,
                             'order_plan', ar.order_plan,
                             'blocked_reasons', ar.blocked_reasons,
                             'created_at', ar.created_at,
                             'updated_at', ar.updated_at
                           ) AS row
                      FROM automation_execution_reconciliation ar
                     WHERE (sleeve.sleeve_id IS NOT NULL AND ar.sleeve_id = sleeve.sleeve_id)
                        OR (desired.desired_position_id IS NOT NULL AND ar.desired_position_id = desired.desired_position_id)
                  ORDER BY ar.updated_at DESC
                     LIMIT 1
                 ) reconciliation ON TRUE
            LEFT JOIN LATERAL (
                    SELECT jsonb_build_object(
                             'orders_total', COUNT(*)::int,
                             'submitted', COUNT(*) FILTER (WHERE bo.status = 'submitted')::int,
                             'filled', COUNT(*) FILTER (WHERE bo.status = 'filled')::int,
                             'partially_filled', COUNT(*) FILTER (WHERE bo.status = 'partially_filled')::int,
                             'rejected', COUNT(*) FILTER (WHERE bo.status = 'rejected')::int,
                             'cancelled', COUNT(*) FILTER (WHERE bo.status = 'cancelled')::int,
                             'latest_event', (
                               SELECT jsonb_build_object(
                                        'event_id', e.event_id,
                                        'client_order_id', e.client_order_id,
                                        'broker_order_id', e.broker_order_id,
                                        'event_kind', e.event_kind,
                                        'status', e.status,
                                        'filled_quantity', e.filled_quantity::float8,
                                        'fill_price', e.fill_price::float8,
                                        'message', e.message,
                                        'created_at', e.created_at
                                      )
                                 FROM automation_broker_order_event e
                                WHERE e.reconciliation_id = reconciliation.reconciliation_id
                             ORDER BY e.created_at DESC
                                LIMIT 1
                             ),
                             'orders', COALESCE(jsonb_agg(jsonb_build_object(
                               'order_id', bo.order_id,
                               'client_order_id', bo.client_order_id,
                               'broker_order_id', bo.broker_order_id,
                               'parent_client_order_id', bo.parent_client_order_id,
                               'order_role', bo.order_role,
                               'action', bo.action,
                               'position_side', bo.position_side,
                               'order_type', bo.order_type,
                               'quantity', bo.quantity::float8,
                               'limit_price', bo.limit_price::float8,
                               'stop_price', bo.stop_price::float8,
                               'transmit', bo.transmit,
                               'status', bo.status,
                               'created_at', bo.created_at,
                               'updated_at', bo.updated_at
                             ) ORDER BY bo.created_at DESC), '[]'::jsonb)
                           ) AS row
                      FROM (
                            SELECT bo.*
                              FROM automation_broker_order bo
                             WHERE reconciliation.reconciliation_id IS NOT NULL
                               AND bo.reconciliation_id = reconciliation.reconciliation_id
                          ORDER BY bo.created_at DESC
                             LIMIT 9
                           ) bo
                 ) paper_order ON TRUE
            LEFT JOIN LATERAL (
                    SELECT jsonb_build_object(
                             'open_positions', COUNT(*)::int,
                             'broker_positions', COUNT(*) FILTER (WHERE pos.source = 'broker')::int,
                             'net_quantity', COALESCE(SUM(CASE WHEN COALESCE(pos.side, 'long') = 'short' THEN -pos.qty ELSE pos.qty END), 0)::float8,
                             'delta_notional', COALESCE(SUM(pos.delta_notional), 0)::float8,
                             'premium_at_risk', COALESCE(SUM(pos.premium_at_risk), 0)::float8,
                             'latest_sync_at', MAX(pos.broker_last_sync_at)
                           ) AS row
                      FROM position pos
                     WHERE pos.symbol = p.symbol
                       AND pos.closed_at IS NULL
                 ) broker ON TRUE
            LEFT JOIN LATERAL (
                    SELECT COUNT(*) FILTER (WHERE i.status <> 'resolved') AS open_incident_count,
                           COALESCE(jsonb_agg(jsonb_build_object(
                             'incident_id', i.incident_id,
                             'severity', i.severity,
                             'status', i.status,
                             'kind', i.kind,
                             'title', i.title,
                             'detail', i.detail,
                             'created_at', i.created_at,
                             'acknowledged_at', i.acknowledged_at,
                             'resolved_at', i.resolved_at
                           ) ORDER BY i.created_at DESC), '[]'::jsonb) AS rows
                      FROM (
                            SELECT *
                              FROM automation_incident i
                             WHERE i.status <> 'resolved'
                               AND (i.permission_id = p.permission_id
                                    OR (i.permission_id IS NULL AND i.symbol = p.symbol))
                          ORDER BY i.created_at DESC
                             LIMIT 5
                           ) i
                 ) incident ON TRUE
                WHERE ($1::text IS NULL OR p.symbol = upper($1))
             ORDER BY p.updated_at DESC, p.symbol, p.strategy_id"#,
        )
        .bind(symbol.map(str::trim).filter(|s| !s.is_empty()))
        .fetch_all(&self.pool)
        .await
        .context("automation_status")?;

        let mut permissions = Vec::with_capacity(rows.len());
        let mut approved = 0_i64;
        let mut pending = 0_i64;
        let mut frozen = 0_i64;
        let mut expired = 0_i64;
        let mut paper_only = 0_i64;
        let mut live_capable = 0_i64;
        let mut incidents_open = 0_i64;
        let mut blocked_strategies = 0_i64;
        let mut readiness_ready = 0_i64;
        let mut readiness_blocked = 0_i64;
        let mut readiness_missing = 0_i64;

        for row in rows {
            let permission_status: String = row.try_get("permission_status")?;
            let manual_freeze: bool = row.try_get("manual_freeze")?;
            let is_expired: bool = row.try_get("is_expired")?;
            let environment_scope: String = row.try_get("environment_scope")?;
            let open_incident_count: i64 = row.try_get("open_incident_count")?;
            let latest_proof_result: Option<String> = row.try_get("latest_proof_result")?;
            let latest_readiness_status: Option<String> = row.try_get("latest_readiness_status")?;

            if permission_status == "approved" {
                approved += 1;
            }
            if permission_status == "pending" {
                pending += 1;
            }
            if manual_freeze {
                frozen += 1;
            }
            if is_expired || permission_status == "expired" {
                expired += 1;
            }
            if environment_scope == "paper" {
                paper_only += 1;
            }
            if matches!(environment_scope.as_str(), "canary_live" | "expanded_live") {
                live_capable += 1;
            }
            incidents_open += open_incident_count;
            if latest_proof_result.as_deref() == Some("blocked") {
                blocked_strategies += 1;
            }
            match latest_readiness_status.as_deref() {
                Some("ready") => readiness_ready += 1,
                Some("blocked") => readiness_blocked += 1,
                _ => readiness_missing += 1,
            }

            permissions.push(row.try_get::<serde_json::Value, _>("permission")?);
        }

        let approval_candidate_rows = sqlx::query(
            r#"SELECT jsonb_build_object(
                       'attention_id', ai.id,
                       'attention_kind', ai.kind,
                       'symbol', upper(ai.symbol),
                       'title', ai.title,
                       'reason', COALESCE(ai.reason, t.edge_rationale),
                       'created_at', ai.created_at,
                       'thesis_id', t.thesis_id,
                       'thesis_state', t.state,
                       'thesis_direction', t.forecast->>'direction',
                       'thesis_edge', t.edge_rationale,
                       'strategy_id', 'thesis_timing',
                       'strategy_version', '0.1.0',
                       'strategy_display_name', 'Thesis Timing',
                       'environment_scope', 'shadow',
                       'default_max_allocation_pct', 0.05,
                       'headline', format('Approve %s for bot-managed trading?', upper(ai.symbol))
                   ) AS candidate
              FROM attention_item ai
         LEFT JOIN LATERAL (
                    SELECT t.*
                      FROM thesis t
                     WHERE (
                               ai.thesis_id IS NOT NULL
                               AND t.thesis_id = ai.thesis_id
                           )
                        OR (
                               ai.thesis_id IS NULL
                               AND ai.symbol IS NOT NULL
                               AND t.symbol = ai.symbol
                               AND t.state NOT IN ('closed', 'disqualified')
                           )
                  ORDER BY CASE WHEN t.thesis_id = ai.thesis_id THEN 0 ELSE 1 END,
                           t.updated_at DESC
                     LIMIT 1
                 ) t ON TRUE
             WHERE ai.status = 'open'
               AND ai.kind IN ('thesis_review', 'thesis_actionable')
               AND ai.symbol IS NOT NULL
               AND t.thesis_id IS NOT NULL
               AND ($1::text IS NULL OR ai.symbol = upper($1))
               AND NOT EXISTS (
                     SELECT 1
                       FROM automation_trade_permission p
                      WHERE p.symbol = upper(ai.symbol)
                        AND p.strategy_id = 'thesis_timing'
                        AND p.strategy_version = '0.1.0'
                        AND p.environment_scope = 'shadow'
                        AND p.status IN ('pending', 'approved')
                   )
          ORDER BY CASE ai.kind WHEN 'thesis_actionable' THEN 0 ELSE 1 END,
                   ai.created_at DESC
             LIMIT 25"#,
        )
        .bind(symbol.map(str::trim).filter(|s| !s.is_empty()))
        .fetch_all(&self.pool)
        .await
        .context("automation approval candidates")?;
        let approval_candidates = approval_candidate_rows
            .into_iter()
            .map(|row| row.try_get::<serde_json::Value, _>("candidate"))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(serde_json::json!({
            "as_of": Utc::now(),
            "kill_switch": {
                "enabled": control.kill_switch_enabled,
                "read_only": true,
                "source": "automation_control_state",
                "reason": control.kill_switch_reason
            },
            "paper_order_adapter": paper_order_adapter,
            "summary": {
                "permissions_total": permissions.len() as i64,
                "approved": approved,
                "pending": pending,
                "frozen": frozen,
                "expired": expired,
                "paper_only": paper_only,
                "live_capable": live_capable,
                "incidents_open": incidents_open,
                "blocked_strategies": blocked_strategies,
                "readiness_ready": readiness_ready,
                "readiness_blocked": readiness_blocked,
                "readiness_missing": readiness_missing,
                "approval_candidates": approval_candidates.len() as i64
            },
            "permissions": permissions,
            "approval_candidates": approval_candidates
        }))
    }

    pub async fn create_price_alert_rule(
        &self,
        input: PriceAlertRuleInput,
    ) -> Result<Option<PriceAlertRule>> {
        validate_rule_input(&input)?;
        let row = sqlx::query(
            r#"INSERT INTO price_alert_rule
                  (symbol, thesis_id, origin, intent, direction, target_price,
                   label, rationale, semantic_key, source_ref, expires_at)
               VALUES (upper($1), $2, $3, $4, $5, $6, $7, $8, $9, $10::jsonb, $11)
               ON CONFLICT DO NOTHING
            RETURNING id, symbol, thesis_id, origin, intent, direction,
                      target_price::float8 AS target_price, label, rationale,
                      semantic_key, status, source_ref, expires_at, created_at,
                      updated_at, triggered_at, disabled_at"#,
        )
        .bind(input.symbol.trim())
        .bind(input.thesis_id)
        .bind(input.origin)
        .bind(input.intent)
        .bind(input.direction)
        .bind(input.target_price)
        .bind(input.label.trim())
        .bind(input.rationale.as_deref())
        .bind(input.semantic_key.as_deref())
        .bind(input.source_ref)
        .bind(input.expires_at)
        .fetch_optional(&self.pool)
        .await
        .context("create_price_alert_rule")?;
        row.map(decode_price_alert_rule).transpose()
    }

    pub async fn update_price_alert_rule(
        &self,
        id: i64,
        patch: PriceAlertRulePatch,
    ) -> Result<Option<PriceAlertRule>> {
        validate_rule_patch(&patch)?;
        let row = sqlx::query(
            r#"UPDATE price_alert_rule
                  SET intent = COALESCE($2, intent),
                      direction = COALESCE($3, direction),
                      target_price = COALESCE($4, target_price::float8),
                      label = COALESCE($5, label),
                      rationale = COALESCE($6, rationale),
                      expires_at = COALESCE($7, expires_at),
                      status = COALESCE($8, status),
                      disabled_at = CASE
                        WHEN $8 = 'disabled' AND disabled_at IS NULL THEN now()
                        ELSE disabled_at
                      END,
                      updated_at = now()
                WHERE id = $1
            RETURNING id, symbol, thesis_id, origin, intent, direction,
                      target_price::float8 AS target_price, label, rationale,
                      semantic_key, status, source_ref, expires_at, created_at,
                      updated_at, triggered_at, disabled_at"#,
        )
        .bind(id)
        .bind(patch.intent.as_deref())
        .bind(patch.direction.as_deref())
        .bind(patch.target_price)
        .bind(patch.label.as_deref())
        .bind(patch.rationale.as_deref())
        .bind(patch.expires_at)
        .bind(patch.status.as_deref())
        .fetch_optional(&self.pool)
        .await
        .context("update_price_alert_rule")?;
        row.map(decode_price_alert_rule).transpose()
    }

    pub async fn disable_price_alert_rule(&self, id: i64) -> Result<Option<PriceAlertRule>> {
        self.update_price_alert_rule(
            id,
            PriceAlertRulePatch {
                status: Some("disabled".to_string()),
                ..Default::default()
            },
        )
        .await
    }

    pub async fn expire_price_alert_rules(&self) -> Result<u64> {
        let res = sqlx::query(
            r#"UPDATE price_alert_rule
                  SET status = 'expired', updated_at = now()
                WHERE status = 'active'
                  AND expires_at IS NOT NULL
                  AND expires_at <= now()"#,
        )
        .execute(&self.pool)
        .await
        .context("expire_price_alert_rules")?;
        Ok(res.rows_affected())
    }

    pub async fn active_price_alert_symbols(&self) -> Result<Vec<String>> {
        let rows = sqlx::query(
            r#"SELECT DISTINCT symbol
                 FROM price_alert_rule
                WHERE status = 'active'
                  AND (expires_at IS NULL OR expires_at > now())
             ORDER BY symbol"#,
        )
        .fetch_all(&self.pool)
        .await
        .context("active_price_alert_symbols")?;
        rows.into_iter()
            .map(|r| r.try_get("symbol").map_err(Into::into))
            .collect()
    }

    pub async fn active_price_alert_rules_for_symbol(
        &self,
        symbol: &str,
    ) -> Result<Vec<PriceAlertRule>> {
        let rows = sqlx::query(
            r#"SELECT id, symbol, thesis_id, origin, intent, direction,
                      target_price::float8 AS target_price, label, rationale,
                      semantic_key, status, source_ref, expires_at, created_at,
                      updated_at, triggered_at, disabled_at
                 FROM price_alert_rule
                WHERE symbol = upper($1)
                  AND status = 'active'
                  AND (expires_at IS NULL OR expires_at > now())
             ORDER BY created_at ASC"#,
        )
        .bind(symbol)
        .fetch_all(&self.pool)
        .await
        .context("active_price_alert_rules_for_symbol")?;
        rows.into_iter().map(decode_price_alert_rule).collect()
    }

    pub async fn latest_daily_bar_for_alert(
        &self,
        symbol: &str,
    ) -> Result<Option<(DateTime<Utc>, f64, f64, f64)>> {
        let row = sqlx::query(
            r#"SELECT ts, high::float8 AS high, low::float8 AS low, close::float8 AS close
                 FROM price_bar
                WHERE symbol = upper($1)
             ORDER BY ts DESC
                LIMIT 1"#,
        )
        .bind(symbol)
        .fetch_optional(&self.pool)
        .await
        .context("latest_daily_bar_for_alert")?;
        row.map(|r| {
            Ok((
                r.try_get("ts")?,
                r.try_get("high")?,
                r.try_get("low")?,
                r.try_get("close")?,
            ))
        })
        .transpose()
    }

    pub async fn latest_intraday_bar_for_alert(
        &self,
        symbol: &str,
    ) -> Result<Option<(DateTime<Utc>, f64, f64, f64)>> {
        let row = sqlx::query(
            r#"SELECT ts, high::float8 AS high, low::float8 AS low, close::float8 AS close
                 FROM price_bar_intraday
                WHERE symbol = upper($1)
             ORDER BY ts DESC
                LIMIT 1"#,
        )
        .bind(symbol)
        .fetch_optional(&self.pool)
        .await
        .context("latest_intraday_bar_for_alert")?;
        row.map(|r| {
            Ok((
                r.try_get("ts")?,
                r.try_get("high")?,
                r.try_get("low")?,
                r.try_get("close")?,
            ))
        })
        .transpose()
    }

    pub async fn trigger_price_alert_rule(
        &self,
        rule: &PriceAlertRule,
        trigger: &PriceTrigger,
    ) -> Result<Option<PriceAlertEvent>> {
        let mut tx = self
            .pool
            .begin()
            .await
            .context("trigger price alert begin")?;
        let updated = sqlx::query(
            r#"UPDATE price_alert_rule
                  SET status = 'triggered',
                      triggered_at = now(),
                      updated_at = now()
                WHERE id = $1
                  AND status = 'active'
                  AND (expires_at IS NULL OR expires_at > now())
            RETURNING id"#,
        )
        .bind(rule.id)
        .fetch_optional(&mut *tx)
        .await
        .context("trigger price alert update")?;
        if updated.is_none() {
            tx.rollback().await.ok();
            return Ok(None);
        }
        let snapshot = serde_json::to_value(rule).context("price alert rule snapshot")?;
        let row = sqlx::query(
            r#"INSERT INTO price_alert_event
                  (rule_id, symbol, thesis_id, trigger_ts, trigger_interval,
                   trigger_price, rule_snapshot)
               VALUES ($1, $2, $3, $4, $5, $6, $7::jsonb)
               ON CONFLICT (rule_id) DO NOTHING
            RETURNING id, rule_id, symbol, thesis_id, triggered_at, trigger_ts,
                      trigger_interval, trigger_price::float8 AS trigger_price,
                      rule_snapshot"#,
        )
        .bind(rule.id)
        .bind(&rule.symbol)
        .bind(rule.thesis_id)
        .bind(trigger.ts)
        .bind(&trigger.interval)
        .bind(trigger.price)
        .bind(snapshot)
        .fetch_optional(&mut *tx)
        .await
        .context("trigger price alert event")?;
        tx.commit().await.context("trigger price alert commit")?;
        row.map(decode_price_alert_event).transpose()
    }

    pub async fn list_price_alert_events(
        &self,
        symbol: Option<&str>,
        limit: i64,
    ) -> Result<Vec<PriceAlertEvent>> {
        let rows = sqlx::query(
            r#"SELECT id, rule_id, symbol, thesis_id, triggered_at, trigger_ts,
                      trigger_interval, trigger_price::float8 AS trigger_price,
                      rule_snapshot
                 FROM price_alert_event
                WHERE ($1::text IS NULL OR symbol = upper($1))
             ORDER BY triggered_at DESC
                LIMIT $2"#,
        )
        .bind(symbol)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .context("list_price_alert_events")?;
        rows.into_iter().map(decode_price_alert_event).collect()
    }

    /// Returns the latest market_state row for the UI. None if the table is empty.
    pub async fn latest_market_state(&self) -> Result<Option<MarketStateRow>> {
        let row = sqlx::query(
            r#"SELECT as_of, regime, capitulation, indicators
                 FROM market_state ORDER BY as_of DESC LIMIT 1"#,
        )
        .fetch_optional(&self.pool)
        .await
        .context("latest_market_state")?;
        let Some(row) = row else { return Ok(None) };
        Ok(Some(MarketStateRow {
            as_of: row.try_get("as_of")?,
            regime: row.try_get("regime")?,
            capitulation: row.try_get("capitulation")?,
            indicators: row.try_get("indicators")?,
        }))
    }

    /// Lists active tracked tickers with their cluster + tier for the UI sidebar.
    pub async fn active_tickers(&self) -> Result<Vec<TickerRow>> {
        // Cast NUMERIC → float8 in SQL to avoid the bigdecimal feature pull-in.
        let rows = sqlx::query(
            r#"SELECT t.symbol,
                      COALESCE(t.cluster_id, '')        AS cluster_id,
                      c.name                            AS cluster_name,
                      t.tier,
                      t.options_eligible,
                      t.domain_fit::float8              AS domain_fit,
                      t.added_at,
                      latest.thesis_id                  AS latest_thesis_id,
                      latest.state                      AS thesis_state,
                      latest.direction                   AS thesis_direction,
                      COALESCE(tech_obs.technical_state, tech.technical_state)
                                                        AS technical_state,
                      COALESCE(tech_obs.setup_kind, tech.setup_kind)
                                                        AS technical_setup_kind,
                      COALESCE(tech_obs.entry_stance, tech.entry_stance)
                                                        AS entry_stance,
                      COALESCE(tech_obs.input_snapshot->'cross', '{}'::jsonb)
                                                        AS technical_cross,
                      tech.pct_vs_200d                  AS technical_pct_vs_200d,
                      freshness.status                  AS freshness_status,
                      COALESCE(attention.open_count, 0) AS open_attention,
                      attention.review_packet_attention_id,
                      COALESCE(attention.states, '[]'::jsonb) AS attention_states,
                      COALESCE(attention.owners, '[]'::jsonb) AS attention_owners,
                      COALESCE(evidence.open_count, 0) AS open_evidence,
                      COALESCE(evidence.blocking_count, 0) AS blocking_evidence,
                      COALESCE(tasks.due_count, 0) AS due_source_tasks,
                      COALESCE(brain.parent_themes, '[]'::jsonb) AS parent_themes,
                      (SELECT count(*) FROM thesis th
                        WHERE th.symbol = t.symbol
                          AND th.state NOT IN ('closed','disqualified')) AS open_theses
                 FROM ticker t
            LEFT JOIN cluster c ON c.id = t.cluster_id
            LEFT JOIN LATERAL (
                SELECT th.thesis_id, th.state, th.forecast->>'direction' AS direction,
                       th.forecast, th.conviction_tier, th.system_confidence,
                       th.updated_at,
                       COALESCE(th.last_evaluated_at, th.updated_at) AS evaluated_at
                  FROM thesis th
                 WHERE th.symbol = t.symbol
                   AND th.state NOT IN ('closed','disqualified')
              ORDER BY th.updated_at DESC, th.created_at DESC
                 LIMIT 1
            ) latest ON TRUE
            LEFT JOIN LATERAL (
                WITH bars AS (
                    SELECT ts, close::float8 AS close, high::float8 AS high
                      FROM price_bar
                     WHERE symbol = t.symbol
                  ORDER BY ts DESC
                     LIMIT 260
                ), ranked AS (
                    SELECT ts, close, high, row_number() OVER (ORDER BY ts DESC) AS rn
                      FROM bars
                ), latest_bar AS (
                    SELECT ts, close
                      FROM ranked
                     WHERE rn = 1
                ), stats AS (
                    SELECT count(*) FILTER (WHERE rn <= 200) AS bars_200,
                           avg(close) FILTER (WHERE rn <= 50) AS sma50,
                           avg(close) FILTER (WHERE rn <= 200) AS sma200,
                           max(high) FILTER (WHERE rn <= 252) AS high252
                      FROM ranked
                ), classified AS (
                    SELECT CASE
                             WHEN stats.bars_200 < 200 OR stats.sma200 IS NULL THEN 'unknown'
                             WHEN ((latest_bar.close - stats.sma200) / NULLIF(stats.sma200, 0) * 100.0) > 20.0
                               OR ((latest_bar.close - stats.high252) / NULLIF(stats.high252, 0) * 100.0) >= -2.0 THEN 'extended'
                             WHEN ((latest_bar.close - stats.sma200) / NULLIF(stats.sma200, 0) * 100.0) < -5.0 THEN 'deteriorating'
                             WHEN stats.sma50 IS NOT NULL
                               AND abs((latest_bar.close - stats.sma50) / NULLIF(stats.sma50, 0) * 100.0) <= 5.0 THEN 'base_building'
                             WHEN ((latest_bar.close - stats.sma200) / NULLIF(stats.sma200, 0) * 100.0) >= 0.0 THEN 'constructive'
                             ELSE 'unknown'
                           END AS technical_state,
                           CASE
                             WHEN stats.bars_200 < 200 OR stats.sma200 IS NULL THEN 'unknown'
                             WHEN ((latest_bar.close - stats.sma200) / NULLIF(stats.sma200, 0) * 100.0) > 20.0
                               OR ((latest_bar.close - stats.high252) / NULLIF(stats.high252, 0) * 100.0) >= -2.0 THEN 'extended_run'
                             WHEN ((latest_bar.close - stats.sma200) / NULLIF(stats.sma200, 0) * 100.0) < -5.0 THEN 'breakdown'
                             WHEN stats.sma50 IS NOT NULL
                               AND abs((latest_bar.close - stats.sma50) / NULLIF(stats.sma50, 0) * 100.0) <= 5.0 THEN 'base_building'
                             WHEN ((latest_bar.close - stats.sma200) / NULLIF(stats.sma200, 0) * 100.0) >= 0.0 THEN 'constructive_trend'
                             ELSE 'unknown'
                           END AS setup_kind,
                           latest_bar.ts AS as_of,
                           ((latest_bar.close - stats.sma200) / NULLIF(stats.sma200, 0) * 100.0)::float8 AS pct_vs_200d
                      FROM latest_bar CROSS JOIN stats
                )
                SELECT technical_state,
                       setup_kind,
                       CASE technical_state
                         WHEN 'extended' THEN 'avoid_chase'
                         WHEN 'deteriorating' THEN 'avoid'
                         WHEN 'base_building' THEN 'wait_breakout'
                         WHEN 'constructive' THEN 'constructive'
                         ELSE 'wait_data'
                       END AS entry_stance,
                       as_of,
                       pct_vs_200d
                  FROM classified
            ) tech ON TRUE
            LEFT JOIN LATERAL (
                SELECT o.technical_state,
                       o.setup_kind,
                       o.entry_stance,
                       o.input_snapshot
                  FROM technical_timing_observation o
                 WHERE o.symbol = t.symbol
                   AND tech.as_of IS NOT NULL
                   AND o.observed_at = tech.as_of
              ORDER BY o.created_at DESC
                 LIMIT 1
            ) tech_obs ON TRUE
            LEFT JOIN LATERAL (
                SELECT tc.created_at AS context_at
                  FROM ticker_context tc
                 WHERE tc.symbol = t.symbol
              ORDER BY tc.version DESC
                 LIMIT 1
            ) ctx ON TRUE
            LEFT JOIN LATERAL (
                SELECT count(*) AS rows_count,
                       count(*) FILTER (WHERE er.blocking_state <> 'satisfied') AS open_count,
                       count(*) FILTER (
                         WHERE er.priority = 'blocking'
                           AND er.blocking_state <> 'satisfied'
                       ) AS blocking_count
                  FROM evidence_requirement er
                 WHERE er.symbol = t.symbol
            ) evidence ON TRUE
            LEFT JOIN LATERAL (
                SELECT count(*) FILTER (
                         WHERE st.state IN ('queued', 'no_rows', 'failed', 'rate_limited', 'blocked')
                           AND st.due_at <= now()
                       ) AS due_count,
                       count(*) FILTER (
                         WHERE st.state = 'fetching'
                           AND st.updated_at < now() - interval '30 minutes'
                       ) AS stale_fetching_count,
                       count(*) FILTER (
                         WHERE st.state IN ('failed', 'rate_limited', 'blocked')
                       ) AS blocked_count
                  FROM source_task st
                 WHERE st.scope = 'symbol'
                   AND st.target_id = t.symbol
            ) tasks ON TRUE
            LEFT JOIN LATERAL (
                SELECT CASE
                         WHEN COALESCE(evidence.blocking_count, 0) > 0
                           OR COALESCE(tasks.blocked_count, 0) > 0 THEN 'blocked'
                         WHEN latest.thesis_id IS NULL
                           OR ctx.context_at IS NULL
                           OR COALESCE(evidence.rows_count, 0) = 0 THEN 'missing'
                         WHEN COALESCE(evidence.open_count, 0) > 0
                           OR COALESCE(tasks.due_count, 0) > 0
                           OR COALESCE(tasks.stale_fetching_count, 0) > 0
                           OR ctx.context_at < now() - interval '12 hours'
                           OR latest.evaluated_at IS NULL
                           OR latest.evaluated_at < now() - interval '30 minutes' THEN 'stale'
                         ELSE 'fresh'
                       END AS status
            ) freshness ON TRUE
            LEFT JOIN LATERAL (
                SELECT
                    (SELECT count(*)
                       FROM attention_item ai
                      WHERE ai.symbol = t.symbol
                        AND ai.status = 'open'
                        AND (ai.fsm_state <> 'operator_deferred'
                             OR (ai.resurface_at IS NOT NULL AND ai.resurface_at <= now()))) AS open_count,
                    (SELECT ai.id
                       FROM attention_item ai
                      WHERE ai.symbol = t.symbol
                        AND ai.status = 'open'
                        AND (ai.fsm_state <> 'operator_deferred'
                             OR (ai.resurface_at IS NOT NULL AND ai.resurface_at <= now()))
                   ORDER BY CASE ai.severity
                              WHEN 'blocked' THEN 0
                              WHEN 'decision' THEN 1
                              WHEN 'review' THEN 2
                              ELSE 3
                            END,
                            ai.created_at DESC
                      LIMIT 1) AS review_packet_attention_id,
                    COALESCE((
                        SELECT jsonb_agg(jsonb_build_object('state', s.fsm_state, 'count', s.n)
                                         ORDER BY s.n DESC, s.fsm_state)
                          FROM (
                              SELECT ai.fsm_state, count(*) AS n
                                FROM attention_item ai
                               WHERE ai.symbol = t.symbol
                                 AND ai.status = 'open'
                                 AND (ai.fsm_state <> 'operator_deferred'
                                      OR (ai.resurface_at IS NOT NULL AND ai.resurface_at <= now()))
                            GROUP BY ai.fsm_state
                          ) s
                    ), '[]'::jsonb) AS states,
                    COALESCE((
                        SELECT jsonb_agg(jsonb_build_object('owner', o.owner, 'count', o.n)
                                         ORDER BY o.n DESC, o.owner)
                          FROM (
                              SELECT ai.owner, count(*) AS n
                                FROM attention_item ai
                               WHERE ai.symbol = t.symbol
                                 AND ai.status = 'open'
                                 AND (ai.fsm_state <> 'operator_deferred'
                                      OR (ai.resurface_at IS NOT NULL AND ai.resurface_at <= now()))
                            GROUP BY ai.owner
                          ) o
                    ), '[]'::jsonb) AS owners
            ) attention ON TRUE
            LEFT JOIN LATERAL (
                SELECT COALESCE(jsonb_agg(
                         jsonb_build_object(
                           'key', bt.key,
                           'name', bt.name,
                           'scope', bt.scope,
                           'state', bt.state,
                           'direction', bt.direction,
                           'role', btt.role,
                           'mapping_conviction', btt.conviction,
                           'conviction', brain_ticker_live_conviction(
                               btt.conviction,
                               latest.conviction_tier,
                               latest.system_confidence,
                               latest.forecast
                           ),
                           'thesis_state', latest.state,
                           'thesis_direction', latest.direction,
                           'thesis_conviction_tier', latest.conviction_tier,
                           'thesis_system_confidence', latest.system_confidence,
                           'thesis_updated_at', latest.updated_at,
                           'link_created_at', btt.created_at,
                           'link_stale', latest.updated_at IS NOT NULL
                               AND btt.created_at IS NOT NULL
                               AND latest.updated_at > btt.created_at
                         )
                         ORDER BY brain_ticker_live_conviction(
                               btt.conviction,
                               latest.conviction_tier,
                               latest.system_confidence,
                               latest.forecast
                           ) DESC, bt.name
                       ), '[]'::jsonb) AS parent_themes
                  FROM brain_thesis_ticker btt
                  JOIN brain_thesis bt ON bt.id = btt.brain_thesis_id
                 WHERE btt.symbol = t.symbol
                   AND bt.active = true
            ) brain ON TRUE
                WHERE t.status = 'active'
             ORDER BY t.tier ASC, t.symbol ASC"#,
        )
        .fetch_all(&self.pool)
        .await
        .context("active_tickers")?;
        rows.into_iter()
            .map(|row| {
                Ok(TickerRow {
                    symbol: row.try_get("symbol")?,
                    cluster_id: row.try_get("cluster_id")?,
                    cluster_name: row.try_get::<Option<String>, _>("cluster_name")?,
                    tier: row.try_get("tier")?,
                    options_eligible: row.try_get("options_eligible")?,
                    domain_fit: row.try_get::<Option<f64>, _>("domain_fit").ok().flatten(),
                    added_at: row.try_get("added_at")?,
                    open_theses: row.try_get::<i64, _>("open_theses").unwrap_or(0),
                    latest_thesis_id: row.try_get("latest_thesis_id").ok(),
                    thesis_state: row.try_get("thesis_state").ok(),
                    thesis_direction: row.try_get("thesis_direction").ok(),
                    technical_state: row.try_get("technical_state").ok(),
                    technical_setup_kind: row.try_get("technical_setup_kind").ok(),
                    entry_stance: row.try_get("entry_stance").ok(),
                    technical_cross: row
                        .try_get("technical_cross")
                        .unwrap_or_else(|_| serde_json::json!({})),
                    technical_pct_vs_200d: row.try_get("technical_pct_vs_200d").ok(),
                    freshness_status: row
                        .try_get("freshness_status")
                        .unwrap_or_else(|_| "missing".to_string()),
                    open_attention: row.try_get::<i64, _>("open_attention").unwrap_or(0),
                    review_packet_attention_id: row.try_get("review_packet_attention_id").ok(),
                    attention_states: row
                        .try_get("attention_states")
                        .unwrap_or_else(|_| serde_json::json!([])),
                    attention_owners: row
                        .try_get("attention_owners")
                        .unwrap_or_else(|_| serde_json::json!([])),
                    open_evidence: row.try_get::<i64, _>("open_evidence").unwrap_or(0),
                    blocking_evidence: row.try_get::<i64, _>("blocking_evidence").unwrap_or(0),
                    due_source_tasks: row.try_get::<i64, _>("due_source_tasks").unwrap_or(0),
                    parent_themes: row
                        .try_get("parent_themes")
                        .unwrap_or_else(|_| serde_json::json!([])),
                })
            })
            .collect()
    }

    /// Latest `ticker_context` row for a symbol. None if never synthesized.
    pub async fn latest_ticker_context(&self, symbol: &str) -> Result<Option<TickerContextRow>> {
        let row = sqlx::query(
            r#"SELECT symbol, version,
                      COALESCE(structural, '{}'::jsonb) AS structural,
                      structural_as_of,
                      COALESCE(narrative,  '{}'::jsonb) AS narrative,
                      narrative_as_of,
                      COALESCE(market,     '{}'::jsonb) AS market,
                      market_as_of,
                      created_at
                 FROM ticker_context
                WHERE symbol = $1
             ORDER BY version DESC
                LIMIT 1"#,
        )
        .bind(symbol)
        .fetch_optional(&self.pool)
        .await
        .context("latest_ticker_context")?;
        let Some(row) = row else { return Ok(None) };
        Ok(Some(TickerContextRow {
            symbol: row.try_get("symbol")?,
            version: row.try_get("version")?,
            structural: row.try_get("structural")?,
            structural_as_of: row.try_get("structural_as_of")?,
            narrative: row.try_get("narrative")?,
            narrative_as_of: row.try_get("narrative_as_of")?,
            market: row.try_get("market")?,
            market_as_of: row.try_get("market_as_of")?,
            created_at: row.try_get("created_at")?,
        }))
    }

    /// Loads a single thesis by id, with the same enrichment that
    /// `theses_for_symbol` produces (substance, history). Returns
    /// `Vec<ThesisDetail>` (will have 0 or 1 entry) so the caller can reuse
    /// the existing per-symbol code path.
    pub async fn theses_for_symbol_id(&self, thesis_id: uuid::Uuid) -> Result<Vec<ThesisDetail>> {
        let symbol: Option<String> =
            sqlx::query_scalar("SELECT symbol FROM thesis WHERE thesis_id = $1")
                .bind(thesis_id)
                .fetch_optional(&self.pool)
                .await
                .context("symbol lookup")?;
        let Some(symbol) = symbol else {
            return Ok(vec![]);
        };
        let all = self.theses_for_symbol(&symbol).await?;
        Ok(all
            .into_iter()
            .filter(|t| t.thesis_id == thesis_id)
            .collect())
    }

    /// Apply a state transition (#15). Caller must have already validated the
    /// edge via `thesis::substance::promotion_allowed`. Writes both the new
    /// state on the thesis row and an append-only `thesis_state_history` row.
    pub async fn apply_state_transition(
        &self,
        thesis_id: uuid::Uuid,
        from: crate::platform::domain::ThesisState,
        to: crate::platform::domain::ThesisState,
        rationale: &str,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.context("begin tx")?;
        sqlx::query("UPDATE thesis SET state = $1, updated_at = now() WHERE thesis_id = $2")
            .bind(to.as_str())
            .bind(thesis_id)
            .execute(&mut *tx)
            .await
            .context("update thesis state")?;
        sqlx::query(
            r#"INSERT INTO thesis_state_history (thesis_id, from_state, to_state, rationale)
               VALUES ($1, $2, $3, NULLIF($4, ''))"#,
        )
        .bind(thesis_id)
        .bind(from.as_str())
        .bind(to.as_str())
        .bind(rationale)
        .execute(&mut *tx)
        .await
        .context("insert state history")?;
        // Attention queue producers/resolvers (#86) for state transitions.
        // Entering 'actionable' fires thesis_actionable; leaving 'actionable'
        // (forward to position_open OR backward to disqualified) resolves it.
        use crate::platform::domain::ThesisState;
        if matches!(to, ThesisState::Actionable) {
            // Look up the symbol for the title.
            let symbol: String =
                sqlx::query_scalar("SELECT symbol FROM thesis WHERE thesis_id = $1")
                    .bind(thesis_id)
                    .fetch_one(&mut *tx)
                    .await
                    .unwrap_or_default();
            let (fsm_state, owner) = crate::attention::initial_assignment(
                crate::attention::kind::THESIS_ACTIONABLE,
                crate::attention::severity::DECISION,
                crate::attention::source::THESIS,
            );
            sqlx::query(
                r#"INSERT INTO attention_item
                     (kind, symbol, thesis_id, severity, title, reason, source, source_ref,
                      fsm_state, owner, state_reason)
                   VALUES ('thesis_actionable', $1, $2, 'decision', $3, $4, 'thesis',
                           jsonb_build_object('from', $5::text, 'to', 'actionable'),
                           $6, $7, 'thesis_actionable')
                   ON CONFLICT DO NOTHING"#,
            )
            .bind(&symbol)
            .bind(thesis_id)
            .bind(format!("{symbol} thesis ready to act on"))
            .bind(if rationale.is_empty() {
                None
            } else {
                Some(rationale)
            })
            .bind(from.as_str())
            .bind(fsm_state)
            .bind(owner)
            .execute(&mut *tx)
            .await
            .context("attention thesis_actionable")?;
        }
        if matches!(from, ThesisState::Actionable) {
            sqlx::query(
                r#"WITH matched AS (
                        SELECT id, fsm_state
                          FROM attention_item
                         WHERE status = 'open'
                           AND kind = 'thesis_actionable'
                           AND thesis_id = $1
                         FOR UPDATE
                     ),
                     updated AS (
                        UPDATE attention_item ai
                           SET status = 'resolved',
                               fsm_state = 'resolved',
                               owner = 'system',
                               resolved_at = now(),
                               resolution_kind = 'thesis_advanced',
                               resolution_ref = jsonb_build_object('to', $2::text),
                               next_retry_at = NULL,
                               resurface_at = NULL,
                               state_reason = 'thesis_advanced'
                          FROM matched m
                         WHERE ai.id = m.id
                     RETURNING ai.id,
                               m.fsm_state AS from_state,
                               ai.fsm_state AS to_state,
                               ai.owner,
                               ai.state_reason,
                               ai.next_retry_at,
                               ai.resurface_at,
                               ai.resolution_ref
                     ),
                     inserted AS (
                        INSERT INTO attention_state_history
                             (attention_id, from_state, to_state, owner, reason,
                              next_retry_at, resurface_at, source_ref)
                        SELECT id, from_state, to_state, owner, state_reason,
                               next_retry_at, resurface_at, resolution_ref
                          FROM updated
                     RETURNING 1
                     )
                  SELECT COUNT(*) FROM updated"#,
            )
            .bind(thesis_id)
            .bind(to.as_str())
            .execute(&mut *tx)
            .await
            .context("attention thesis_actionable resolve")?;
        }
        tx.commit().await.context("commit tx")?;
        Ok(())
    }

    /// Loads all theses for a symbol plus their version-history audit trail.
    /// Returns most-recently-updated first so the UI sees the latest thesis on
    /// top when there are multiple.
    async fn thesis_freshness_for_symbol(&self, symbol: &str) -> Result<ThesisFreshnessSummary> {
        let row = sqlx::query(
            r#"SELECT
                  (SELECT created_at
                     FROM ticker_context
                    WHERE symbol = $1
                 ORDER BY version DESC
                    LIMIT 1) AS context_at,
                  (SELECT max(snapshot_at)
                     FROM estimate_snapshot
                    WHERE symbol = $1) AS estimates_at,
                  (SELECT max(ingested_at)
                     FROM news_article
                    WHERE symbol = $1) AS news_at,
                  (SELECT count(*)
                     FROM news_article
                    WHERE symbol = $1
                      AND published_at >= now() - interval '14 days') AS recent_news_14d,
                  (SELECT max(COALESCE(last_success_at, last_started_at, updated_at))
                     FROM source_health
                    WHERE source IN ('fred', 'cboe')) AS market_at"#,
        )
        .bind(symbol)
        .fetch_one(&self.pool)
        .await
        .context("thesis freshness query")?;

        let now = Utc::now();
        let mut penalties = Vec::new();
        let mut components = Vec::new();

        let push = |components: &mut Vec<ThesisFreshnessComponent>,
                    penalties: &mut Vec<String>,
                    item: (ThesisFreshnessComponent, Option<String>)| {
            components.push(item.0);
            if let Some(penalty) = item.1 {
                penalties.push(penalty);
            }
        };

        push(
            &mut components,
            &mut penalties,
            age_component(
                "market",
                now,
                row.try_get("market_at").ok().flatten(),
                FreshnessThresholds {
                    fresh: ChronoDuration::hours(24),
                    stale: ChronoDuration::days(7),
                    old: ChronoDuration::days(30),
                },
                "market regime/crowd evidence is too old for high confidence",
            ),
        );
        push(
            &mut components,
            &mut penalties,
            age_component(
                "context",
                now,
                row.try_get("context_at").ok().flatten(),
                FreshnessThresholds {
                    fresh: ChronoDuration::days(7),
                    stale: ChronoDuration::days(30),
                    old: ChronoDuration::days(90),
                },
                "narrative context is stale",
            ),
        );
        push(
            &mut components,
            &mut penalties,
            age_component(
                "estimates",
                now,
                row.try_get("estimates_at").ok().flatten(),
                FreshnessThresholds {
                    fresh: ChronoDuration::days(14),
                    stale: ChronoDuration::days(60),
                    old: ChronoDuration::days(120),
                },
                "estimate-revision evidence is too old for actionable promotion",
            ),
        );
        push(
            &mut components,
            &mut penalties,
            news_component(
                row.try_get("recent_news_14d").unwrap_or(0),
                row.try_get("news_at").ok().flatten(),
            ),
        );

        let score = components
            .iter()
            .fold(1.0_f64, |acc, component| acc * component.score)
            .clamp(0.0, 1.0);
        let status = freshness_status(score);
        let confidence_cap = confidence_cap(score, &components);

        Ok(ThesisFreshnessSummary {
            score,
            status,
            confidence_cap,
            penalties,
            components,
        })
    }

    pub async fn theses_for_symbol(&self, symbol: &str) -> Result<Vec<ThesisDetail>> {
        let rows = sqlx::query(
            r#"SELECT thesis_id, symbol, cluster_id, cluster_thesis, state,
                      edge_rationale, bull_case, bear_case,
                      COALESCE(forecast, 'null'::jsonb)               AS forecast,
                      COALESCE(conviction_conditions, '[]'::jsonb)    AS conviction_conditions,
                      COALESCE(trigger_conditions, '[]'::jsonb)       AS trigger_conditions,
                      COALESCE(invalidation_conditions, '[]'::jsonb)  AS invalidation_conditions,
                      COALESCE(fulfillment_conditions, '[]'::jsonb)   AS fulfillment_conditions,
                      COALESCE(known_unknowns, '[]'::jsonb)           AS known_unknowns,
                      conviction_tier, system_confidence,
                      COALESCE(system_confidence_components, '{}'::jsonb) AS system_confidence_components,
                      instrument,
                      COALESCE(intended_size, 'null'::jsonb)          AS intended_size,
                      version,
                      COALESCE(immutable_original, '{}'::jsonb)       AS immutable_original,
                      created_at, updated_at, last_evaluated_at
                 FROM thesis
                WHERE symbol = $1
             ORDER BY updated_at DESC"#,
        )
        .bind(symbol)
        .fetch_all(&self.pool)
        .await
        .context("theses_for_symbol")?;

        let parent_themes_row = sqlx::query(
            r#"SELECT COALESCE(jsonb_agg(
                         jsonb_build_object(
                           'key', bt.key,
                           'name', bt.name,
                           'scope', bt.scope,
                           'state', bt.state,
                           'direction', bt.direction,
                           'role', btt.role,
                           'mapping_conviction', btt.conviction,
                           'conviction', brain_ticker_live_conviction(
                               btt.conviction,
                               latest.conviction_tier,
                               latest.system_confidence,
                               latest.forecast
                           ),
                           'rationale', btt.rationale,
                           'summary', bt.summary,
                           'core_claim', bt.core_claim,
                           'why_now', bt.why_now,
                           'thesis_state', latest.state,
                           'thesis_direction', latest.direction,
                           'thesis_conviction_tier', latest.conviction_tier,
                           'thesis_system_confidence', latest.system_confidence,
                           'thesis_updated_at', latest.updated_at,
                           'link_created_at', btt.created_at,
                           'link_stale', latest.updated_at IS NOT NULL
                               AND btt.created_at IS NOT NULL
                               AND latest.updated_at > btt.created_at
                         )
                         ORDER BY CASE bt.scope
                                    WHEN 'macro' THEN 0
                                    WHEN 'sector' THEN 1
                                    ELSE 2
                                  END,
                                  brain_ticker_live_conviction(
                                      btt.conviction,
                                      latest.conviction_tier,
                                      latest.system_confidence,
                                      latest.forecast
                                  ) DESC,
                                  bt.name
                       ), '[]'::jsonb) AS parent_themes
                  FROM brain_thesis_ticker btt
                  JOIN brain_thesis bt ON bt.id = btt.brain_thesis_id
             LEFT JOIN LATERAL (
                       SELECT th.state, th.forecast,
                              th.forecast->>'direction' AS direction,
                              th.conviction_tier, th.system_confidence, th.updated_at
                         FROM thesis th
                        WHERE th.symbol = btt.symbol
                          AND th.state NOT IN ('closed', 'disqualified')
                     ORDER BY th.updated_at DESC, th.created_at DESC
                        LIMIT 1
                  ) latest ON TRUE
                 WHERE btt.symbol = $1
                   AND bt.active = true"#,
        )
        .bind(symbol)
        .fetch_one(&self.pool)
        .await
        .context("thesis parent_themes")?;
        let parent_themes: serde_json::Value = parent_themes_row.try_get("parent_themes")?;

        let freshness = self.thesis_freshness_for_symbol(symbol).await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let thesis_id: uuid::Uuid = row.try_get("thesis_id")?;
            let state_s: String = row.try_get("state")?;
            let state = serde_json::from_value(serde_json::Value::String(state_s))
                .map_err(|e| anyhow::anyhow!("decode ThesisState: {e}"))?;

            // Version history for this thesis.
            let hist_rows = sqlx::query(
                r#"SELECT version, weakens_invalidation,
                          COALESCE(diff, '{}'::jsonb) AS diff,
                          rationale, at
                     FROM thesis_version_history
                    WHERE thesis_id = $1
                 ORDER BY version DESC, at DESC"#,
            )
            .bind(thesis_id)
            .fetch_all(&self.pool)
            .await
            .context("thesis_version_history")?;

            let history: Vec<ThesisVersionEvent> = hist_rows
                .into_iter()
                .map(|r| ThesisVersionEvent {
                    version: r.try_get("version").unwrap_or(0),
                    weakens_invalidation: r.try_get("weakens_invalidation").unwrap_or(false),
                    diff: r.try_get("diff").unwrap_or(serde_json::Value::Null),
                    rationale: r.try_get::<Option<String>, _>("rationale").unwrap_or(None),
                    at: r.try_get("at").unwrap_or_else(|_| chrono::Utc::now()),
                })
                .collect();

            let evidence_rows = sqlx::query(
                r#"SELECT ei.id, ei.symbol, ei.kind, ei.observed_at, ei.source,
                          ei.source_id, ei.source_ref, ei.summary, ei.strength,
                          ei.polarity, ei.url, ei.created_at,
                          te.weight, te.added_by
                     FROM thesis_evidence te
                     JOIN evidence_item ei ON ei.id = te.evidence_id
                    WHERE te.thesis_id = $1
                 ORDER BY te.weight DESC NULLS LAST, ei.observed_at DESC, ei.id DESC
                    LIMIT 25"#,
            )
            .bind(thesis_id)
            .fetch_all(&self.pool)
            .await
            .context("thesis_evidence")?;
            let evidence_items: Vec<serde_json::Value> = evidence_rows
                .into_iter()
                .map(|r| {
                    let observed_at: DateTime<Utc> = r.try_get("observed_at")?;
                    let created_at: DateTime<Utc> = r.try_get("created_at")?;
                    Ok(serde_json::json!({
                        "id": r.try_get::<i64, _>("id")?,
                        "symbol": r.try_get::<String, _>("symbol")?,
                        "kind": r.try_get::<String, _>("kind")?,
                        "observed_at": observed_at,
                        "source": r.try_get::<String, _>("source")?,
                        "source_id": r.try_get::<String, _>("source_id")?,
                        "source_ref": r.try_get::<serde_json::Value, _>("source_ref")?,
                        "summary": r.try_get::<String, _>("summary")?,
                        "strength": r.try_get::<Option<f64>, _>("strength")?,
                        "polarity": r.try_get::<Option<f64>, _>("polarity")?,
                        "url": r.try_get::<Option<String>, _>("url")?,
                        "created_at": created_at,
                        "weight": r.try_get::<Option<f64>, _>("weight")?,
                        "added_by": r.try_get::<String, _>("added_by")?,
                    }))
                })
                .collect::<Result<Vec<_>>>()?;

            let forecast: serde_json::Value = row.try_get("forecast")?;
            let conviction_conditions: serde_json::Value = row.try_get("conviction_conditions")?;
            let trigger_conditions: serde_json::Value = row.try_get("trigger_conditions")?;
            let invalidation_conditions: serde_json::Value =
                row.try_get("invalidation_conditions")?;
            let fulfillment_conditions: serde_json::Value =
                row.try_get("fulfillment_conditions")?;
            let known_unknowns: serde_json::Value = row.try_get("known_unknowns")?;
            let intended_size: serde_json::Value = row.try_get("intended_size")?;

            let parse_conds = |v: &serde_json::Value| -> Vec<Condition> {
                serde_json::from_value(v.clone()).unwrap_or_default()
            };
            let conviction = parse_conds(&conviction_conditions);
            let trigger = parse_conds(&trigger_conditions);
            let invalidation = parse_conds(&invalidation_conditions);
            let fulfillment = parse_conds(&fulfillment_conditions);

            // Substance is "present" when forecast/intended_size is a non-null
            // populated value. The thesis engine writes `null` for absent.
            let forecast_present = !forecast.is_null()
                && !matches!(&forecast, serde_json::Value::Object(o) if o.is_empty());
            let intended_size_present = !intended_size.is_null()
                && !matches!(&intended_size, serde_json::Value::Object(o) if o.is_empty());
            let sub_input = SubstanceInput {
                forecast_present,
                intended_size_present,
                conviction: conviction.clone(),
                trigger: trigger.clone(),
                invalidation: invalidation.clone(),
                fulfillment: fulfillment.clone(),
            };
            let wfc = sub_input.well_formed_counts();
            let report = substance::substance_report(&sub_input);
            let substance_summary = ThesisSubstance {
                score: report.score,
                max_score: report.max_score,
                missing: report.missing,
                blocked_at: report.blocked_at,
                well_formed: WellFormedCondCounts {
                    conviction: u32::try_from(wfc.conviction).unwrap_or(0),
                    trigger: u32::try_from(wfc.trigger).unwrap_or(0),
                    invalidation: u32::try_from(wfc.invalidation).unwrap_or(0),
                    fulfillment: u32::try_from(wfc.fulfillment).unwrap_or(0),
                },
                freshness_score: freshness.score,
                freshness_status: freshness.status.clone(),
                confidence_cap: freshness.confidence_cap.clone(),
                freshness_penalties: freshness.penalties.clone(),
                freshness_components: freshness.components.clone(),
            };

            out.push(ThesisDetail {
                thesis_id,
                symbol: row.try_get("symbol")?,
                cluster_id: row.try_get("cluster_id").ok(),
                cluster_thesis: row.try_get("cluster_thesis").ok(),
                parent_themes: parent_themes.clone(),
                state,
                edge_rationale: row.try_get("edge_rationale")?,
                bull_case: row.try_get("bull_case").ok(),
                bear_case: row.try_get("bear_case").ok(),
                forecast,
                conviction_conditions,
                trigger_conditions,
                invalidation_conditions,
                fulfillment_conditions,
                known_unknowns,
                conviction_tier: row.try_get("conviction_tier").ok(),
                system_confidence: row.try_get("system_confidence").ok(),
                system_confidence_components: row
                    .try_get("system_confidence_components")
                    .unwrap_or_else(|_| serde_json::json!({})),
                instrument: row.try_get("instrument").ok(),
                intended_size,
                version: row.try_get("version")?,
                immutable_original: row.try_get("immutable_original")?,
                created_at: row.try_get("created_at")?,
                updated_at: row.try_get("updated_at")?,
                last_evaluated_at: row.try_get("last_evaluated_at").ok(),
                history,
                evidence_items,
                substance: Some(substance_summary),
            });
        }
        Ok(out)
    }

    /// List pending discovery candidates with their LLM classification (if any).
    /// Used by the review UI in #54 phase B.
    pub async fn pending_discovery_candidates(&self) -> Result<Vec<serde_json::Value>> {
        // Dedupe by (symbol, signal_name) — show only the most recent proposed
        // candidate per signal. Schema allows multiple rows per (sym, sig)
        // because the same signal can re-fire on different days, but the
        // user only wants one entry in the review queue per pending signal.
        let rows = sqlx::query(
            r#"SELECT * FROM (
                  SELECT DISTINCT ON (dc.symbol, dc.signal_name)
                         dc.id, dc.symbol, dc.signal_name, dc.signal_value, dc.domain_fit,
                         dc.proposed_tier, dc.reasoning, dc.proposed_at,
                         COALESCE(dcl.proposed_lists, '[]'::jsonb) AS proposed_lists,
                         dcl.suggested_new_list,
                         COALESCE(parent.parent_themes, '[]'::jsonb) AS parent_themes,
                         parent.parent_theme_fit,
                         dislocation.dislocation_classification
                    FROM discovery_candidate dc
                    LEFT JOIN discovery_classification dcl ON dcl.candidate_id = dc.id
                    LEFT JOIN discovery_pool dp
                           ON dp.symbol = dc.symbol
                          AND dp.dropped_at IS NULL
                    LEFT JOIN LATERAL (
                        SELECT max(brain_ticker_live_conviction(
                                   btt.conviction,
                                   latest.conviction_tier,
                                   latest.system_confidence,
                                   latest.forecast
                               ))::double precision
                                  AS parent_theme_fit,
                               jsonb_agg(
                                   jsonb_build_object(
                                       'key', bt.key,
                                       'name', bt.name,
                                       'scope', bt.scope,
                                       'role', btt.role,
                                       'mapping_conviction', btt.conviction,
                                       'conviction', brain_ticker_live_conviction(
                                           btt.conviction,
                                           latest.conviction_tier,
                                           latest.system_confidence,
                                           latest.forecast
                                       ),
                                       'rationale', btt.rationale,
                                       'thesis_state', latest.state,
                                       'thesis_direction', latest.direction,
                                       'thesis_conviction_tier', latest.conviction_tier,
                                       'thesis_system_confidence', latest.system_confidence,
                                       'thesis_updated_at', latest.updated_at,
                                       'link_created_at', btt.created_at,
                                       'link_stale', latest.updated_at IS NOT NULL
                                           AND btt.created_at IS NOT NULL
                                           AND latest.updated_at > btt.created_at
                                   )
                                   ORDER BY brain_ticker_live_conviction(
                                           btt.conviction,
                                           latest.conviction_tier,
                                           latest.system_confidence,
                                           latest.forecast
                                       ) DESC,
                                       bt.name
                               ) AS parent_themes
                          FROM brain_thesis_ticker btt
                          JOIN brain_thesis bt ON bt.id = btt.brain_thesis_id
                     LEFT JOIN LATERAL (
                               SELECT th.state, th.forecast,
                                      th.forecast->>'direction' AS direction,
                                      th.conviction_tier, th.system_confidence, th.updated_at
                                 FROM thesis th
                                WHERE th.symbol = btt.symbol
                                  AND th.state NOT IN ('closed', 'disqualified')
                             ORDER BY th.updated_at DESC, th.created_at DESC
                                LIMIT 1
                          ) latest ON TRUE
                         WHERE btt.symbol = dc.symbol
                           AND bt.active = true
                           AND bt.scope IN ('sector', 'theme')
                    ) parent ON true
                    LEFT JOIN LATERAL (
                        SELECT bt.source_ref #>> ARRAY[
                                   'maintainer',
                                   'dislocation_map',
                                   'sector_classifications',
                                   COALESCE(dp.sector, ''),
                                   'classification'
                               ] AS dislocation_classification
                          FROM brain_thesis bt
                         WHERE bt.active = true
                           AND bt.scope = 'macro'
                      ORDER BY bt.updated_at DESC, bt.created_at DESC
                         LIMIT 1
                    ) dislocation ON true
                   WHERE dc.status = 'proposed'
                ORDER BY dc.symbol, dc.signal_name, dc.proposed_at DESC
               ) latest
            ORDER BY proposed_at DESC"#,
        )
        .fetch_all(&self.pool)
        .await
        .context("pending_discovery_candidates")?;
        let mut ranked = rows
            .into_iter()
            .map(|r| {
                let signal_value: Option<f64> = r.try_get("signal_value").ok();
                let proposed_lists: serde_json::Value = r.try_get("proposed_lists")?;
                let parent_themes: serde_json::Value = r.try_get("parent_themes")?;
                let parent_theme_fit: Option<f64> = r.try_get("parent_theme_fit").ok().flatten();
                let dislocation_classification: Option<String> =
                    r.try_get("dislocation_classification").ok().flatten();
                let suggested_new_list = r
                    .try_get::<Option<serde_json::Value>, _>("suggested_new_list")
                    .unwrap_or(None);
                let proposed_at: chrono::DateTime<chrono::Utc> = r.try_get("proposed_at")?;
                let rank = crate::discovery::ranking::rank_candidate(
                    &r.try_get::<String, _>("signal_name")?,
                    signal_value,
                    r.try_get::<Option<f64>, _>("domain_fit").ok().flatten(),
                    parent_theme_fit,
                    dislocation_classification.as_deref(),
                    r.try_get::<i32, _>("proposed_tier").unwrap_or(2),
                    &proposed_lists,
                    suggested_new_list.is_some(),
                );
                Ok((
                    rank.score,
                    proposed_at,
                    serde_json::json!({
                        "id": r.try_get::<i64, _>("id")?,
                        "symbol": r.try_get::<String, _>("symbol")?,
                        "signal_name": r.try_get::<String, _>("signal_name")?,
                        "signal_value": signal_value,
                        "domain_fit": r.try_get::<Option<f64>, _>("domain_fit").ok().flatten(),
                        "parent_theme_fit": parent_theme_fit,
                        "parent_themes": parent_themes,
                        "dislocation_classification": dislocation_classification,
                        "proposed_tier": r.try_get::<i32, _>("proposed_tier").unwrap_or(2),
                        "reasoning": r.try_get::<Option<String>, _>("reasoning").ok(),
                        "proposed_at": proposed_at,
                        "proposed_lists": proposed_lists,
                        "suggested_new_list": suggested_new_list,
                        "rank_score": rank.score,
                        "rank_bucket": rank.bucket,
                        "rank_reasons": rank.reasons,
                    }),
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        ranked.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.1.cmp(&a.1))
        });
        let mut research_nominations = 0usize;
        Ok(ranked
            .into_iter()
            .filter_map(|(_, _, value)| {
                if value.get("signal_name").and_then(serde_json::Value::as_str)
                    == Some("research_nomination")
                {
                    if research_nominations >= 100 {
                        return None;
                    }
                    research_nominations += 1;
                }
                Some(value)
            })
            .collect())
    }

    /// Confirm a candidate to one or more watchlists. Updates status, adds
    /// the symbol to each list (idempotent), records timestamp.
    pub async fn confirm_discovery_candidate(
        &self,
        candidate_id: i64,
        watchlist_ids: &[uuid::Uuid],
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.context("begin tx")?;
        let row = sqlx::query("SELECT symbol, signal_name FROM discovery_candidate WHERE id = $1")
            .bind(candidate_id)
            .fetch_one(&mut *tx)
            .await
            .context("load candidate")?;
        let symbol: String = row.try_get("symbol")?;
        let signal_name: String = row.try_get("signal_name")?;
        let added_by = format!("discovery:{signal_name}");
        // Ensure ticker exists (tier=2 default for fresh discoveries).
        sqlx::query("INSERT INTO ticker (symbol, tier) VALUES ($1, 2) ON CONFLICT DO NOTHING")
            .bind(&symbol)
            .execute(&mut *tx)
            .await?;
        for id in watchlist_ids {
            sqlx::query(
                r#"INSERT INTO watchlist_member (watchlist_id, symbol, added_by)
                   VALUES ($1, $2, $3) ON CONFLICT DO NOTHING"#,
            )
            .bind(id)
            .bind(&symbol)
            .bind(&added_by)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query(
            "UPDATE discovery_candidate SET status = 'confirmed', decided_at = now() WHERE id = $1",
        )
        .bind(candidate_id)
        .execute(&mut *tx)
        .await?;
        // Resolve the matching attention item (#86) inside the same tx so
        // queue + candidate status stay consistent.
        sqlx::query(
            r#"WITH matched AS (
                    SELECT id, fsm_state
                      FROM attention_item
                     WHERE status = 'open'
                       AND kind = 'candidate_review'
                       AND candidate_id = $1
                     FOR UPDATE
                 ),
                 updated AS (
                    UPDATE attention_item ai
                       SET status = 'resolved',
                           fsm_state = 'resolved',
                           owner = 'system',
                           resolved_at = now(),
                           resolution_kind = 'candidate_confirmed',
                           resolution_ref = jsonb_build_object('watchlist_ids', $2::text[]),
                           next_retry_at = NULL,
                           resurface_at = NULL,
                           state_reason = 'candidate_confirmed'
                      FROM matched m
                     WHERE ai.id = m.id
                 RETURNING ai.id,
                           m.fsm_state AS from_state,
                           ai.fsm_state AS to_state,
                           ai.owner,
                           ai.state_reason,
                           ai.next_retry_at,
                           ai.resurface_at,
                           ai.resolution_ref
                 ),
                 inserted AS (
                    INSERT INTO attention_state_history
                         (attention_id, from_state, to_state, owner, reason,
                          next_retry_at, resurface_at, source_ref)
                    SELECT id, from_state, to_state, owner, state_reason,
                           next_retry_at, resurface_at, resolution_ref
                      FROM updated
                 RETURNING 1
                 )
              SELECT COUNT(*) FROM updated"#,
        )
        .bind(candidate_id)
        .bind(
            watchlist_ids
                .iter()
                .map(uuid::Uuid::to_string)
                .collect::<Vec<_>>(),
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await.context("commit tx")?;
        Ok(())
    }

    pub async fn reject_discovery_candidate(&self, candidate_id: i64) -> Result<bool> {
        let mut tx = self.pool.begin().await.context("begin tx")?;
        let res = sqlx::query(
            "UPDATE discovery_candidate SET status = 'rejected', decided_at = now() WHERE id = $1",
        )
        .bind(candidate_id)
        .execute(&mut *tx)
        .await
        .context("reject_discovery_candidate")?;
        // Dismiss the matching attention item.
        sqlx::query(
            r#"WITH matched AS (
                    SELECT id, fsm_state
                      FROM attention_item
                     WHERE status = 'open'
                       AND kind = 'candidate_review'
                       AND candidate_id = $1
                     FOR UPDATE
                 ),
                 updated AS (
                    UPDATE attention_item ai
                       SET status = 'dismissed',
                           fsm_state = 'dismissed',
                           owner = 'operator',
                           resolved_at = now(),
                           resolution_kind = 'candidate_rejected',
                           resolution_ref = jsonb_build_object('reason', 'candidate_rejected'),
                           next_retry_at = NULL,
                           resurface_at = NULL,
                           state_reason = 'candidate_rejected'
                      FROM matched m
                     WHERE ai.id = m.id
                 RETURNING ai.id,
                           m.fsm_state AS from_state,
                           ai.fsm_state AS to_state,
                           ai.owner,
                           ai.state_reason,
                           ai.next_retry_at,
                           ai.resurface_at,
                           ai.resolution_ref
                 ),
                 inserted AS (
                    INSERT INTO attention_state_history
                         (attention_id, from_state, to_state, owner, reason,
                          next_retry_at, resurface_at, source_ref)
                    SELECT id, from_state, to_state, owner, state_reason,
                           next_retry_at, resurface_at, resolution_ref
                      FROM updated
                 RETURNING 1
                 )
              SELECT COUNT(*) FROM updated"#,
        )
        .bind(candidate_id)
        .execute(&mut *tx)
        .await?;
        tx.commit()
            .await
            .context("commit reject_discovery_candidate")?;
        Ok(res.rows_affected() > 0)
    }

    /// All watchlists with member counts (#54). Most-recent first; system
    /// lists rendered with a chip in the UI.
    pub async fn list_watchlists(&self) -> Result<Vec<Watchlist>> {
        let rows = sqlx::query(
            r#"SELECT w.id, w.name, w.description, w.color, w.is_system, w.created_at,
                      COUNT(m.symbol) AS member_count
                 FROM watchlist w
                 LEFT JOIN watchlist_member m ON m.watchlist_id = w.id
             GROUP BY w.id
             ORDER BY w.is_system DESC, w.name ASC"#,
        )
        .fetch_all(&self.pool)
        .await
        .context("list_watchlists")?;
        rows.into_iter()
            .map(|r| {
                Ok(Watchlist {
                    id: r.try_get("id")?,
                    name: r.try_get("name")?,
                    description: r.try_get("description").ok(),
                    color: r.try_get("color").ok(),
                    is_system: r.try_get("is_system")?,
                    created_at: r.try_get("created_at")?,
                    member_count: r.try_get::<i64, _>("member_count").unwrap_or(0),
                })
            })
            .collect()
    }

    /// Members of one watchlist (UI loads on click).
    pub async fn list_watchlist_members(&self, id: uuid::Uuid) -> Result<Vec<WatchlistMember>> {
        let rows = sqlx::query(
            r#"SELECT wm.watchlist_id,
                      wm.symbol,
                      wm.added_at,
                      wm.added_by,
                      latest.thesis_id AS latest_thesis_id,
                      latest.state AS thesis_state,
                      latest.direction AS thesis_direction,
                      tech.technical_state AS technical_state,
                      tech.entry_stance AS entry_stance,
                      tech.pct_vs_200d AS technical_pct_vs_200d,
                      freshness.status AS freshness_status,
                      COALESCE(attention.open_count, 0) AS open_attention,
                      COALESCE(attention.states, '[]'::jsonb) AS attention_states,
                      COALESCE(attention.owners, '[]'::jsonb) AS attention_owners,
                      COALESCE(evidence.open_count, 0) AS open_evidence,
                      COALESCE(evidence.blocking_count, 0) AS blocking_evidence,
                      COALESCE(tasks.due_count, 0) AS due_source_tasks,
                      COALESCE(brain.parent_themes, '[]'::jsonb) AS parent_themes,
                      (SELECT count(*) FROM thesis th
                        WHERE th.symbol = wm.symbol
                          AND th.state NOT IN ('closed','disqualified')) AS open_theses
                 FROM watchlist_member wm
            LEFT JOIN LATERAL (
                SELECT th.thesis_id, th.state, th.forecast->>'direction' AS direction,
                       th.forecast, th.conviction_tier, th.system_confidence,
                       th.updated_at,
                       COALESCE(th.last_evaluated_at, th.updated_at) AS evaluated_at
                  FROM thesis th
                 WHERE th.symbol = wm.symbol
                   AND th.state NOT IN ('closed','disqualified')
              ORDER BY th.updated_at DESC, th.created_at DESC
                 LIMIT 1
            ) latest ON TRUE
            LEFT JOIN LATERAL (
                WITH bars AS (
                    SELECT ts, close::float8 AS close, high::float8 AS high
                      FROM price_bar
                     WHERE symbol = wm.symbol
                  ORDER BY ts DESC
                     LIMIT 260
                ), ranked AS (
                    SELECT ts, close, high, row_number() OVER (ORDER BY ts DESC) AS rn
                      FROM bars
                ), latest_bar AS (
                    SELECT close
                      FROM ranked
                     WHERE rn = 1
                ), stats AS (
                    SELECT count(*) FILTER (WHERE rn <= 200) AS bars_200,
                           avg(close) FILTER (WHERE rn <= 50) AS sma50,
                           avg(close) FILTER (WHERE rn <= 200) AS sma200,
                           max(high) FILTER (WHERE rn <= 252) AS high252
                      FROM ranked
                ), classified AS (
                    SELECT CASE
                             WHEN stats.bars_200 < 200 OR stats.sma200 IS NULL THEN 'unknown'
                             WHEN ((latest_bar.close - stats.sma200) / NULLIF(stats.sma200, 0) * 100.0) > 20.0
                               OR ((latest_bar.close - stats.high252) / NULLIF(stats.high252, 0) * 100.0) >= -2.0 THEN 'extended'
                             WHEN ((latest_bar.close - stats.sma200) / NULLIF(stats.sma200, 0) * 100.0) < -5.0 THEN 'deteriorating'
                             WHEN stats.sma50 IS NOT NULL
                               AND abs((latest_bar.close - stats.sma50) / NULLIF(stats.sma50, 0) * 100.0) <= 5.0 THEN 'base_building'
                             WHEN ((latest_bar.close - stats.sma200) / NULLIF(stats.sma200, 0) * 100.0) >= 0.0 THEN 'constructive'
                             ELSE 'unknown'
                           END AS technical_state,
                           ((latest_bar.close - stats.sma200) / NULLIF(stats.sma200, 0) * 100.0)::float8 AS pct_vs_200d
                      FROM latest_bar CROSS JOIN stats
                )
                SELECT technical_state,
                       CASE technical_state
                         WHEN 'extended' THEN 'avoid_chase'
                         WHEN 'deteriorating' THEN 'avoid'
                         WHEN 'base_building' THEN 'wait_breakout'
                         WHEN 'constructive' THEN 'constructive'
                         ELSE 'wait_data'
                       END AS entry_stance,
                       pct_vs_200d
                  FROM classified
            ) tech ON TRUE
            LEFT JOIN LATERAL (
                SELECT tc.created_at AS context_at
                  FROM ticker_context tc
                 WHERE tc.symbol = wm.symbol
              ORDER BY tc.version DESC
                 LIMIT 1
            ) ctx ON TRUE
            LEFT JOIN LATERAL (
                SELECT count(*) AS rows_count,
                       count(*) FILTER (WHERE er.blocking_state <> 'satisfied') AS open_count,
                       count(*) FILTER (
                         WHERE er.priority = 'blocking'
                           AND er.blocking_state <> 'satisfied'
                       ) AS blocking_count
                  FROM evidence_requirement er
                 WHERE er.symbol = wm.symbol
            ) evidence ON TRUE
            LEFT JOIN LATERAL (
                SELECT count(*) FILTER (
                         WHERE st.state IN ('queued', 'no_rows', 'failed', 'rate_limited', 'blocked')
                           AND st.due_at <= now()
                       ) AS due_count,
                       count(*) FILTER (
                         WHERE st.state = 'fetching'
                           AND st.updated_at < now() - interval '30 minutes'
                       ) AS stale_fetching_count,
                       count(*) FILTER (
                         WHERE st.state IN ('failed', 'rate_limited', 'blocked')
                       ) AS blocked_count
                  FROM source_task st
                 WHERE st.scope = 'symbol'
                   AND st.target_id = wm.symbol
            ) tasks ON TRUE
            LEFT JOIN LATERAL (
                SELECT CASE
                         WHEN COALESCE(evidence.blocking_count, 0) > 0
                           OR COALESCE(tasks.blocked_count, 0) > 0 THEN 'blocked'
                         WHEN latest.thesis_id IS NULL
                           OR ctx.context_at IS NULL
                           OR COALESCE(evidence.rows_count, 0) = 0 THEN 'missing'
                         WHEN COALESCE(evidence.open_count, 0) > 0
                           OR COALESCE(tasks.due_count, 0) > 0
                           OR COALESCE(tasks.stale_fetching_count, 0) > 0
                           OR ctx.context_at < now() - interval '12 hours'
                           OR latest.evaluated_at IS NULL
                           OR latest.evaluated_at < now() - interval '30 minutes' THEN 'stale'
                         ELSE 'fresh'
                       END AS status
            ) freshness ON TRUE
            LEFT JOIN LATERAL (
                SELECT
                    (SELECT count(*)
                       FROM attention_item ai
                      WHERE ai.symbol = wm.symbol
                        AND ai.status = 'open'
                        AND (ai.fsm_state <> 'operator_deferred'
                             OR (ai.resurface_at IS NOT NULL AND ai.resurface_at <= now()))) AS open_count,
                    COALESCE((
                        SELECT jsonb_agg(jsonb_build_object('state', s.fsm_state, 'count', s.n)
                                         ORDER BY s.n DESC, s.fsm_state)
                          FROM (
                              SELECT ai.fsm_state, count(*) AS n
                                FROM attention_item ai
                               WHERE ai.symbol = wm.symbol
                                 AND ai.status = 'open'
                                 AND (ai.fsm_state <> 'operator_deferred'
                                      OR (ai.resurface_at IS NOT NULL AND ai.resurface_at <= now()))
                            GROUP BY ai.fsm_state
                          ) s
                    ), '[]'::jsonb) AS states,
                    COALESCE((
                        SELECT jsonb_agg(jsonb_build_object('owner', o.owner, 'count', o.n)
                                         ORDER BY o.n DESC, o.owner)
                          FROM (
                              SELECT ai.owner, count(*) AS n
                                FROM attention_item ai
                               WHERE ai.symbol = wm.symbol
                                 AND ai.status = 'open'
                                 AND (ai.fsm_state <> 'operator_deferred'
                                      OR (ai.resurface_at IS NOT NULL AND ai.resurface_at <= now()))
                            GROUP BY ai.owner
                          ) o
                    ), '[]'::jsonb) AS owners
            ) attention ON TRUE
            LEFT JOIN LATERAL (
                SELECT COALESCE(jsonb_agg(
                         jsonb_build_object(
                           'key', bt.key,
                           'name', bt.name,
                           'scope', bt.scope,
                           'state', bt.state,
                           'direction', bt.direction,
                           'role', btt.role,
                           'mapping_conviction', btt.conviction,
                           'conviction', brain_ticker_live_conviction(
                               btt.conviction,
                               latest.conviction_tier,
                               latest.system_confidence,
                               latest.forecast
                           ),
                           'thesis_state', latest.state,
                           'thesis_direction', latest.direction,
                           'thesis_conviction_tier', latest.conviction_tier,
                           'thesis_system_confidence', latest.system_confidence,
                           'thesis_updated_at', latest.updated_at,
                           'link_created_at', btt.created_at,
                           'link_stale', latest.updated_at IS NOT NULL
                               AND btt.created_at IS NOT NULL
                               AND latest.updated_at > btt.created_at
                         )
                         ORDER BY brain_ticker_live_conviction(
                               btt.conviction,
                               latest.conviction_tier,
                               latest.system_confidence,
                               latest.forecast
                           ) DESC, bt.name
                       ), '[]'::jsonb) AS parent_themes
                  FROM brain_thesis_ticker btt
                  JOIN brain_thesis bt ON bt.id = btt.brain_thesis_id
                 WHERE btt.symbol = wm.symbol
                   AND bt.active = true
            ) brain ON TRUE
                WHERE wm.watchlist_id = $1
             ORDER BY wm.added_at DESC"#,
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await
        .context("list_watchlist_members")?;
        rows.into_iter()
            .map(|r| {
                Ok(WatchlistMember {
                    watchlist_id: r.try_get("watchlist_id")?,
                    symbol: r.try_get("symbol")?,
                    added_at: r.try_get("added_at")?,
                    added_by: r.try_get("added_by").ok(),
                    latest_thesis_id: r.try_get("latest_thesis_id").ok(),
                    thesis_state: r.try_get("thesis_state").ok(),
                    thesis_direction: r.try_get("thesis_direction").ok(),
                    technical_state: r.try_get("technical_state").ok(),
                    entry_stance: r.try_get("entry_stance").ok(),
                    technical_pct_vs_200d: r.try_get("technical_pct_vs_200d").ok(),
                    open_theses: r.try_get::<i64, _>("open_theses").unwrap_or(0),
                    freshness_status: r
                        .try_get("freshness_status")
                        .unwrap_or_else(|_| "missing".to_string()),
                    open_attention: r.try_get::<i64, _>("open_attention").unwrap_or(0),
                    attention_states: r
                        .try_get("attention_states")
                        .unwrap_or_else(|_| serde_json::json!([])),
                    attention_owners: r
                        .try_get("attention_owners")
                        .unwrap_or_else(|_| serde_json::json!([])),
                    open_evidence: r.try_get::<i64, _>("open_evidence").unwrap_or(0),
                    blocking_evidence: r.try_get::<i64, _>("blocking_evidence").unwrap_or(0),
                    due_source_tasks: r.try_get::<i64, _>("due_source_tasks").unwrap_or(0),
                    parent_themes: r
                        .try_get("parent_themes")
                        .unwrap_or_else(|_| serde_json::json!([])),
                })
            })
            .collect()
    }

    pub async fn create_watchlist(
        &self,
        name: &str,
        description: Option<&str>,
        color: Option<&str>,
    ) -> Result<uuid::Uuid> {
        let row = sqlx::query(
            r#"INSERT INTO watchlist (name, description, color, is_system)
               VALUES ($1, $2, $3, false)
               RETURNING id"#,
        )
        .bind(name)
        .bind(description)
        .bind(color)
        .fetch_one(&self.pool)
        .await
        .context("create_watchlist")?;
        Ok(row.try_get("id")?)
    }

    /// Adds symbol to watchlist. Idempotent on (watchlist_id, symbol) PK;
    /// inserts the ticker row if it doesn't exist (default tier=2 — watch-only).
    pub async fn add_to_watchlist(
        &self,
        watchlist_id: uuid::Uuid,
        symbol: &str,
        added_by: &str,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.context("begin tx")?;
        // Ensure ticker exists; default tier=2 (watch-only) for fresh adds.
        sqlx::query("INSERT INTO ticker (symbol, tier) VALUES ($1, 2) ON CONFLICT DO NOTHING")
            .bind(symbol)
            .execute(&mut *tx)
            .await
            .context("ensure ticker row")?;
        sqlx::query(
            r#"INSERT INTO watchlist_member (watchlist_id, symbol, added_by)
               VALUES ($1, $2, $3) ON CONFLICT DO NOTHING"#,
        )
        .bind(watchlist_id)
        .bind(symbol)
        .bind(added_by)
        .execute(&mut *tx)
        .await
        .context("add_to_watchlist")?;
        tx.commit().await.context("commit tx")?;
        Ok(())
    }

    pub async fn promote_ticker(
        &self,
        symbol: &str,
        tier: i32,
        watchlist_ids: &[uuid::Uuid],
        added_by: &str,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.context("begin tx")?;
        sqlx::query(
            r#"INSERT INTO ticker (symbol, tier, status, last_promoted_at)
               VALUES ($1, $2, 'active', now())
               ON CONFLICT (symbol) DO UPDATE
                  SET status = 'active',
                      tier = LEAST(ticker.tier, EXCLUDED.tier),
                      last_promoted_at = now()"#,
        )
        .bind(symbol)
        .bind(tier)
        .execute(&mut *tx)
        .await
        .context("promote ticker")?;
        for id in watchlist_ids {
            sqlx::query(
                r#"INSERT INTO watchlist_member (watchlist_id, symbol, added_by)
                   VALUES ($1, $2, $3) ON CONFLICT DO NOTHING"#,
            )
            .bind(id)
            .bind(symbol)
            .bind(added_by)
            .execute(&mut *tx)
            .await
            .context("promote ticker watchlist member")?;
        }
        tx.commit().await.context("commit tx")?;
        Ok(())
    }

    pub async fn remove_from_watchlist(
        &self,
        watchlist_id: uuid::Uuid,
        symbol: &str,
    ) -> Result<bool> {
        let res =
            sqlx::query("DELETE FROM watchlist_member WHERE watchlist_id = $1 AND symbol = $2")
                .bind(watchlist_id)
                .bind(symbol)
                .execute(&self.pool)
                .await
                .context("remove_from_watchlist")?;
        Ok(res.rows_affected() > 0)
    }

    /// Delete a watchlist + its memberships. Refuses to drop system lists.
    pub async fn delete_watchlist(&self, id: uuid::Uuid) -> Result<bool> {
        let res = sqlx::query("DELETE FROM watchlist WHERE id = $1 AND is_system = false")
            .bind(id)
            .execute(&self.pool)
            .await
            .context("delete_watchlist")?;
        Ok(res.rows_affected() > 0)
    }

    /// Upsert a batch of price bars (#17). Primary key (symbol, ts) handles
    /// dedup; same-day re-polls overwrite (a later intraday bar replaces an
    /// earlier one with the same date).
    pub async fn upsert_price_bars(
        &self,
        rows: &[crate::ingest::massive::PriceBarRow],
    ) -> Result<usize> {
        if rows.is_empty() {
            return Ok(0);
        }
        let mut inserted = 0;
        let mut tx = self.pool.begin().await.context("begin tx")?;
        for r in rows {
            let res = sqlx::query(
                r#"INSERT INTO price_bar (symbol, ts, open, high, low, close, volume)
                   VALUES ($1, $2, $3, $4, $5, $6, $7)
                   ON CONFLICT (symbol, ts) DO UPDATE SET
                     open   = EXCLUDED.open,
                     high   = EXCLUDED.high,
                     low    = EXCLUDED.low,
                     close  = EXCLUDED.close,
                     volume = EXCLUDED.volume"#,
            )
            .bind(&r.symbol)
            .bind(r.ts)
            .bind(r.open)
            .bind(r.high)
            .bind(r.low)
            .bind(r.close)
            .bind(r.volume)
            .execute(&mut *tx)
            .await
            .context("upsert_price_bars")?;
            if res.rows_affected() > 0 {
                inserted += 1;
            }
        }
        tx.commit().await.context("commit tx")?;
        Ok(inserted)
    }

    pub async fn upsert_intraday_price_bars(
        &self,
        rows: &[crate::ingest::fmp_intraday::IntradayPriceBarRow],
    ) -> Result<usize> {
        if rows.is_empty() {
            return Ok(0);
        }
        let mut inserted = 0;
        let mut tx = self.pool.begin().await.context("begin tx")?;
        for r in rows {
            let res = sqlx::query(
                r#"INSERT INTO price_bar_intraday
                     (symbol, interval, ts, open, high, low, close, volume, source, fetched_at)
                   VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'fmp', now())
                   ON CONFLICT (symbol, interval, ts) DO UPDATE SET
                     open       = EXCLUDED.open,
                     high       = EXCLUDED.high,
                     low        = EXCLUDED.low,
                     close      = EXCLUDED.close,
                     volume     = EXCLUDED.volume,
                     fetched_at = now()"#,
            )
            .bind(&r.symbol)
            .bind(&r.interval)
            .bind(r.ts)
            .bind(r.open)
            .bind(r.high)
            .bind(r.low)
            .bind(r.close)
            .bind(r.volume)
            .execute(&mut *tx)
            .await
            .context("upsert_intraday_price_bars")?;
            if res.rows_affected() > 0 {
                inserted += 1;
            }
        }
        tx.commit().await.context("commit tx")?;
        Ok(inserted)
    }

    /// Upsert a batch of XBRL facts. Idempotent via the unique index on
    /// (symbol, taxonomy, concept, period_end, accession). Returns number
    /// of rows actually inserted (vs already-present).
    pub async fn upsert_company_facts(
        &self,
        rows: &[crate::ingest::xbrl::FactRow],
    ) -> Result<usize> {
        if rows.is_empty() {
            return Ok(0);
        }
        let mut inserted = 0;
        let mut tx = self.pool.begin().await.context("begin tx")?;
        for r in rows {
            let res = sqlx::query(
                r#"INSERT INTO company_fact
                     (symbol, cik, taxonomy, concept, period_end, period_start,
                      value, unit, form, fiscal_year, fiscal_period,
                      accession, filed_at)
                   VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
                   ON CONFLICT DO NOTHING"#,
            )
            .bind(&r.symbol)
            .bind(&r.cik)
            .bind(&r.taxonomy)
            .bind(&r.concept)
            .bind(r.period_end)
            .bind(r.period_start)
            .bind(r.value)
            .bind(&r.unit)
            .bind(r.form.as_deref())
            .bind(r.fiscal_year)
            .bind(r.fiscal_period.as_deref())
            .bind(r.accession.as_deref())
            .bind(r.filed_at)
            .execute(&mut *tx)
            .await
            .context("upsert_company_facts")?;
            if res.rows_affected() > 0 {
                inserted += 1;
            }
        }
        tx.commit().await.context("commit tx")?;
        Ok(inserted)
    }

    /// Records a single LLM call to the audit table (#6). Pair with
    /// `llm::prompts::invoke` — the recorder is wired via the trait impl
    /// below.
    pub async fn record_llm_invocation(
        &self,
        prompt_name: &str,
        prompt_hash: &str,
        provider: &str,
        model: &str,
        input_tokens: i32,
        output_tokens: i32,
        latency_ms: i32,
        request_summary: &str,
        response_summary: &str,
    ) -> Result<()> {
        sqlx::query(
            r#"INSERT INTO llm_invocation
                 (prompt_name, prompt_hash, provider, model,
                  input_tokens, output_tokens, latency_ms,
                  request_summary, response_summary)
               VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)"#,
        )
        .bind(prompt_name)
        .bind(prompt_hash)
        .bind(provider)
        .bind(model)
        .bind(input_tokens)
        .bind(output_tokens)
        .bind(latency_ms)
        .bind(request_summary)
        .bind(response_summary)
        .execute(&self.pool)
        .await
        .context("record_llm_invocation")?;
        Ok(())
    }

    /// Writes a regime classification row (SPEC §5.4). `as_of` is PK; conflicts
    /// overwrite. `config_version` is stored as text per schema typing.
    pub async fn upsert_market_state(
        &self,
        as_of: DateTime<Utc>,
        regime: &str,
        capitulation: bool,
        indicators: &serde_json::Value,
        config_version: i32,
    ) -> Result<()> {
        sqlx::query(
            r#"INSERT INTO market_state (as_of, regime, capitulation, indicators, config_version)
               VALUES ($1, $2, $3, $4, $5)
               ON CONFLICT (as_of) DO UPDATE SET
                 regime = EXCLUDED.regime,
                 capitulation = EXCLUDED.capitulation,
                 indicators = EXCLUDED.indicators,
                 config_version = EXCLUDED.config_version"#,
        )
        .bind(as_of)
        .bind(regime)
        .bind(capitulation)
        .bind(indicators)
        .bind(config_version.to_string())
        .execute(&self.pool)
        .await
        .context("upsert_market_state")?;
        Ok(())
    }
}

/// Decode one `alert` row into [`Alert`]. Shared by `recent_alerts` and
/// `recent_alerts_filtered`.
fn decode_alert(row: sqlx::postgres::PgRow) -> Result<Alert> {
    let kind_s: String = row.try_get("kind")?;
    let kind: AlertKind = serde_json::from_value(serde_json::Value::String(kind_s))
        .map_err(|e| anyhow::anyhow!("decode AlertKind: {e}"))?;
    Ok(Alert {
        id: row.try_get("id")?,
        thesis_id: row.try_get("thesis_id")?,
        symbol: row
            .try_get::<Option<String>, _>("symbol")?
            .unwrap_or_default(),
        kind,
        payload: row.try_get("payload")?,
        acknowledged: row.try_get("acknowledged")?,
        created_at: row.try_get("created_at")?,
    })
}

fn decode_price_alert_rule(row: sqlx::postgres::PgRow) -> Result<PriceAlertRule> {
    Ok(PriceAlertRule {
        id: row.try_get("id")?,
        symbol: row.try_get("symbol")?,
        thesis_id: row.try_get("thesis_id")?,
        origin: row.try_get("origin")?,
        intent: row.try_get("intent")?,
        direction: row.try_get("direction")?,
        target_price: row.try_get("target_price")?,
        label: row.try_get("label")?,
        rationale: row.try_get("rationale")?,
        semantic_key: row.try_get("semantic_key")?,
        status: row.try_get("status")?,
        source_ref: row.try_get("source_ref")?,
        expires_at: row.try_get("expires_at")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        triggered_at: row.try_get("triggered_at")?,
        disabled_at: row.try_get("disabled_at")?,
    })
}

fn decode_price_alert_event(row: sqlx::postgres::PgRow) -> Result<PriceAlertEvent> {
    Ok(PriceAlertEvent {
        id: row.try_get("id")?,
        rule_id: row.try_get("rule_id")?,
        symbol: row.try_get("symbol")?,
        thesis_id: row.try_get("thesis_id")?,
        triggered_at: row.try_get("triggered_at")?,
        trigger_ts: row.try_get("trigger_ts")?,
        trigger_interval: row.try_get("trigger_interval")?,
        trigger_price: row.try_get("trigger_price")?,
        rule_snapshot: row.try_get("rule_snapshot")?,
    })
}

#[async_trait::async_trait]
impl InvocationRecorder for Store {
    async fn record(&self, row: InvocationRow<'_>) -> Result<()> {
        // i32 cast is fine: token counts ≤ ~200k per call; latency ≤ ~10min.
        self.record_llm_invocation(
            row.prompt_name,
            row.prompt_hash,
            row.provider,
            row.model,
            i32::try_from(row.input_tokens).unwrap_or(i32::MAX),
            i32::try_from(row.output_tokens).unwrap_or(i32::MAX),
            i32::try_from(row.latency_ms).unwrap_or(i32::MAX),
            row.request_summary,
            row.response_summary,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn workflow_facts() -> SymbolWorkflowFacts {
        SymbolWorkflowFacts {
            symbol: "AMD".to_string(),
            active_tier: Some(2),
            context_version: Some(1),
            attention_items: serde_json::json!([]),
            ..Default::default()
        }
    }

    #[test]
    fn workflow_classifies_candidate_attention_as_nominated() {
        let mut facts = workflow_facts();
        facts.active_tier = None;
        facts.context_version = None;
        facts.latest_thesis_id = None;
        facts.open_attention_count = 1;
        facts.candidate_attention_id = Some(7);
        facts.attention_items = serde_json::json!([{
            "id": 7,
            "kind": "candidate_review",
            "reason": "2.4x volume vs SMA"
        }]);

        let decision = classify_symbol_workflow(&facts);

        assert_eq!(decision.state, "nominated");
        assert_eq!(decision.primary_kind, "attention");
        assert_eq!(decision.primary_label, "Promote to Universe");
        assert_eq!(decision.review_packet_attention_id, Some(7));
        assert_eq!(decision.reason, "2.4x volume vs SMA");
    }

    #[test]
    fn workflow_classifies_pool_candidate_before_active_work() {
        let mut facts = workflow_facts();
        facts.active_tier = None;
        facts.context_version = None;
        facts.in_pool = true;

        let decision = classify_symbol_workflow(&facts);

        assert_eq!(decision.state, "pool_candidate");
        assert_eq!(decision.primary_kind, "promote");
        assert_eq!(decision.state_label, "Pool candidate");
    }

    #[test]
    fn workflow_blocks_active_symbol_without_context() {
        let mut facts = workflow_facts();
        facts.context_version = None;

        let decision = classify_symbol_workflow(&facts);

        assert_eq!(decision.state, "context_missing");
        assert_eq!(decision.primary_kind, "research");
    }

    #[test]
    fn workflow_prioritizes_blocking_evidence() {
        let mut facts = workflow_facts();
        facts.blocking_evidence = 2;

        let decision = classify_symbol_workflow(&facts);

        assert_eq!(decision.state, "evidence_blocked");
        assert_eq!(decision.tone, "blocked");
        assert_eq!(decision.primary_kind, "research");
    }

    #[test]
    fn workflow_tracks_open_positions_before_decision_history() {
        let mut facts = workflow_facts();
        facts.open_position_count = 1;
        facts.decision_count = 2;

        let decision = classify_symbol_workflow(&facts);

        assert_eq!(decision.state, "position_tracking");
        assert_eq!(decision.primary_kind, "tracking");
    }

    #[test]
    fn workflow_flags_confirmed_decision_without_open_position_as_fill_needed() {
        let mut facts = workflow_facts();
        facts.decision_count = 1;
        facts.pending_manual_fill_count = 1;

        let decision = classify_symbol_workflow(&facts);

        assert_eq!(decision.state, "decision_recorded");
        assert_eq!(decision.state_label, "Fill needed");
        assert_eq!(decision.primary_kind, "decision");
    }

    #[test]
    fn workflow_distinguishes_actionable_and_monitoring_theses() {
        let mut actionable = workflow_facts();
        actionable.latest_thesis_id = Some(uuid::Uuid::nil());
        actionable.thesis_state = Some("armed".to_string());
        actionable.thesis_direction = Some("up".to_string());

        let decision = classify_symbol_workflow(&actionable);

        assert_eq!(decision.state, "thesis_actionable");
        assert_eq!(decision.primary_kind, "decision");

        let mut monitoring = workflow_facts();
        monitoring.latest_thesis_id = Some(uuid::Uuid::nil());
        monitoring.thesis_state = Some("forming".to_string());
        monitoring.thesis_direction = Some("up".to_string());

        let response = symbol_workflow_response(&monitoring);

        assert_eq!(response["state"], "thesis_monitoring");
        assert_eq!(response["primary_action"]["kind"], "thesis");
        assert_eq!(response["steps"][3]["value"], "forming · bull");
    }

    #[test]
    fn workflow_surfaces_automation_approval_state() {
        let mut facts = workflow_facts();
        facts.automation_permission_count = 1;
        facts.automation_approved_count = 1;

        let response = symbol_workflow_response(&facts);
        let automation_step = response["steps"]
            .as_array()
            .unwrap()
            .iter()
            .find(|step| step["key"] == "automation")
            .unwrap();

        assert_eq!(automation_step["label"], "Automation");
        assert_eq!(automation_step["value"], "1 approved");
        assert_eq!(automation_step["action"], "automation");
        assert_eq!(automation_step["tone"], "tracking");
    }

    #[test]
    fn workflow_surfaces_declines_and_context_ready_fallback() {
        let mut declined = workflow_facts();
        declined.decline_count = 1;
        declined.decline_reason = Some("No differentiated evidence.".to_string());

        let decision = classify_symbol_workflow(&declined);

        assert_eq!(decision.state, "declined");
        assert_eq!(decision.primary_kind, "retry_thesis");
        assert_eq!(decision.primary_label, "Retry thesis");
        assert_eq!(decision.reason, "No differentiated evidence.");

        let ready = workflow_facts();
        let decision = classify_symbol_workflow(&ready);

        assert_eq!(decision.state, "context_ready");
        assert_eq!(decision.primary_kind, "overview");
    }

    #[test]
    fn freshness_components_penalize_stale_context() {
        let now = Utc.with_ymd_and_hms(2026, 6, 2, 12, 0, 0).unwrap();
        let (component, penalty) = age_component(
            "context",
            now,
            Some(Utc.with_ymd_and_hms(2026, 4, 1, 12, 0, 0).unwrap()),
            FreshnessThresholds {
                fresh: ChronoDuration::days(7),
                stale: ChronoDuration::days(30),
                old: ChronoDuration::days(90),
            },
            "narrative context is stale",
        );

        assert_eq!(component.status, "stale");
        assert_eq!(component.score, 0.6);
        assert_eq!(
            penalty,
            Some("context: narrative context is stale".to_string())
        );
    }

    #[test]
    fn freshness_confidence_cap_blocks_sub_high_scores() {
        let components = vec![
            ThesisFreshnessComponent {
                name: "market".to_string(),
                status: "fresh".to_string(),
                score: 1.0,
                last_at: None,
                reason: "fresh".to_string(),
            },
            ThesisFreshnessComponent {
                name: "context".to_string(),
                status: "stale".to_string(),
                score: 0.6,
                last_at: None,
                reason: "stale".to_string(),
            },
        ];

        assert_eq!(freshness_status(0.60), "stale");
        assert_eq!(
            confidence_cap(0.60, &components),
            Some("medium".to_string())
        );
        assert_eq!(confidence_cap(0.40, &components), Some("low".to_string()));
    }

    #[test]
    fn news_component_penalizes_missing_recent_coverage() {
        let (component, penalty) = news_component(0, None);

        assert_eq!(component.status, "missing");
        assert_eq!(component.score, 0.5);
        assert_eq!(
            penalty,
            Some("news: cannot rely on sentiment-shift evidence".to_string())
        );
    }

    #[test]
    fn journal_categories_cover_attention_source_tasks_and_thesis_states() {
        assert_eq!(
            journal_attention_category("candidate_review", "review"),
            ("research", 70)
        );
        assert_eq!(
            journal_attention_category("thesis_actionable", "decision"),
            ("changed", 85)
        );
        assert_eq!(
            journal_attention_category("context_stale", "blocked"),
            ("blocked", 70)
        );

        assert_eq!(
            journal_source_task_category("satisfied", Some("rows_seen"), "high"),
            ("changed", 78)
        );
        assert_eq!(
            journal_source_task_category("rate_limited", None, "high"),
            ("blocked", 78)
        );
        assert_eq!(
            journal_source_task_category("queued", None, "blocking"),
            ("research", 62)
        );
        assert_eq!(
            journal_source_task_category("fetching", None, "high"),
            ("research", 62)
        );

        assert_eq!(journal_thesis_state_importance("actionable"), 90);
        assert_eq!(journal_thesis_state_importance("forming"), 65);
    }

    #[test]
    fn journal_candidate_score_separates_thesis_quality_from_entry_setup() {
        let clean = journal_candidate_score(
            Some("actionable"),
            Some("up"),
            Some("constructive"),
            Some("constructive"),
            Some("fresh"),
            1,
            Some(80.0),
        );
        let extended = journal_candidate_score(
            Some("actionable"),
            Some("up"),
            Some("extended"),
            Some("avoid_chase"),
            Some("fresh"),
            1,
            Some(80.0),
        );
        let blocked = journal_candidate_score(
            Some("actionable"),
            Some("up"),
            Some("constructive"),
            Some("constructive"),
            Some("blocked"),
            1,
            Some(80.0),
        );

        assert!(clean > extended);
        assert!(clean > blocked);
        assert!(journal_waits_for_setup(
            Some("extended"),
            Some("avoid_chase")
        ));
        assert!(journal_direction_is_bullish(Some("up")));
        assert!(journal_direction_is_bearish(Some("down")));
    }

    #[test]
    fn journal_trade_desk_item_links_to_review_packet() {
        let ticker = TickerRow {
            symbol: "CRDO".to_string(),
            cluster_id: "ai".to_string(),
            cluster_name: Some("AI infrastructure".to_string()),
            tier: 1,
            options_eligible: true,
            domain_fit: Some(84.0),
            added_at: Utc.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap(),
            open_theses: 1,
            latest_thesis_id: Some(
                uuid::Uuid::parse_str("00000000-0000-0000-0000-000000000111").unwrap(),
            ),
            thesis_state: Some("actionable".to_string()),
            thesis_direction: Some("up".to_string()),
            technical_state: Some("constructive".to_string()),
            technical_setup_kind: Some("constructive_trend".to_string()),
            entry_stance: Some("constructive".to_string()),
            technical_cross: serde_json::json!({
                "trend_state": "uptrend",
                "momentum_state": "positive",
                "relative_strength_state": "outperforming",
                "volume_state": "normal",
                "volatility_state": "normal",
                "buy_timing": "constructive"
            }),
            technical_pct_vs_200d: Some(3.2),
            freshness_status: "fresh".to_string(),
            open_attention: 1,
            review_packet_attention_id: Some(9102),
            attention_states: serde_json::json!([]),
            attention_owners: serde_json::json!([]),
            open_evidence: 0,
            blocking_evidence: 0,
            due_source_tasks: 0,
            parent_themes: serde_json::json!([]),
        };

        let item = journal_trade_desk_item(&ticker, 76, "consider");

        assert_eq!(item["review_packet_attention_id"], serde_json::json!(9102));
        assert_eq!(item["open_attention"], serde_json::json!(1));
        assert_eq!(
            item["technical_read"]["stance"],
            serde_json::json!("constructive")
        );
        assert_eq!(
            item["technical_panel_path"],
            serde_json::json!("/symbol/CRDO?p=technical")
        );
    }

    #[test]
    fn journal_technical_read_maps_pullback_reversal_to_starter_ok() {
        let read = journal_technical_read(
            Some("reversal_confirming"),
            Some("pullback_reversal"),
            Some("starter_ok"),
            &serde_json::json!({
                "trend_state": "pullback_in_uptrend",
                "momentum_state": "turning_up_from_oversold",
                "relative_strength_state": "outperforming",
                "volume_state": "accumulation",
                "volatility_state": "compression",
                "buy_timing": "pullback_reversal"
            }),
        );

        assert_eq!(read.stance, "starter_ok");
        assert!(read.reason.contains("trend pullback in uptrend"));
        assert!(read.reason.contains("momentum turning up from oversold"));
        assert!(read.reason.contains("relative strength outperforming"));
        assert!(read.reason.contains("volume accumulation"));
        assert!(read.reason.contains("volatility compression"));
    }

    #[test]
    fn journal_technical_read_maps_extended_chase_to_avoid_chase() {
        let read = journal_technical_read(
            Some("extended"),
            Some("extended_run"),
            Some("avoid_chase"),
            &serde_json::json!({
                "trend_state": "extended_chase",
                "momentum_state": "extended",
                "relative_strength_state": "outperforming",
                "volume_state": "normal",
                "volatility_state": "wide",
                "buy_timing": "avoid_chase"
            }),
        );

        assert_eq!(read.stance, "avoid_chase");
        assert!(read.reason.contains("trend extended chase"));
    }

    #[test]
    fn journal_technical_read_maps_breakdown_to_avoid_breakdown() {
        let read = journal_technical_read(
            Some("deteriorating"),
            Some("breakdown"),
            Some("avoid"),
            &serde_json::json!({
                "trend_state": "breakdown",
                "momentum_state": "weak",
                "relative_strength_state": "underperforming",
                "volume_state": "distribution",
                "volatility_state": "expansion",
                "buy_timing": "avoid_breakdown"
            }),
        );

        assert_eq!(read.stance, "avoid_breakdown");
        assert!(read.reason.contains("relative strength underperforming"));
    }
}
