//! Substance gates — the bullshit detector for thesis structural completeness
//! (#10). Pure functions; no I/O, no LLM. Consumed at state-machine promotion
//! time (#15) and surfaced in the UI per-thesis.
//!
//! The check is structural, not semantic: it doesn't ask "is this a good
//! thesis?" — it asks "did you fill in the slots that make it possible to
//! later answer that question?" A thesis with no forecast, no measurable
//! invalidation, and no intended size is bullshit by definition — there's
//! nothing for any downstream service to evaluate.

use serde::Serialize;

use crate::platform::domain::{Condition, ThesisState};

/// Result of the substance check.
#[derive(Debug, Clone, Serialize)]
pub struct SubstanceReport {
    /// Number of substance slots filled, out of [`SubstanceReport::MAX_SCORE`].
    pub score: u8,
    pub max_score: u8,
    /// Slot names that are missing or empty.
    pub missing: Vec<String>,
    /// First state this thesis cannot enter because of missing substance.
    /// `None` if all gates pass.
    pub blocked_at: Option<ThesisState>,
}

impl SubstanceReport {
    pub const MAX_SCORE: u8 = 6;

    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.blocked_at.is_none()
    }
}

/// Minimal subset of a thesis the substance check needs. The gateway can
/// build this from a [`crate::platform::domain::ThesisDetail`] via [`from_detail`].
#[derive(Debug, Clone, Default)]
pub struct Thesis {
    pub forecast_present: bool,
    pub intended_size_present: bool,
    pub conviction: Vec<Condition>,
    pub trigger: Vec<Condition>,
    pub invalidation: Vec<Condition>,
    pub fulfillment: Vec<Condition>,
}

