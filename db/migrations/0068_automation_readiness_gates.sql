-- Strategy readiness and promotion gates (#299).
--
-- Promotions are threshold-gated and require explicit operator approval.
-- The evaluator writes append-only readiness rows; lifecycle changes are
-- captured as events before strategy status is advanced, demoted, frozen, or
-- retired.

CREATE TABLE IF NOT EXISTS automation_strategy_promotion_approval (
    approval_id      uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    strategy_id      text NOT NULL,
    strategy_version text NOT NULL,
    from_stage       text NOT NULL CHECK (from_stage IN ('draft', 'shadow', 'paper', 'canary_live', 'expanded_live', 'frozen', 'retired')),
    to_stage         text NOT NULL CHECK (to_stage IN ('draft', 'shadow', 'paper', 'canary_live', 'expanded_live', 'frozen', 'retired')),
    status           text NOT NULL DEFAULT 'approved'
                     CHECK (status IN ('approved', 'used', 'revoked', 'expired')),
    approved_by      text NOT NULL,
    approved_at      timestamptz NOT NULL DEFAULT now(),
    expires_at       timestamptz,
    reason           text,
    source_ref       jsonb NOT NULL DEFAULT '{}'::jsonb,
    created_at       timestamptz NOT NULL DEFAULT now(),
    updated_at       timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (strategy_id, strategy_version)
        REFERENCES automation_strategy_definition(strategy_id, strategy_version)
);

CREATE UNIQUE INDEX IF NOT EXISTS ux_automation_strategy_promotion_active
    ON automation_strategy_promotion_approval(strategy_id, strategy_version, from_stage, to_stage)
 WHERE status = 'approved';

CREATE INDEX IF NOT EXISTS ix_automation_strategy_promotion_lookup
    ON automation_strategy_promotion_approval(strategy_id, strategy_version, status, approved_at DESC);

CREATE TABLE IF NOT EXISTS automation_strategy_readiness_evaluation (
    evaluation_id    uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    strategy_id      text NOT NULL,
    strategy_version text NOT NULL,
    lifecycle_stage  text NOT NULL CHECK (lifecycle_stage IN ('draft', 'shadow', 'paper', 'canary_live', 'expanded_live', 'frozen', 'retired')),
    target_stage     text CHECK (target_stage IS NULL OR target_stage IN ('draft', 'shadow', 'paper', 'canary_live', 'expanded_live', 'frozen', 'retired')),
    status           text NOT NULL CHECK (status IN ('ready', 'blocked')),
    readiness_score  numeric NOT NULL CHECK (readiness_score >= 0 AND readiness_score <= 1),
    approval_id      uuid REFERENCES automation_strategy_promotion_approval(approval_id),
    approval_required bool NOT NULL DEFAULT true,
    approval_valid   bool NOT NULL DEFAULT false,
    freeze_live_permissions bool NOT NULL DEFAULT false,
    metrics          jsonb NOT NULL DEFAULT '{}'::jsonb,
    blockers         jsonb NOT NULL DEFAULT '[]'::jsonb,
    warnings         jsonb NOT NULL DEFAULT '[]'::jsonb,
    thresholds       jsonb NOT NULL DEFAULT '{}'::jsonb,
    lookback_days    int NOT NULL DEFAULT 90 CHECK (lookback_days BETWEEN 1 AND 3660),
    evaluated_at     timestamptz NOT NULL DEFAULT now(),
    source_ref       jsonb NOT NULL DEFAULT '{}'::jsonb,
    FOREIGN KEY (strategy_id, strategy_version)
        REFERENCES automation_strategy_definition(strategy_id, strategy_version)
);

CREATE INDEX IF NOT EXISTS ix_automation_strategy_readiness_latest
    ON automation_strategy_readiness_evaluation(strategy_id, strategy_version, evaluated_at DESC);

CREATE INDEX IF NOT EXISTS ix_automation_strategy_readiness_status
    ON automation_strategy_readiness_evaluation(status, target_stage, evaluated_at DESC);

CREATE TABLE IF NOT EXISTS automation_strategy_lifecycle_event (
    id               bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    strategy_id      text NOT NULL,
    strategy_version text NOT NULL,
    event_kind       text NOT NULL CHECK (event_kind IN (
                         'readiness_evaluated', 'promotion_approved', 'promoted',
                         'demoted', 'frozen', 'retired'
                     )),
    from_status      text,
    to_status        text,
    evaluation_id    uuid REFERENCES automation_strategy_readiness_evaluation(evaluation_id),
    approval_id      uuid REFERENCES automation_strategy_promotion_approval(approval_id),
    actor            text NOT NULL DEFAULT 'system',
    reason           text,
    source_ref       jsonb NOT NULL DEFAULT '{}'::jsonb,
    occurred_at      timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (strategy_id, strategy_version)
        REFERENCES automation_strategy_definition(strategy_id, strategy_version)
);

CREATE INDEX IF NOT EXISTS ix_automation_strategy_lifecycle_event_strategy
    ON automation_strategy_lifecycle_event(strategy_id, strategy_version, occurred_at DESC);
