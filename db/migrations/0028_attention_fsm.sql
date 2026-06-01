-- 0028_attention_fsm.sql
--
-- Attention is not just open/resolved/dismissed. Keep the existing coarse
-- status for compatibility with terminal states, and add the operational FSM
-- fields needed for defer/retry/resurface behavior.

ALTER TABLE attention_item
  ADD COLUMN IF NOT EXISTS fsm_state text NOT NULL DEFAULT 'ready_for_review'
    CHECK (fsm_state IN (
      'queued',
      'evaluating',
      'waiting_on_data',
      'ready_for_review',
      'operator_deferred',
      'actionable',
      'resolved',
      'dismissed',
      'blocked'
    )),
  ADD COLUMN IF NOT EXISTS owner text NOT NULL DEFAULT 'operator'
    CHECK (owner IN ('system', 'operator', 'source', 'cognition', 'risk')),
  ADD COLUMN IF NOT EXISTS next_retry_at timestamptz,
  ADD COLUMN IF NOT EXISTS resurface_at timestamptz,
  ADD COLUMN IF NOT EXISTS state_reason text;

UPDATE attention_item
   SET fsm_state = CASE
         WHEN status = 'resolved' THEN 'resolved'
         WHEN status = 'dismissed' THEN 'dismissed'
         ELSE fsm_state
       END
 WHERE (status = 'resolved' AND fsm_state <> 'resolved')
    OR (status = 'dismissed' AND fsm_state <> 'dismissed');

CREATE TABLE IF NOT EXISTS attention_state_history (
    id              bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    attention_id    bigint NOT NULL REFERENCES attention_item(id) ON DELETE CASCADE,
    from_state      text,
    to_state        text NOT NULL,
    owner           text NOT NULL,
    reason          text,
    next_retry_at   timestamptz,
    resurface_at    timestamptz,
    source_ref      jsonb NOT NULL DEFAULT '{}'::jsonb,
    transitioned_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS ix_attention_visible_open
    ON attention_item(severity, created_at DESC)
 WHERE status = 'open'
   AND fsm_state <> 'operator_deferred';

CREATE INDEX IF NOT EXISTS ix_attention_resurface
    ON attention_item(resurface_at)
 WHERE status = 'open' AND fsm_state = 'operator_deferred';
