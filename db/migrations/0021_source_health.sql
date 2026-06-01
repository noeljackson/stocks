-- Explicit ingest/source health. This records pass-level outcomes separately
-- from normalized rows so "0 rows inserted" does not look like "source dead".
CREATE TABLE IF NOT EXISTS source_health (
    source              text PRIMARY KEY,
    last_started_at     timestamptz,
    last_success_at     timestamptz,
    last_failure_at     timestamptz,
    last_status         text NOT NULL DEFAULT 'unknown',
    last_failure_kind   text,
    last_error          text,
    retry_after_at      timestamptz,
    rows_seen           bigint NOT NULL DEFAULT 0,
    rows_inserted       bigint NOT NULL DEFAULT 0,
    symbols_attempted   integer NOT NULL DEFAULT 0,
    symbols_failed      integer NOT NULL DEFAULT 0,
    updated_at          timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS ix_source_health_updated_at ON source_health(updated_at DESC);
