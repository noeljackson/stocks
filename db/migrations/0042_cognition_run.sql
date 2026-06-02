-- 0042_cognition_run.sql
--
-- Durable brain-loop attempt ledger. Context/thesis timestamps say what changed;
-- this table says what cognition attempted, why it ran, what blocked it, and
-- when it finished.

CREATE TABLE IF NOT EXISTS cognition_run (
    id                      bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    symbol                  text NOT NULL REFERENCES ticker(symbol),
    trigger                 text NOT NULL,
    sweep_reason            text,
    status                  text NOT NULL
                            CHECK (status IN (
                                'running',
                                'context_refreshed',
                                'blocked_on_evidence',
                                'declined',
                                'drafted',
                                'reconciled',
                                'no_change',
                                'failed'
                            )),
    reason                  text,
    context_version         int,
    thesis_id               uuid REFERENCES thesis(thesis_id),
    thesis_classification   text,
    evidence_open_count     int NOT NULL DEFAULT 0 CHECK (evidence_open_count >= 0),
    evidence_blocking_count int NOT NULL DEFAULT 0 CHECK (evidence_blocking_count >= 0),
    started_at              timestamptz NOT NULL DEFAULT now(),
    finished_at             timestamptz,
    next_retry_at           timestamptz,
    error                   text,
    source_ref              jsonb NOT NULL DEFAULT '{}'::jsonb
);

CREATE INDEX IF NOT EXISTS ix_cognition_run_symbol_started
    ON cognition_run(symbol, started_at DESC);

CREATE INDEX IF NOT EXISTS ix_cognition_run_status_started
    ON cognition_run(status, started_at DESC);

CREATE INDEX IF NOT EXISTS ix_cognition_run_running
    ON cognition_run(started_at DESC)
    WHERE status = 'running';
