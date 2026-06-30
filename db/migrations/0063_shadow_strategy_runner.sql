-- 0063_shadow_strategy_runner.sql
--
-- Shadow strategy runner support (#293). This keeps strategy output as
-- append-only desired exposure plus validation observations. It does not add
-- broker order placement or executable reconciliation.

ALTER TABLE desired_strategy_position
    ADD COLUMN IF NOT EXISTS strategy_config_hash text NOT NULL DEFAULT '';

ALTER TABLE automation_proof
    ADD COLUMN IF NOT EXISTS strategy_config_hash text NOT NULL DEFAULT '';

CREATE TABLE IF NOT EXISTS automation_strategy_signal_observation (
    observation_id       uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    desired_position_id  uuid NOT NULL REFERENCES desired_strategy_position(desired_position_id),
    proof_id             uuid REFERENCES automation_proof(proof_id),
    permission_id        uuid NOT NULL REFERENCES automation_trade_permission(permission_id),
    sleeve_id            uuid NOT NULL REFERENCES automation_strategy_sleeve(sleeve_id),
    symbol               text NOT NULL REFERENCES ticker(symbol),
    strategy_id          text NOT NULL,
    strategy_version     text NOT NULL,
    strategy_config_hash text NOT NULL,
    signal_key           text NOT NULL,
    target_side          text NOT NULL CHECK (target_side IN ('flat', 'long', 'short')),
    prior_target_side    text CHECK (prior_target_side IS NULL OR prior_target_side IN ('flat', 'long', 'short')),
    churn_event          bool NOT NULL DEFAULT false,
    reason_codes         jsonb NOT NULL DEFAULT '[]'::jsonb,
    feature_snapshot     jsonb NOT NULL DEFAULT '{}'::jsonb,
    validation_snapshot  jsonb NOT NULL DEFAULT '{}'::jsonb,
    evaluation_due_at    timestamptz NOT NULL,
    forward_return_pct   numeric,
    max_drawdown_pct     numeric,
    evaluated_at         timestamptz,
    created_at           timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (strategy_id, strategy_version)
        REFERENCES automation_strategy_definition(strategy_id, strategy_version)
);

CREATE UNIQUE INDEX IF NOT EXISTS ux_automation_signal_observation_desired
    ON automation_strategy_signal_observation(desired_position_id);

CREATE INDEX IF NOT EXISTS ix_automation_signal_observation_due
    ON automation_strategy_signal_observation(evaluation_due_at, symbol)
 WHERE evaluated_at IS NULL;

CREATE INDEX IF NOT EXISTS ix_automation_signal_observation_strategy
    ON automation_strategy_signal_observation(strategy_id, strategy_version, created_at DESC);
