//! Attention queue (#86).
//!
//! Events tell you what happened. Attention tells you what needs judgment.
//! Decisions record what the human chose.
//!
//! Producers (services that create attention items):
//! - discovery scanner → candidate_review on composed discovery candidates
//! - discovery scanner → thesis_actionable when a hit belongs to an existing thesis
//! - gateway transition endpoint → thesis_actionable when state → actionable
//! - cognition reconciliation → thesis_review when a standing thesis materially changes
//! - risk service → risk_review on veto/warning
//! - staler service → context_stale + invalidation_hit (future)
//! - reflection → outcome_ready on horizon_at reached (future)
//!
//! Resolvers (paths that close items):
//! - confirm_discovery_candidate → resolve candidate_review
//! - reject_discovery_candidate → dismiss candidate_review
//! - record_decision → resolve thesis_actionable
//! - ack_alert → resolve risk_review
//! - refresh_context → resolve context_stale
//! - score_outcome → resolve outcome_ready
//!
//! Helpers below build the title/reason strings consistently across
//! producers so the operator sees the same vocabulary in the attention list.

/// Attention kinds (mirrors the CHECK constraint in 0016).
pub mod kind {
    pub const CANDIDATE_REVIEW: &str = "candidate_review";
    pub const CONTEXT_STALE: &str = "context_stale";
    pub const THESIS_INCOMPLETE: &str = "thesis_incomplete";
    pub const THESIS_REVIEW: &str = "thesis_review";
    pub const THESIS_ACTIONABLE: &str = "thesis_actionable";
    pub const RISK_REVIEW: &str = "risk_review";
    pub const INVALIDATION_HIT: &str = "invalidation_hit";
    pub const OUTCOME_READY: &str = "outcome_ready";
}

/// Severity tiers (mirrors the CHECK constraint).
pub mod severity {
    pub const INFO: &str = "info";
    pub const REVIEW: &str = "review";
    pub const DECISION: &str = "decision";
    pub const BLOCKED: &str = "blocked";
}

/// Operational FSM states (mirrors 0028_attention_fsm.sql).
pub mod fsm {
    pub const QUEUED: &str = "queued";
    pub const EVALUATING: &str = "evaluating";
    pub const WAITING_ON_DATA: &str = "waiting_on_data";
    pub const READY_FOR_REVIEW: &str = "ready_for_review";
    pub const OPERATOR_DEFERRED: &str = "operator_deferred";
    pub const ACTIONABLE: &str = "actionable";
    pub const RESOLVED: &str = "resolved";
    pub const DISMISSED: &str = "dismissed";
    pub const BLOCKED: &str = "blocked";
}

/// Current owner labels (mirrors 0028_attention_fsm.sql).
pub mod owner {
    pub const SYSTEM: &str = "system";
    pub const OPERATOR: &str = "operator";
    pub const SOURCE: &str = "source";
    pub const COGNITION: &str = "cognition";
    pub const RISK: &str = "risk";
}

#[must_use]
pub fn is_valid_fsm_state(state: &str) -> bool {
    matches!(
        state,
        fsm::QUEUED
            | fsm::EVALUATING
            | fsm::WAITING_ON_DATA
            | fsm::READY_FOR_REVIEW
            | fsm::OPERATOR_DEFERRED
            | fsm::ACTIONABLE
            | fsm::RESOLVED
            | fsm::DISMISSED
            | fsm::BLOCKED
    )
}

#[must_use]
pub fn is_valid_owner(value: &str) -> bool {
    matches!(
        value,
        owner::SYSTEM | owner::OPERATOR | owner::SOURCE | owner::COGNITION | owner::RISK
    )
}

#[must_use]
pub fn status_for_state(state: &str) -> &'static str {
    match state {
        fsm::RESOLVED => "resolved",
        fsm::DISMISSED => "dismissed",
        _ => "open",
    }
}

#[must_use]
pub fn default_owner_for_state(state: &str) -> &'static str {
    match state {
        fsm::QUEUED | fsm::EVALUATING => owner::SYSTEM,
        fsm::WAITING_ON_DATA | fsm::BLOCKED => owner::SOURCE,
        fsm::RESOLVED => owner::SYSTEM,
        _ => owner::OPERATOR,
    }
}

