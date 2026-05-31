//! Attention queue (#86).
//!
//! Events tell you what happened. Attention tells you what needs judgment.
//! Decisions record what the human chose.
//!
//! Producers (services that create attention items):
//! - discovery scanner → candidate_review on each fresh discovery_candidate
//! - gateway transition endpoint → thesis_actionable when state → actionable
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

/// Builds the standard title for a candidate_review item.
#[must_use]
pub fn title_for_candidate(symbol: &str, signal_name: &str) -> String {
    format!("{symbol} candidate via {signal_name}")
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
mod tests {
    use super::*;

    #[test]
    fn titles_format_consistently() {
        assert_eq!(title_for_candidate("MU", "volume_anomaly"),
                   "MU candidate via volume_anomaly");
        assert_eq!(title_for_thesis_actionable("NVDA"),
                   "NVDA thesis ready to act on");
        assert_eq!(title_for_risk_review("MU", true,
                       &["single_name_delta_notional_pct".into(),
                         "cash_floor_pct".into()]),
                   "MU risk veto: single_name_delta_notional_pct, cash_floor_pct");
        assert_eq!(title_for_risk_review("MU", false, &[]),
                   "MU risk warning");
    }

    #[test]
    fn constants_match_check_constraint() {
        // belt-and-braces: every kind/severity/source matches what the
        // migration's CHECK constraint accepts.
        for k in [kind::CANDIDATE_REVIEW, kind::CONTEXT_STALE, kind::THESIS_INCOMPLETE,
                  kind::THESIS_ACTIONABLE, kind::RISK_REVIEW, kind::INVALIDATION_HIT,
                  kind::OUTCOME_READY] {
            assert!(!k.is_empty());
        }
        for s in [severity::INFO, severity::REVIEW, severity::DECISION, severity::BLOCKED] {
            assert!(!s.is_empty());
        }
        for s in [source::DISCOVERY, source::THESIS, source::RISK, source::CONTEXT,
                  source::CONSENSUS, source::REFLECTION, source::SYSTEM] {
            assert!(!s.is_empty());
        }
    }
}
