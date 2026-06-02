-- 0052_decision_disagreement_reasons.sql
-- Structured operator disagreement feedback for skip/reject decisions.

ALTER TABLE decision
    ADD COLUMN IF NOT EXISTS disagreement_reason text,
    ADD COLUMN IF NOT EXISTS disagreement_detail text;

ALTER TABLE decision
    DROP CONSTRAINT IF EXISTS decision_disagreement_reason_check;

ALTER TABLE decision
    ADD CONSTRAINT decision_disagreement_reason_check
    CHECK (
        disagreement_reason IS NULL
        OR disagreement_reason IN (
            'wrong_cluster',
            'not_my_edge',
            'signal_too_weak',
            'valuation_priced',
            'data_stale',
            'llm_overreached',
            'risk_too_high',
            'other'
        )
    );