impl Thesis {
    /// Returns the count of *well-formed* conditions (per
    /// [`Condition::is_well_formed`]) in each array — used by the UI to
    /// render "2 well-formed / 3 present" badges per condition slot.
    #[must_use]
    pub fn well_formed_counts(&self) -> WellFormedCounts {
        let count = |v: &[Condition]| v.iter().filter(|c| c.is_well_formed()).count();
        WellFormedCounts {
            conviction: count(&self.conviction),
            trigger: count(&self.trigger),
            invalidation: count(&self.invalidation),
            fulfillment: count(&self.fulfillment),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct WellFormedCounts {
    pub conviction: usize,
    pub trigger: usize,
    pub invalidation: usize,
    pub fulfillment: usize,
}

/// Compute the substance report for a thesis.
#[must_use]
pub fn substance_report(t: &Thesis) -> SubstanceReport {
    let mut missing = Vec::new();

    if !t.forecast_present {
        missing.push("forecast".to_string());
    }
    if !t.conviction.iter().any(Condition::is_well_formed) {
        missing.push("conviction_conditions".to_string());
    }
    if !t.trigger.iter().any(Condition::is_well_formed) {
        missing.push("trigger_conditions".to_string());
    }
    if !t.invalidation.iter().any(Condition::is_well_formed) {
        missing.push("invalidation_conditions".to_string());
    }
    if !t.intended_size_present {
        missing.push("intended_size".to_string());
    }
    if !t.fulfillment.iter().any(Condition::is_well_formed) {
        missing.push("fulfillment_conditions".to_string());
    }

    // First state this thesis cannot enter, walking the lifecycle in order.
    let blocked_at = if missing.iter().any(|m| m == "forecast" || m == "conviction_conditions") {
        Some(ThesisState::BuildingConviction)
    } else if missing
        .iter()
        .any(|m| m == "invalidation_conditions" || m == "trigger_conditions")
    {
        Some(ThesisState::Armed)
    } else if missing.iter().any(|m| m == "intended_size") {
        Some(ThesisState::Actionable)
    } else if missing.iter().any(|m| m == "fulfillment_conditions") {
        Some(ThesisState::Exiting)
    } else {
        None
    };

    let score = u8::try_from(SubstanceReport::MAX_SCORE as usize - missing.len())
        .unwrap_or(0);
    SubstanceReport {
        score,
        max_score: SubstanceReport::MAX_SCORE,
        missing,
        blocked_at,
    }
}

/// Whether a thesis can transition `from → to` right now. Returns `Err` with
/// either an "illegal transition" error or the list of missing substance
/// slots blocking it.
pub fn promotion_allowed(
    from: ThesisState,
    to: ThesisState,
    t: &Thesis,
) -> Result<(), Vec<String>> {
    use ThesisState::*;
    // Legal edges + required substance slots per SPEC §5.3 lifecycle.
    let (allowed, required): (bool, &[&str]) = match (from, to) {
        (Forming, BuildingConviction) => (true, &["forecast", "conviction_conditions"]),
        (BuildingConviction, Armed) => (true, &["invalidation_conditions", "trigger_conditions"]),
        (Armed, Actionable) => (true, &["intended_size"]),
        (Actionable, PositionOpen) => (true, &[]), // human decision; no further substance gate
        (PositionOpen, Exiting) => (true, &["fulfillment_conditions"]),
        (Exiting, Closed) => (true, &[]),
        // disqualified is the universal "kill" terminal
        (_, Disqualified) if from != Disqualified => (true, &[]),
        _ => (false, &[]),
    };
    if !allowed {
        return Err(vec![format!(
            "illegal transition {} → {}",
            from.as_str(),
            to.as_str()
        )]);
    }
    let report = substance_report(t);
    let missing: Vec<String> = required
        .iter()
        .filter(|f| report.missing.iter().any(|m| m == *f))
        .map(|f| (*f).to_string())
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(missing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    fn empty_thesis() -> Thesis {
        Thesis::default()
    }

    fn well_formed_condition(name: &str) -> Condition {
        Condition {
            name: name.into(),
            condition_type: "quantitative".into(),
            expr: Some(format!("{name} > 0")),
            assertion: None,
            target: Some(json!({"op":">","value":0})),
            deadline_at: Some(Utc::now()),
            evidence_source: Some("edgar:10-Q:TEST".into()),
            status: "pending".into(),
            last_checked_at: None,
            last_observed_value: None,
        }
    }

    fn vague_condition(name: &str) -> Condition {
        Condition {
            name: name.into(),
            condition_type: "quantitative".into(),
            expr: Some(format!("{name} > 0")),
            assertion: None,
            // missing target + deadline + evidence_source
            target: None,
            deadline_at: None,
            evidence_source: None,
            status: "pending".into(),
            last_checked_at: None,
            last_observed_value: None,
        }
    }

    #[test]
    fn empty_thesis_is_maximally_missing() {
        let r = substance_report(&empty_thesis());
        assert_eq!(r.score, 0);
        assert_eq!(r.missing.len(), 6);
        assert_eq!(r.blocked_at, Some(ThesisState::BuildingConviction));
        assert!(!r.is_complete());
    }

    #[test]
    fn vague_conditions_dont_count() {
        // Even with 5 vague conditions in every slot, score is just for forecast/size.
        let t = Thesis {
            forecast_present: true,
            intended_size_present: true,
            conviction: vec![vague_condition("c1")],
            trigger: vec![vague_condition("t1")],
            invalidation: vec![vague_condition("i1")],
            fulfillment: vec![vague_condition("f1")],
        };
        let r = substance_report(&t);
        assert_eq!(r.score, 2, "forecast + intended_size, none of the conditions count");
        assert!(r.missing.iter().any(|m| m == "conviction_conditions"));
        assert!(r.missing.iter().any(|m| m == "trigger_conditions"));
        assert!(r.missing.iter().any(|m| m == "invalidation_conditions"));
    }

    #[test]
    fn fully_substantiated_thesis_passes_all_gates() {
        let t = Thesis {
            forecast_present: true,
            intended_size_present: true,
            conviction: vec![well_formed_condition("c")],
            trigger: vec![well_formed_condition("t")],
            invalidation: vec![well_formed_condition("i")],
            fulfillment: vec![well_formed_condition("f")],
        };
        let r = substance_report(&t);
        assert_eq!(r.score, 6);
        assert!(r.missing.is_empty());
        assert_eq!(r.blocked_at, None);
        assert!(r.is_complete());
    }

    // ---------- promotion_allowed ----------

    #[test]
    fn forming_to_building_conviction_needs_forecast_plus_conviction() {
        let t = empty_thesis();
        let err = promotion_allowed(ThesisState::Forming, ThesisState::BuildingConviction, &t)
            .unwrap_err();
        assert!(err.iter().any(|m| m == "forecast"));
        assert!(err.iter().any(|m| m == "conviction_conditions"));
    }

    #[test]
    fn forming_to_building_conviction_succeeds_when_slots_filled() {
        let t = Thesis {
            forecast_present: true,
            conviction: vec![well_formed_condition("c")],
            ..Thesis::default()
        };
        promotion_allowed(ThesisState::Forming, ThesisState::BuildingConviction, &t).unwrap();
    }

    #[test]
    fn building_conviction_to_armed_needs_invalidation_and_trigger() {
        let t = Thesis {
            forecast_present: true,
            conviction: vec![well_formed_condition("c")],
            ..Thesis::default()
        };
        let err = promotion_allowed(
            ThesisState::BuildingConviction,
            ThesisState::Armed,
            &t,
        )
        .unwrap_err();
        assert!(err.iter().any(|m| m == "invalidation_conditions"));
        assert!(err.iter().any(|m| m == "trigger_conditions"));
    }

    #[test]
    fn armed_to_actionable_needs_intended_size() {
        let t = Thesis {
            forecast_present: true,
            conviction: vec![well_formed_condition("c")],
            trigger: vec![well_formed_condition("t")],
            invalidation: vec![well_formed_condition("i")],
            ..Thesis::default()
        };
        let err = promotion_allowed(ThesisState::Armed, ThesisState::Actionable, &t).unwrap_err();
        assert_eq!(err, vec!["intended_size"]);
    }

    #[test]
    fn position_open_to_exiting_needs_fulfillment() {
        let t = Thesis {
            forecast_present: true,
            intended_size_present: true,
            ..Thesis::default()
        };
        let err =
            promotion_allowed(ThesisState::PositionOpen, ThesisState::Exiting, &t).unwrap_err();
        assert!(err.iter().any(|m| m == "fulfillment_conditions"));
    }

    #[test]
    fn skipping_states_is_illegal() {
        let t = empty_thesis();
        let err =
            promotion_allowed(ThesisState::Forming, ThesisState::PositionOpen, &t).unwrap_err();
        assert!(err[0].contains("illegal transition"), "{err:?}");
    }

    #[test]
    fn disqualified_is_universal_kill_switch() {
        let t = empty_thesis();
        for from in [
            ThesisState::Forming,
            ThesisState::BuildingConviction,
            ThesisState::Armed,
            ThesisState::Actionable,
            ThesisState::PositionOpen,
            ThesisState::Exiting,
            ThesisState::Closed,
        ] {
            promotion_allowed(from, ThesisState::Disqualified, &t)
                .unwrap_or_else(|e| panic!("disqualify from {from:?} should always work: {e:?}"));
        }
    }

    #[test]
    fn no_self_transitions() {
        let t = empty_thesis();
        let err =
            promotion_allowed(ThesisState::Forming, ThesisState::Forming, &t).unwrap_err();
        assert!(err[0].contains("illegal transition"));
    }

    #[test]
    fn well_formed_counts_per_slot() {
        let t = Thesis {
            conviction: vec![well_formed_condition("a"), vague_condition("b")],
            trigger: vec![vague_condition("c")],
            ..Thesis::default()
        };
        let c = t.well_formed_counts();
        assert_eq!(c.conviction, 1, "1 of 2 well-formed");
        assert_eq!(c.trigger, 0);
    }
}