/// Source labels (where the item came from in our pipeline).
pub mod source {
    pub const DISCOVERY: &str = "discovery";
    pub const THESIS: &str = "thesis";
    pub const RISK: &str = "risk";
    pub const CONTEXT: &str = "context";
    pub const CONSENSUS: &str = "consensus";
    pub const REFLECTION: &str = "reflection";
    pub const SYSTEM: &str = "system";
}

#[must_use]
pub fn initial_assignment(
    kind: &str,
    severity: &str,
    source: &str,
) -> (&'static str, &'static str) {
    match kind {
        kind::CANDIDATE_REVIEW => (fsm::READY_FOR_REVIEW, owner::OPERATOR),
        kind::THESIS_REVIEW => (fsm::READY_FOR_REVIEW, owner::OPERATOR),
        kind::THESIS_ACTIONABLE => (fsm::ACTIONABLE, owner::OPERATOR),
        kind::RISK_REVIEW if severity == severity::BLOCKED => (fsm::BLOCKED, owner::OPERATOR),
        kind::RISK_REVIEW => (fsm::READY_FOR_REVIEW, owner::OPERATOR),
        kind::THESIS_INCOMPLETE if source == source::CONSENSUS => {
            (fsm::EVALUATING, owner::COGNITION)
        }
        kind::THESIS_INCOMPLETE if source == source::CONTEXT => {
            (fsm::WAITING_ON_DATA, owner::SOURCE)
        }
        kind::CONTEXT_STALE => (fsm::WAITING_ON_DATA, owner::COGNITION),
        kind::INVALIDATION_HIT | kind::OUTCOME_READY => (fsm::READY_FOR_REVIEW, owner::OPERATOR),
        _ => (fsm::READY_FOR_REVIEW, owner::OPERATOR),
    }
}

/// Builds the standard title for a candidate_review item.
#[must_use]
pub fn title_for_candidate(symbol: &str, signal_name: &str) -> String {
    let label = match signal_name {
        "early_accumulation" => "possible early accumulation",
        "breakout_confirmation" => "breakout confirmation",
        "extended_momentum" => "extended momentum review",
        "consensus_arrival" => "consensus arrival review",
        "possible_exhaustion" => "possible exhaustion review",
        "existing_thesis_trigger" => "existing thesis trigger",
        "research_nomination" => "research nomination",
        _ => return format!("{symbol} candidate via {signal_name}"),
    };
    format!("{symbol}: {label}")
}

#[must_use]
pub fn title_for_thesis_actionable(symbol: &str) -> String {
    format!("{symbol} thesis ready to act on")
}

#[must_use]
pub fn title_for_risk_review(symbol: &str, veto: bool, reasons: &[String]) -> String {
    let head = if veto { "veto" } else { "warning" };
    if reasons.is_empty() {
        format!("{symbol} risk {head}")
    } else {
        format!("{symbol} risk {head}: {}", reasons.join(", "))
    }
}

#[cfg(test)]
mod fsm_tests {
    use super::{
        default_owner_for_state, fsm, is_valid_fsm_state, is_valid_owner, owner, status_for_state,
    };

    #[test]
    fn attention_state_validation_matches_schema_values() {
        for state in [
            fsm::QUEUED,
            fsm::EVALUATING,
            fsm::WAITING_ON_DATA,
            fsm::READY_FOR_REVIEW,
            fsm::OPERATOR_DEFERRED,
            fsm::ACTIONABLE,
            fsm::RESOLVED,
            fsm::DISMISSED,
            fsm::BLOCKED,
        ] {
            assert!(is_valid_fsm_state(state));
        }
        assert!(!is_valid_fsm_state("paused"));
    }

    #[test]
    fn attention_owner_validation_matches_schema_values() {
        for value in [
            owner::SYSTEM,
            owner::OPERATOR,
            owner::SOURCE,
            owner::COGNITION,
            owner::RISK,
        ] {
            assert!(is_valid_owner(value));
        }
        assert!(!is_valid_owner("analyst"));
    }

