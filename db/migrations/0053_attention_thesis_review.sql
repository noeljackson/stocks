-- 0053_attention_thesis_review.sql
-- Operator review cue for material thesis reconciliation updates.

ALTER TABLE attention_item
    DROP CONSTRAINT IF EXISTS attention_item_kind_check;

ALTER TABLE attention_item
    ADD CONSTRAINT attention_item_kind_check
    CHECK (kind IN (
        'candidate_review',
        'context_stale',
        'thesis_incomplete',
        'thesis_review',
        'thesis_actionable',
        'risk_review',
        'invalidation_hit',
        'outcome_ready',
        'price_alert'
    ));
