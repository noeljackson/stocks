use chrono::{DateTime, Duration, Utc};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    Fresh,
    Stale,
    Missing,
}

impl Freshness {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fresh => "fresh",
            Self::Stale => "stale",
            Self::Missing => "missing",
        }
    }
}

#[must_use]
pub fn age_freshness(
    now: DateTime<Utc>,
    last_at: Option<DateTime<Utc>>,
    max_age: Duration,
) -> Freshness {
    let Some(last_at) = last_at else {
        return Freshness::Missing;
    };
    if now.signed_duration_since(last_at) <= max_age {
        Freshness::Fresh
    } else {
        Freshness::Stale
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BrainDecisionInput {
    pub evidence_rows: i64,
    pub open_evidence: i64,
    pub blocking_evidence: i64,
    pub due_evidence: i64,
    pub has_context: bool,
    pub context_stale: bool,
    pub has_open_thesis: bool,
    pub thesis_stale: bool,
    pub any_source_stale: bool,
    pub source_blocked: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BrainDecision {
    pub status: &'static str,
    pub next_action: &'static str,
    pub reason: &'static str,
}

#[must_use]
pub fn decide(input: BrainDecisionInput) -> BrainDecision {
    if input.source_blocked {
        return BrainDecision {
            status: "blocked",
            next_action: "wait_for_source_retry",
            reason: "a required source is rate-limited or failing",
        };
    }
    if input.evidence_rows == 0 {
        return BrainDecision {
            status: "due",
            next_action: "initialize_evidence",
            reason: "evidence checklist has not been initialized",
        };
    }
    if input.blocking_evidence > 0 {
        return BrainDecision {
            status: "waiting_on_evidence",
            next_action: "fetch_blocking_evidence",
            reason: "blocking evidence is missing",
        };
    }
    if !input.has_context {
        return BrainDecision {
            status: "due",
            next_action: "refresh_context",
            reason: "ticker context has not been synthesized",
        };
    }
    if input.due_evidence > 0 {
        return BrainDecision {
            status: "due",
            next_action: "retry_evidence",
            reason: "missing evidence is due for retry",
        };
    }
    if !input.has_open_thesis && input.open_evidence > 0 {
        return BrainDecision {
            status: "waiting_on_evidence",
            next_action: "wait_for_evidence_retry",
            reason: "non-blocking evidence is still missing before another thesis attempt",
        };
    }
    if !input.has_open_thesis {
        return BrainDecision {
            status: "due",
            next_action: "draft_or_decline_thesis",
            reason: "symbol has no current standing thesis",
        };
    }
    if input.thesis_stale {
        return BrainDecision {
            status: "due",
            next_action: "reevaluate_thesis",
            reason: "open thesis is past the re-evaluation window",
        };
    }
    if input.context_stale {
        return BrainDecision {
            status: "stale",
            next_action: "refresh_context",
            reason: "ticker context is stale",
        };
    }
    if input.open_evidence > 0 {
        return BrainDecision {
            status: "waiting_on_evidence",
            next_action: "wait_for_evidence_retry",
            reason: "non-blocking evidence is still missing",
        };
    }
    if input.any_source_stale {
        return BrainDecision {
            status: "stale",
            next_action: "refresh_sources",
            reason: "one or more data sources are past the freshness target",
        };
    }
    BrainDecision {
        status: "fresh",
        next_action: "monitor",
        reason: "brain loop is current for this symbol",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn age_freshness_handles_missing_stale_and_fresh() {
        let now = Utc.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap();
        assert_eq!(
            age_freshness(now, None, Duration::minutes(30)),
            Freshness::Missing
        );
        assert_eq!(
            age_freshness(
                now,
                Some(now - Duration::minutes(31)),
                Duration::minutes(30)
            ),
            Freshness::Stale
        );
        assert_eq!(
            age_freshness(
                now,
                Some(now - Duration::minutes(30)),
                Duration::minutes(30)
            ),
            Freshness::Fresh
        );
    }

    #[test]
    fn decision_prioritizes_source_blocks() {
        let got = decide(BrainDecisionInput {
            source_blocked: true,
            evidence_rows: 0,
            open_evidence: 0,
            blocking_evidence: 0,
            due_evidence: 0,
            has_context: false,
            context_stale: false,
            has_open_thesis: false,
            thesis_stale: false,
            any_source_stale: false,
        });

        assert_eq!(got.status, "blocked");
        assert_eq!(got.next_action, "wait_for_source_retry");
    }

    #[test]
    fn decision_asks_for_open_thesis_update_before_context_staleness() {
        let got = decide(BrainDecisionInput {
            source_blocked: false,
            evidence_rows: 4,
            open_evidence: 0,
            blocking_evidence: 0,
            due_evidence: 0,
            has_context: true,
            context_stale: true,
            has_open_thesis: true,
            thesis_stale: true,
            any_source_stale: false,
        });

        assert_eq!(got.status, "due");
        assert_eq!(got.next_action, "reevaluate_thesis");
    }

    #[test]
    fn decision_waits_on_nonblocking_evidence_before_retrying_declined_symbol() {
        let got = decide(BrainDecisionInput {
            source_blocked: false,
            evidence_rows: 7,
            open_evidence: 6,
            blocking_evidence: 0,
            due_evidence: 0,
            has_context: true,
            context_stale: false,
            has_open_thesis: false,
            thesis_stale: false,
            any_source_stale: false,
        });

        assert_eq!(got.status, "waiting_on_evidence");
        assert_eq!(got.next_action, "wait_for_evidence_retry");
    }

    #[test]
    fn decision_is_fresh_when_every_gate_is_current() {
        let got = decide(BrainDecisionInput {
            source_blocked: false,
            evidence_rows: 4,
            open_evidence: 0,
            blocking_evidence: 0,
            due_evidence: 0,
            has_context: true,
            context_stale: false,
            has_open_thesis: true,
            thesis_stale: false,
            any_source_stale: false,
        });

        assert_eq!(got.status, "fresh");
        assert_eq!(got.next_action, "monitor");
    }
}
