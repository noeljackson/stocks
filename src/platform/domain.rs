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
    pub latest_thesis_id: Option<Uuid>,
    pub thesis_state: Option<String>,
    pub thesis_direction: Option<String>,
}

/// User-curated multi-list ticker organization (#54).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Watchlist {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub color: Option<String>,
    pub is_system: bool,
    pub created_at: DateTime<Utc>,
    /// Member count (denormalized into the response so the UI can render a
    /// chip without N+1 fetches).
    pub member_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchlistMember {
    pub watchlist_id: Uuid,
    pub symbol: String,
    pub added_at: DateTime<Utc>,
    pub added_by: Option<String>,
    pub latest_thesis_id: Option<Uuid>,
    pub thesis_state: Option<String>,
    pub thesis_direction: Option<String>,
    pub open_theses: i64,
}

/// A single thesis condition — the canonical shape used by the thesis engine,
/// goalpost detector, staleness service, and (future) condition evaluator.
///
/// The legacy minimal shape `{ name, type, expr | assertion }` is still
/// valid — `target`, `deadline_at`, `evidence_source`, `status`,
/// `last_checked_at`, `last_observed_value` are all optional and default at
/// the application layer. New conditions written by the thesis engine (#8)
/// after prompt-update (#9) include all six.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Condition {
    #[serde(default)]
    pub name: String,
    /// "quantitative" | "narrative"
    #[serde(default, rename = "type")]
    pub condition_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expr: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assertion: Option<String>,
    /// Measurable threshold or boolean. E.g.
    /// `{ "metric": "MU.HBM_revenue", "op": ">", "value": 1.2e9, "unit": "USD" }`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline_at: Option<DateTime<Utc>>,
    /// Where the answer comes from — URL, EDGAR form spec, FRED series, etc.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_source: Option<String>,
    /// `pending | satisfied | refuted | inconclusive | stale`. Defaults to `pending`.
    #[serde(default = "default_status")]
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_checked_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_observed_value: Option<serde_json::Value>,
}

fn default_status() -> String {
    "pending".to_string()
}

impl Condition {
    /// True if this condition has all three substance-gate slots filled
    /// (target + deadline_at + evidence_source). Used by #10 substance check.
    #[must_use]
    pub fn is_well_formed(&self) -> bool {
        self.target.is_some()
            && self.deadline_at.is_some()
            && self
                .evidence_source
                .as_deref()
                .is_some_and(|s| !s.is_empty())
    }
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
/// Structural completeness check attached to ThesisDetail responses (#10).
/// Frontend reads `substance.blocked_at` / `substance.missing` to render the
/// SKELETON banner and per-slot ✓/✗ checklist.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThesisSubstance {
    pub score: u8,
    pub max_score: u8,
    pub missing: Vec<String>,
    pub blocked_at: Option<ThesisState>,
    pub well_formed: WellFormedCondCounts,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct WellFormedCondCounts {
    pub conviction: u32,
    pub trigger: u32,
    pub invalidation: u32,
    pub fulfillment: u32,
}

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_evaluated_at: Option<DateTime<Utc>>,
    pub history: Vec<ThesisVersionEvent>,
    #[serde(default)]
    pub evidence_items: Vec<serde_json::Value>,
    /// Computed at read-time via [`crate::thesis::substance::substance_report`].
    /// Optional so callers that don't care (or older serialized rows) can
    /// omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub substance: Option<ThesisSubstance>,
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

    #[test]
    fn condition_legacy_shape_deserializes() {
        // Minimal { name, type, expr } shape from before #9 must still parse.
        let c: Condition = serde_json::from_str(
            r#"{ "name":"gm", "type":"quantitative", "expr":"gross_margin < 45" }"#,
        )
        .unwrap();
        assert_eq!(c.name, "gm");
        assert_eq!(c.condition_type, "quantitative");
        assert_eq!(c.expr.as_deref(), Some("gross_margin < 45"));
        assert_eq!(c.status, "pending"); // default
        assert!(!c.is_well_formed(), "legacy shape lacks the three slots");
    }

    #[test]
    fn condition_rich_shape_roundtrips() {
        let json = r#"{
          "name":"hbm4_q3_revenue",
          "type":"quantitative",
          "expr":"MU.HBM4_revenue > 1.2B",
          "target": {"metric":"MU.HBM4_revenue","op":">","value":1.2e9,"unit":"USD"},
          "deadline_at":"2026-07-30T00:00:00Z",
          "evidence_source":"edgar:10-Q:MU",
          "status":"pending"
        }"#;
        let c: Condition = serde_json::from_str(json).unwrap();
        assert!(c.is_well_formed(), "all three slots filled");
        // Round-trip back to JSON without losing fields.
        let back = serde_json::to_value(&c).unwrap();
        assert_eq!(back["target"]["op"], ">");
        assert_eq!(back["evidence_source"], "edgar:10-Q:MU");
    }

    #[test]
    fn condition_well_formed_requires_all_three() {
        let mut c = Condition {
            name: "x".into(),
            condition_type: "quantitative".into(),
            expr: Some("x > 1".into()),
            assertion: None,
            target: Some(serde_json::json!({"op":">", "value": 1})),
            deadline_at: None,
            evidence_source: Some("fred:DGS10".into()),
            status: "pending".into(),
            last_checked_at: None,
            last_observed_value: None,
        };
        assert!(!c.is_well_formed(), "missing deadline_at");
        c.deadline_at = Some(Utc::now());
        assert!(c.is_well_formed());
        c.evidence_source = Some(String::new());
        assert!(!c.is_well_formed(), "empty evidence_source doesn't count");
    }
}
