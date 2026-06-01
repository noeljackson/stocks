-- 0023_evidence_requirements.sql — first-class missing-evidence work queue (#136).
--
-- Missing evidence is not a conclusion. It is a retryable acquisition state
-- that should be visible to the operator and to the cognition sweep.

CREATE TABLE IF NOT EXISTS evidence_requirement (
    id               bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    symbol           text NOT NULL REFERENCES ticker(symbol),
    requirement_key  text NOT NULL,
    source_type      text NOT NULL,
    reason           text NOT NULL,
    priority         text NOT NULL DEFAULT 'medium'
                     CHECK (priority IN ('low', 'medium', 'high', 'blocking')),
    blocking_state   text NOT NULL DEFAULT 'missing'
                     CHECK (blocking_state IN ('missing', 'fetching', 'partial', 'blocked', 'satisfied')),
    attempts         int NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    next_retry_at    timestamptz,
    last_error       text,
    source_ref       jsonb NOT NULL DEFAULT '{}'::jsonb,
    created_at       timestamptz NOT NULL DEFAULT now(),
    updated_at       timestamptz NOT NULL DEFAULT now(),
    satisfied_at     timestamptz,
    UNIQUE (symbol, requirement_key)
);

CREATE INDEX IF NOT EXISTS ix_evidence_requirement_symbol_state
    ON evidence_requirement(symbol, blocking_state, priority, updated_at DESC);

CREATE INDEX IF NOT EXISTS ix_evidence_requirement_retry
    ON evidence_requirement(next_retry_at)
    WHERE blocking_state IN ('missing', 'partial', 'blocked') AND satisfied_at IS NULL;