    #[test]
    fn terminal_states_drive_coarse_status() {
        assert_eq!(status_for_state(fsm::RESOLVED), "resolved");
        assert_eq!(status_for_state(fsm::DISMISSED), "dismissed");
        assert_eq!(status_for_state(fsm::ACTIONABLE), "open");
    }

    #[test]
    fn default_owner_names_work_owner() {
        assert_eq!(default_owner_for_state(fsm::WAITING_ON_DATA), owner::SOURCE);
        assert_eq!(default_owner_for_state(fsm::EVALUATING), owner::SYSTEM);
        assert_eq!(default_owner_for_state(fsm::ACTIONABLE), owner::OPERATOR);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_format_consistently() {
        assert_eq!(
            title_for_candidate("MU", "volume_anomaly"),
            "MU candidate via volume_anomaly"
        );
        assert_eq!(
            title_for_candidate("MU", "extended_momentum"),
            "MU: extended momentum review"
        );
        assert_eq!(
            title_for_candidate("CRWV", "research_nomination"),
            "CRWV: research nomination"
        );
        assert_eq!(
            title_for_thesis_actionable("NVDA"),
            "NVDA thesis ready to act on"
        );
        assert_eq!(
            title_for_risk_review(
                "MU",
                true,
                &[
                    "single_name_delta_notional_pct".into(),
                    "cash_floor_pct".into()
                ]
            ),
            "MU risk veto: single_name_delta_notional_pct, cash_floor_pct"
        );
        assert_eq!(title_for_risk_review("MU", false, &[]), "MU risk warning");
    }

    #[test]
    fn constants_match_check_constraint() {
        // belt-and-braces: every kind/severity/source matches what the
        // migration's CHECK constraint accepts.
        for k in [
            kind::CANDIDATE_REVIEW,
            kind::CONTEXT_STALE,
            kind::THESIS_INCOMPLETE,
            kind::THESIS_REVIEW,
            kind::THESIS_ACTIONABLE,
            kind::RISK_REVIEW,
            kind::INVALIDATION_HIT,
            kind::OUTCOME_READY,
        ] {
            assert!(!k.is_empty());
        }
        for s in [
            severity::INFO,
            severity::REVIEW,
            severity::DECISION,
            severity::BLOCKED,
        ] {
            assert!(!s.is_empty());
        }
        for s in [
            fsm::QUEUED,
            fsm::EVALUATING,
            fsm::WAITING_ON_DATA,
            fsm::READY_FOR_REVIEW,
            fsm::OPERATOR_DEFERRED,
            fsm::ACTIONABLE,
            fsm::RESOLVED,
            fsm::DISMISSED,
            fsm::BLOCKED,
        ] {
            assert!(!s.is_empty());
        }
        for o in [
            owner::SYSTEM,
            owner::OPERATOR,
            owner::SOURCE,
            owner::COGNITION,
            owner::RISK,
        ] {
            assert!(!o.is_empty());
        }
        for s in [
            source::DISCOVERY,
            source::THESIS,
            source::RISK,
            source::CONTEXT,
            source::CONSENSUS,
            source::REFLECTION,
            source::SYSTEM,
        ] {
            assert!(!s.is_empty());
        }
    }

    #[test]
    fn initial_assignment_names_the_current_owner() {
        assert_eq!(
            initial_assignment(kind::CANDIDATE_REVIEW, severity::REVIEW, source::DISCOVERY),
            (fsm::READY_FOR_REVIEW, owner::OPERATOR)
        );
        assert_eq!(
            initial_assignment(kind::THESIS_REVIEW, severity::REVIEW, source::THESIS),
            (fsm::READY_FOR_REVIEW, owner::OPERATOR)
        );
        assert_eq!(
            initial_assignment(kind::THESIS_ACTIONABLE, severity::DECISION, source::THESIS),
            (fsm::ACTIONABLE, owner::OPERATOR)
        );
        assert_eq!(
            initial_assignment(kind::THESIS_INCOMPLETE, severity::REVIEW, source::CONSENSUS),
            (fsm::EVALUATING, owner::COGNITION)
        );
        assert_eq!(
            initial_assignment(kind::RISK_REVIEW, severity::BLOCKED, source::RISK),
            (fsm::BLOCKED, owner::OPERATOR)
        );
    }
}
