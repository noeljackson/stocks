-- 0034_source_tasks.sql
--
-- Source tasks are the active work queue behind evidence requirements. A
-- requirement says what is missing; a source task says what the system should
-- fetch, when it is due, and why it is waiting.

CREATE TABLE IF NOT EXISTS source_task (
    id              bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    source_type     text NOT NULL,
    requirement_key text,
    action          text NOT NULL,
    scope           text NOT NULL DEFAULT 'symbol'
                    CHECK (scope IN ('symbol', 'factor', 'universe', 'benchmark')),
    target_id       text NOT NULL,
    provider        text NOT NULL,
    limiter_key     text NOT NULL,
    state           text NOT NULL DEFAULT 'queued'
                    CHECK (state IN ('queued', 'fetching', 'satisfied', 'no_rows', 'rate_limited', 'failed', 'blocked')),
    priority        text NOT NULL DEFAULT 'medium'
                    CHECK (priority IN ('low', 'medium', 'high', 'blocking')),
    due_at          timestamptz NOT NULL DEFAULT now(),
    attempts        int NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    next_retry_at   timestamptz,
    last_error      text,
    source_ref      jsonb NOT NULL DEFAULT '{}'::jsonb,
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now(),
    UNIQUE (scope, target_id, requirement_key, action)
);

CREATE INDEX IF NOT EXISTS ix_source_task_due
    ON source_task(priority, due_at)
    WHERE state IN ('queued', 'no_rows', 'failed', 'rate_limited', 'blocked');

CREATE INDEX IF NOT EXISTS ix_source_task_target
    ON source_task(scope, target_id, state, priority, updated_at DESC);

CREATE INDEX IF NOT EXISTS ix_source_task_provider
    ON source_task(provider, state, due_at);

WITH open_requirements AS (
    SELECT er.*,
           COALESCE(
               NULLIF(er.source_ref->>'acquisition_state', ''),
               er.blocking_state
           ) AS acquisition_state
      FROM evidence_requirement er
),
actions AS (
    SELECT er.*,
           action.action
      FROM open_requirements er
      CROSS JOIN LATERAL jsonb_array_elements_text(
          COALESCE(er.source_ref->'fetch_actions', '[]'::jsonb)
      ) AS action(action)
),
task_rows AS (
    SELECT source_type,
           requirement_key,
           action,
           'symbol'::text AS scope,
           symbol AS target_id,
           CASE
             WHEN action LIKE 'fmp_%' THEN 'fmp'
             WHEN action LIKE 'massive_%' THEN 'massive'
             WHEN action LIKE 'sec_%' THEN 'sec'
             WHEN action LIKE 'gdelt_%' THEN 'gdelt'
             WHEN action LIKE 'bing_%' THEN 'bing'
             WHEN action LIKE 'llm_%' THEN 'llm'
             ELSE source_type
           END AS provider,
           CASE
             WHEN action LIKE 'fmp_%' THEN 'fmp'
             WHEN action LIKE 'massive_%' THEN 'massive'
             WHEN action LIKE 'sec_%' THEN 'sec'
             WHEN action LIKE 'gdelt_%' THEN 'gdelt'
             WHEN action LIKE 'bing_%' THEN 'bing'
             WHEN action LIKE 'llm_%' THEN 'llm'
             ELSE source_type
           END AS limiter_key,
           CASE
             WHEN blocking_state = 'satisfied' THEN 'satisfied'
             WHEN blocking_state = 'fetching' THEN 'fetching'
             WHEN acquisition_state = 'rate_limited' THEN 'rate_limited'
             WHEN blocking_state = 'blocked' THEN 'failed'
             WHEN acquisition_state IN ('source_checked_no_new_rows', 'source_checked_no_relevant_rows', 'no_relevant_symbol_evidence_after_success') THEN 'no_rows'
             ELSE 'queued'
           END AS state,
           priority,
           COALESCE(next_retry_at, now()) AS due_at,
           attempts,
           next_retry_at,
           last_error,
           jsonb_build_object(
             'created_by', '0034_source_tasks',
             'evidence_requirement_id', id,
             'evidence_blocking_state', blocking_state,
             'acquisition_state', acquisition_state,
             'source_health', COALESCE(source_ref->'source_health', '[]'::jsonb)
           ) AS source_ref
      FROM actions
)
INSERT INTO source_task
    (source_type, requirement_key, action, scope, target_id, provider, limiter_key,
     state, priority, due_at, attempts, next_retry_at, last_error, source_ref)
SELECT source_type, requirement_key, action, scope, target_id, provider, limiter_key,
       state, priority, due_at, attempts, next_retry_at, last_error, source_ref
  FROM task_rows
ON CONFLICT (scope, target_id, requirement_key, action) DO UPDATE SET
    source_type = EXCLUDED.source_type,
    provider = EXCLUDED.provider,
    limiter_key = EXCLUDED.limiter_key,
    state = EXCLUDED.state,
    priority = EXCLUDED.priority,
    due_at = EXCLUDED.due_at,
    attempts = EXCLUDED.attempts,
    next_retry_at = EXCLUDED.next_retry_at,
    last_error = EXCLUDED.last_error,
    source_ref = EXCLUDED.source_ref,
    updated_at = now();
