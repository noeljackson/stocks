-- 0061_price_alert_rules.sql
--
-- Price alert rules are operator/system-managed objects. Trigger events are
-- append-only and feed the existing alert + attention surfaces.

ALTER TABLE alert
  DROP CONSTRAINT IF EXISTS alert_kind_check,
  ADD CONSTRAINT alert_kind_check
    CHECK (kind IN ('state_transition','alignment','consensus','risk','price_alert'));

ALTER TABLE attention_item
  DROP CONSTRAINT IF EXISTS attention_item_kind_check,
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

ALTER TABLE attention_item
  DROP CONSTRAINT IF EXISTS attention_item_source_check,
  ADD CONSTRAINT attention_item_source_check
    CHECK (source IN ('discovery', 'thesis', 'risk', 'context', 'consensus', 'reflection', 'system', 'price_alert'));

CREATE TABLE IF NOT EXISTS price_alert_rule (
    id             bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    symbol         text NOT NULL,
    thesis_id      uuid REFERENCES thesis(thesis_id),
    origin         text NOT NULL CHECK (origin IN ('manual', 'ai')),
    intent         text NOT NULL CHECK (intent IN ('watch', 'entry', 'invalidation', 'exit')),
    direction      text NOT NULL CHECK (direction IN ('above', 'below')),
    target_price   numeric NOT NULL CHECK (target_price > 0),
    label          text NOT NULL,
    rationale      text,
    semantic_key   text,
    status         text NOT NULL DEFAULT 'active'
                   CHECK (status IN ('active', 'triggered', 'disabled', 'expired')),
    source_ref     jsonb NOT NULL DEFAULT '{}'::jsonb,
    expires_at     timestamptz,
    created_at     timestamptz NOT NULL DEFAULT now(),
    updated_at     timestamptz NOT NULL DEFAULT now(),
    triggered_at   timestamptz,
    disabled_at    timestamptz
);

CREATE INDEX IF NOT EXISTS ix_price_alert_rule_active
    ON price_alert_rule(symbol, created_at DESC)
 WHERE status = 'active';

CREATE INDEX IF NOT EXISTS ix_price_alert_rule_status
    ON price_alert_rule(status, created_at DESC);

CREATE UNIQUE INDEX IF NOT EXISTS ux_price_alert_rule_ai_semantic_active
    ON price_alert_rule(symbol, intent, semantic_key)
 WHERE origin = 'ai' AND status = 'active' AND semantic_key IS NOT NULL;

CREATE TABLE IF NOT EXISTS price_alert_event (
    id              bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    rule_id         bigint NOT NULL REFERENCES price_alert_rule(id),
    symbol          text NOT NULL,
    thesis_id       uuid REFERENCES thesis(thesis_id),
    triggered_at    timestamptz NOT NULL DEFAULT now(),
    trigger_ts      timestamptz NOT NULL,
    trigger_interval text NOT NULL,
    trigger_price   numeric NOT NULL,
    rule_snapshot   jsonb NOT NULL DEFAULT '{}'::jsonb
);

CREATE UNIQUE INDEX IF NOT EXISTS ux_price_alert_event_rule
    ON price_alert_event(rule_id);

CREATE INDEX IF NOT EXISTS ix_price_alert_event_symbol
    ON price_alert_event(symbol, triggered_at DESC);
