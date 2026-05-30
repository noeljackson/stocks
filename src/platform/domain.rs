//! Core domain types — proper Rust sum types, not strings.
//!
//! Serde uses snake_case so the DB CHECK constraints (which are string-typed
//! in the schema) stay valid. The conversion enforces exhaustiveness at the
//! Rust boundary.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Per-thesis lifecycle state machine (SPEC §5.3). Maps to the DB
/// `thesis.state` text column via serde's snake_case rename.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThesisState {
    Forming,
    BuildingConviction,
    Armed,
    Actionable,
    PositionOpen,
    Exiting,
    Closed,
    Disqualified,
}

impl ThesisState {
    /// Returns the canonical string form used in the DB and on the wire.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Forming => "forming",
            Self::BuildingConviction => "building_conviction",
            Self::Armed => "armed",
            Self::Actionable => "actionable",
            Self::PositionOpen => "position_open",
            Self::Exiting => "exiting",
            Self::Closed => "closed",
            Self::Disqualified => "disqualified",
        }
    }
}

/// Macro regime classification (SPEC §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Regime {
    RiskOn,
    Neutral,
    RiskOff,
}

impl Regime {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RiskOn => "risk_on",
            Self::Neutral => "neutral",
            Self::RiskOff => "risk_off",
        }
    }
}

/// Alert kinds emitted to the UI feed (SPEC §3 FR7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertKind {
    StateTransition,
    Alignment,
    Consensus,
    Risk,
}

impl AlertKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::StateTransition => "state_transition",
            Self::Alignment => "alignment",
            Self::Consensus => "consensus",
            Self::Risk => "risk",
        }
    }
}

/// Latest market regime classification (SPEC §5.4). Surfaced via /api/regime.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketStateRow {
    pub as_of: DateTime<Utc>,
    pub regime: String,
    pub capitulation: bool,
    pub indicators: serde_json::Value,
}

/// Tracked-ticker summary for the UI sidebar.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TickerRow {
    pub symbol: String,
    pub cluster_id: String,
    pub cluster_name: Option<String>,
    pub tier: i32,
    pub options_eligible: bool,
    pub domain_fit: Option<f64>,
    pub added_at: DateTime<Utc>,
    pub open_theses: i64,
}

/// Per-ticker context row for the UI's drill-down panel (SPEC §5.2).
/// The market band is intentionally raw (not LLM-synthesized) and may be `{}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TickerContextRow {
    pub symbol: String,
    pub version: i32,
    pub structural: serde_json::Value,
    pub structural_as_of: Option<DateTime<Utc>>,
    pub narrative: serde_json::Value,
    pub narrative_as_of: Option<DateTime<Utc>>,
    pub market: serde_json::Value,
    pub market_as_of: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// Full thesis record + version-history audit trail for the UI detail panel.
/// The history lets the UI render goalpost-moved markers per revision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThesisDetail {
    pub thesis_id: Uuid,
    pub symbol: String,
    pub cluster_id: Option<String>,
    pub cluster_thesis: Option<String>,
    pub state: ThesisState,
    pub edge_rationale: String,
    pub bull_case: Option<String>,
    pub bear_case: Option<String>,
    pub forecast: serde_json::Value,
    pub conviction_conditions: serde_json::Value,
    pub trigger_conditions: serde_json::Value,
    pub invalidation_conditions: serde_json::Value,
    pub fulfillment_conditions: serde_json::Value,
    pub conviction_tier: Option<String>,
    pub instrument: Option<String>,
    pub intended_size: serde_json::Value,
    pub version: i32,
    pub immutable_original: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub history: Vec<ThesisVersionEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThesisVersionEvent {
    pub version: i32,
    pub weakens_invalidation: bool,
    pub diff: serde_json::Value,
    pub rationale: Option<String>,
    pub at: DateTime<Utc>,
}

/// Unit pushed onto the live SSE feed (SPEC §3 FR7). Mirrors the `alert`
/// table; payload is opaque JSON until the consumer parses by `kind`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alert {
    pub id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thesis_id: Option<Uuid>,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub symbol: String,
    pub kind: AlertKind,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub payload: serde_json::Value,
    pub acknowledged: bool,
    pub created_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn thesis_state_serde_round_trips() {
        for s in [
            ThesisState::Forming,
            ThesisState::BuildingConviction,
            ThesisState::PositionOpen,
            ThesisState::Disqualified,
        ] {
            let json = serde_json::to_string(&s).unwrap();
            // The serde form must match what we store in the DB text column.
            assert_eq!(json, format!("\"{}\"", s.as_str()));
            let back: ThesisState = serde_json::from_str(&json).unwrap();
            assert_eq!(back, s);
        }
    }

    #[test]
    fn regime_as_str_matches_db_check_constraint() {
        // Must match the CHECK constraint in db/migrations/0001_init.sql.
        assert_eq!(Regime::RiskOn.as_str(), "risk_on");
        assert_eq!(Regime::Neutral.as_str(), "neutral");
        assert_eq!(Regime::RiskOff.as_str(), "risk_off");
    }

    #[test]
    fn alert_kind_serde_uses_snake_case() {
        assert_eq!(
            serde_json::to_string(&AlertKind::StateTransition).unwrap(),
            "\"state_transition\""
        );
    }
}
