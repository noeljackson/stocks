-- 0062_automation_state.sql
--
-- Automation v2 foundation (#291). This migration adds durable state for
-- permissioned strategy automation, but does not add broker order placement.
-- Strategies write desired exposure. Proof/reconciliation/execution gates are
-- separate state objects so no strategy can be treated as an order source.

CREATE TABLE IF NOT EXISTS automation_strategy_definition (
    strategy_id       text NOT NULL,
    strategy_version  text NOT NULL,
    family            text NOT NULL CHECK (family IN ('thesis_timing', 'technical_timing', 'hybrid', 'manual')),
    display_name      text NOT NULL,
    description       text,
    config_hash       text NOT NULL,
    config            jsonb NOT NULL DEFAULT '{}'::jsonb,
    status            text NOT NULL DEFAULT 'draft'
                      CHECK (status IN ('draft', 'shadow', 'paper', 'canary_live', 'expanded_live', 'frozen', 'retired')),
    created_at        timestamptz NOT NULL DEFAULT now(),
    retired_at        timestamptz,
    PRIMARY KEY (strategy_id, strategy_version)
);

CREATE INDEX IF NOT EXISTS ix_automation_strategy_status
    ON automation_strategy_definition(status, strategy_id);

CREATE TABLE IF NOT EXISTS automation_trade_permission (
    permission_id       uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    symbol              text NOT NULL REFERENCES ticker(symbol),
    strategy_id         text NOT NULL,
    strategy_version    text NOT NULL,
    status              text NOT NULL DEFAULT 'pending'
                        CHECK (status IN ('pending', 'approved', 'revoked', 'expired')),
    instrument_scope    text NOT NULL DEFAULT 'equity_long_short'
                        CHECK (instrument_scope IN ('equity_long_short', 'equity_long_only', 'equity_short_only')),
    environment_scope   text NOT NULL DEFAULT 'shadow'
                        CHECK (environment_scope IN ('shadow', 'paper', 'canary_live', 'expanded_live')),
    max_allocation_pct  numeric CHECK (max_allocation_pct IS NULL OR (max_allocation_pct > 0 AND max_allocation_pct <= 1)),
    max_notional_usd    numeric CHECK (max_notional_usd IS NULL OR max_notional_usd > 0),
    max_quantity        numeric CHECK (max_quantity IS NULL OR max_quantity > 0),
    manual_freeze       bool NOT NULL DEFAULT false,
    freeze_reason       text,
    approved_by         text,
    approved_at         timestamptz,
    expires_at          timestamptz,
    source_ref          jsonb NOT NULL DEFAULT '{}'::jsonb,
    created_at          timestamptz NOT NULL DEFAULT now(),
    updated_at          timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (strategy_id, strategy_version)
        REFERENCES automation_strategy_definition(strategy_id, strategy_version)
);

CREATE INDEX IF NOT EXISTS ix_automation_permission_symbol
    ON automation_trade_permission(symbol, status, environment_scope, updated_at DESC);

CREATE INDEX IF NOT EXISTS ix_automation_permission_strategy
    ON automation_trade_permission(strategy_id, strategy_version, status, updated_at DESC);

CREATE UNIQUE INDEX IF NOT EXISTS ux_automation_permission_active
    ON automation_trade_permission(symbol, strategy_id, strategy_version, environment_scope)
 WHERE status IN ('pending', 'approved');

CREATE TABLE IF NOT EXISTS automation_permission_event (
    id               bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    permission_id    uuid NOT NULL REFERENCES automation_trade_permission(permission_id) ON DELETE CASCADE,
    event_kind       text NOT NULL CHECK (event_kind IN ('created', 'approved', 'revoked', 'expired', 'freeze_set', 'freeze_cleared', 'limits_changed')),
    from_status      text,
    to_status        text,
    manual_freeze    bool,
    actor            text NOT NULL DEFAULT 'system',
    reason           text,
    source_ref       jsonb NOT NULL DEFAULT '{}'::jsonb,
    occurred_at      timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS ix_automation_permission_event_permission
    ON automation_permission_event(permission_id, occurred_at DESC);

CREATE TABLE IF NOT EXISTS automation_strategy_sleeve (
    sleeve_id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    sleeve_kind          text NOT NULL CHECK (sleeve_kind IN ('manual', 'strategy')),
    permission_id        uuid REFERENCES automation_trade_permission(permission_id),
    symbol               text NOT NULL REFERENCES ticker(symbol),
    strategy_id          text,
    strategy_version     text,
    status               text NOT NULL DEFAULT 'active'
                         CHECK (status IN ('active', 'frozen', 'closed')),
    current_side         text NOT NULL DEFAULT 'flat'
                         CHECK (current_side IN ('flat', 'long', 'short')),
    current_quantity     numeric NOT NULL DEFAULT 0 CHECK (current_quantity >= 0),
    current_notional_usd numeric NOT NULL DEFAULT 0 CHECK (current_notional_usd >= 0),
    allocated_notional_usd numeric CHECK (allocated_notional_usd IS NULL OR allocated_notional_usd >= 0),
    realized_pnl         numeric NOT NULL DEFAULT 0,
    source_ref           jsonb NOT NULL DEFAULT '{}'::jsonb,
    opened_at            timestamptz NOT NULL DEFAULT now(),
    closed_at            timestamptz,
    updated_at           timestamptz NOT NULL DEFAULT now(),
    CHECK (
        (sleeve_kind = 'manual'
         AND permission_id IS NULL
         AND strategy_id IS NULL
         AND strategy_version IS NULL)
        OR
        (sleeve_kind = 'strategy'
         AND permission_id IS NOT NULL
         AND strategy_id IS NOT NULL
         AND strategy_version IS NOT NULL)
    ),
    FOREIGN KEY (strategy_id, strategy_version)
        REFERENCES automation_strategy_definition(strategy_id, strategy_version)
);

CREATE UNIQUE INDEX IF NOT EXISTS ux_automation_manual_sleeve_open
    ON automation_strategy_sleeve(symbol)
 WHERE sleeve_kind = 'manual'
   AND closed_at IS NULL;

CREATE UNIQUE INDEX IF NOT EXISTS ux_automation_strategy_sleeve_open
    ON automation_strategy_sleeve(permission_id)
 WHERE sleeve_kind = 'strategy'
   AND closed_at IS NULL;

CREATE INDEX IF NOT EXISTS ix_automation_sleeve_symbol
    ON automation_strategy_sleeve(symbol, status, updated_at DESC);

CREATE TABLE IF NOT EXISTS automation_sleeve_event (
    id             bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    sleeve_id      uuid NOT NULL REFERENCES automation_strategy_sleeve(sleeve_id) ON DELETE CASCADE,
    event_kind     text NOT NULL CHECK (event_kind IN ('created', 'allocated', 'desired_position', 'fill_attributed', 'manual_freeze', 'manual_unfreeze', 'closed', 'reconciled')),
    from_status    text,
    to_status      text,
    quantity_delta numeric,
    notional_delta numeric,
    realized_pnl_delta numeric,
    source_ref     jsonb NOT NULL DEFAULT '{}'::jsonb,
    occurred_at    timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS ix_automation_sleeve_event_sleeve
    ON automation_sleeve_event(sleeve_id, occurred_at DESC);

CREATE TABLE IF NOT EXISTS desired_strategy_position (
    desired_position_id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    permission_id       uuid NOT NULL REFERENCES automation_trade_permission(permission_id),
    sleeve_id           uuid NOT NULL REFERENCES automation_strategy_sleeve(sleeve_id),
    symbol              text NOT NULL REFERENCES ticker(symbol),
    thesis_id           uuid REFERENCES thesis(thesis_id),
    strategy_id         text NOT NULL,
    strategy_version    text NOT NULL,
    target_side         text NOT NULL CHECK (target_side IN ('flat', 'long', 'short')),
    target_quantity     numeric CHECK (target_quantity IS NULL OR target_quantity >= 0),
    target_notional_usd numeric CHECK (target_notional_usd IS NULL OR target_notional_usd >= 0),
    target_weight_pct   numeric CHECK (target_weight_pct IS NULL OR (target_weight_pct >= 0 AND target_weight_pct <= 1)),
    rationale           text,
    reason_codes        jsonb NOT NULL DEFAULT '[]'::jsonb,
    feature_snapshot    jsonb NOT NULL DEFAULT '{}'::jsonb,
    signal_ref          jsonb NOT NULL DEFAULT '{}'::jsonb,
    supersedes_desired_position_id uuid REFERENCES desired_strategy_position(desired_position_id),
    emitted_at          timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (strategy_id, strategy_version)
        REFERENCES automation_strategy_definition(strategy_id, strategy_version)
);

CREATE INDEX IF NOT EXISTS ix_desired_strategy_position_symbol
    ON desired_strategy_position(symbol, emitted_at DESC);

CREATE INDEX IF NOT EXISTS ix_desired_strategy_position_permission
    ON desired_strategy_position(permission_id, emitted_at DESC);

CREATE INDEX IF NOT EXISTS ix_desired_strategy_position_sleeve
    ON desired_strategy_position(sleeve_id, emitted_at DESC);

CREATE TABLE IF NOT EXISTS automation_proof (
    proof_id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    desired_position_id uuid NOT NULL REFERENCES desired_strategy_position(desired_position_id),
    permission_id       uuid NOT NULL REFERENCES automation_trade_permission(permission_id),
    sleeve_id           uuid NOT NULL REFERENCES automation_strategy_sleeve(sleeve_id),
    symbol              text NOT NULL REFERENCES ticker(symbol),
    strategy_id         text NOT NULL,
    strategy_version    text NOT NULL,
    environment_scope   text NOT NULL CHECK (environment_scope IN ('shadow', 'paper', 'canary_live', 'expanded_live')),
    result              text NOT NULL CHECK (result IN ('passed', 'warning', 'blocked')),
    blocked_reasons     jsonb NOT NULL DEFAULT '[]'::jsonb,
    input_snapshot      jsonb NOT NULL DEFAULT '{}'::jsonb,
    permission_snapshot jsonb NOT NULL DEFAULT '{}'::jsonb,
    risk_result         jsonb NOT NULL DEFAULT '{}'::jsonb,
    data_freshness      jsonb NOT NULL DEFAULT '{}'::jsonb,
    session_state       jsonb NOT NULL DEFAULT '{}'::jsonb,
    capital_allocation  jsonb NOT NULL DEFAULT '{}'::jsonb,
    broker_reconciliation jsonb NOT NULL DEFAULT '{}'::jsonb,
    evaluated_at        timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (strategy_id, strategy_version)
        REFERENCES automation_strategy_definition(strategy_id, strategy_version)
);

CREATE INDEX IF NOT EXISTS ix_automation_proof_desired
    ON automation_proof(desired_position_id, evaluated_at DESC);

CREATE INDEX IF NOT EXISTS ix_automation_proof_symbol
    ON automation_proof(symbol, evaluated_at DESC);

CREATE TABLE IF NOT EXISTS automation_execution_reconciliation (
    reconciliation_id   uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    desired_position_id uuid NOT NULL REFERENCES desired_strategy_position(desired_position_id),
    proof_id            uuid REFERENCES automation_proof(proof_id),
    sleeve_id           uuid NOT NULL REFERENCES automation_strategy_sleeve(sleeve_id),
    symbol              text NOT NULL REFERENCES ticker(symbol),
    environment_scope   text NOT NULL CHECK (environment_scope IN ('shadow', 'paper', 'canary_live', 'expanded_live')),
    status              text NOT NULL DEFAULT 'pending'
                        CHECK (status IN ('pending', 'noop', 'needs_order', 'submitted', 'reconciled', 'blocked', 'incident')),
    idempotency_key     text NOT NULL,
    target_snapshot     jsonb NOT NULL DEFAULT '{}'::jsonb,
    broker_snapshot     jsonb NOT NULL DEFAULT '{}'::jsonb,
    delta_snapshot      jsonb NOT NULL DEFAULT '{}'::jsonb,
    order_plan          jsonb NOT NULL DEFAULT '{}'::jsonb,
    blocked_reasons     jsonb NOT NULL DEFAULT '[]'::jsonb,
    source_ref          jsonb NOT NULL DEFAULT '{}'::jsonb,
    created_at          timestamptz NOT NULL DEFAULT now(),
    updated_at          timestamptz NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX IF NOT EXISTS ux_automation_reconciliation_idempotency
    ON automation_execution_reconciliation(idempotency_key);

CREATE INDEX IF NOT EXISTS ix_automation_reconciliation_symbol
    ON automation_execution_reconciliation(symbol, status, updated_at DESC);

CREATE TABLE IF NOT EXISTS automation_incident (
    incident_id       uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    severity          text NOT NULL CHECK (severity IN ('info', 'warning', 'critical')),
    status            text NOT NULL DEFAULT 'open'
                      CHECK (status IN ('open', 'acknowledged', 'resolved')),
    kind              text NOT NULL,
    symbol            text REFERENCES ticker(symbol),
    permission_id     uuid REFERENCES automation_trade_permission(permission_id),
    sleeve_id         uuid REFERENCES automation_strategy_sleeve(sleeve_id),
    desired_position_id uuid REFERENCES desired_strategy_position(desired_position_id),
    proof_id          uuid REFERENCES automation_proof(proof_id),
    reconciliation_id uuid REFERENCES automation_execution_reconciliation(reconciliation_id),
    title             text NOT NULL,
    detail            text,
    source_ref        jsonb NOT NULL DEFAULT '{}'::jsonb,
    created_at        timestamptz NOT NULL DEFAULT now(),
    acknowledged_at   timestamptz,
    resolved_at       timestamptz,
    resolved_by       text
);

CREATE INDEX IF NOT EXISTS ix_automation_incident_open
    ON automation_incident(severity, created_at DESC)
 WHERE status <> 'resolved';

CREATE INDEX IF NOT EXISTS ix_automation_incident_symbol
    ON automation_incident(symbol, created_at DESC);
