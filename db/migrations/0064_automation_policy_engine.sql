-- 0064_automation_policy_engine.sql
--
-- Deterministic proof/policy support (#294). Blocked preflight evaluations
-- need to be auditable even when no desired-position row is written.

ALTER TABLE automation_proof
    ALTER COLUMN desired_position_id DROP NOT NULL;

ALTER TABLE automation_proof
    ALTER COLUMN sleeve_id DROP NOT NULL;

CREATE INDEX IF NOT EXISTS ix_automation_proof_permission
    ON automation_proof(permission_id, evaluated_at DESC);

CREATE TABLE IF NOT EXISTS automation_control_state (
    id                  bool PRIMARY KEY DEFAULT true CHECK (id),
    kill_switch_enabled bool NOT NULL DEFAULT false,
    kill_switch_reason  text,
    updated_by          text NOT NULL DEFAULT 'system',
    updated_at          timestamptz NOT NULL DEFAULT now()
);

INSERT INTO automation_control_state (id, kill_switch_enabled, kill_switch_reason)
VALUES (true, false, NULL)
ON CONFLICT (id) DO NOTHING;
