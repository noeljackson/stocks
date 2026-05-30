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
