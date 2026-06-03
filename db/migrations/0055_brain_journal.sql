-- 0055_brain_journal.sql
--
-- Append-only daily operator ledger. The journal is not a replacement for
-- attention, evidence, thesis history, or source tasks; it is the daily
-- "what did the brain notice?" view that links back to those source rows.

CREATE TABLE IF NOT EXISTS brain_journal_entry (
    id              bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    journal_date    date NOT NULL,
    category        text NOT NULL CHECK (category IN (
                        'changed',
                        'curious',
                        'research',
                        'crowded_or_extended',
                        'ignored_or_hated',
                        'blocked'
                    )),
    source_kind     text NOT NULL CHECK (source_kind IN (
                        'attention',
                        'thesis_state',
                        'thesis_version',
                        'source_task',
                        'evidence',
                        'brain_thesis'
                    )),
    source_id       text NOT NULL,
    event_key       text NOT NULL UNIQUE,
    symbol          text,
    brain_thesis_id uuid REFERENCES brain_thesis(id) ON DELETE SET NULL,
    thesis_id       uuid REFERENCES thesis(thesis_id) ON DELETE SET NULL,
    title           text NOT NULL,
    summary         text NOT NULL,
    importance      int NOT NULL DEFAULT 50 CHECK (importance >= 0 AND importance <= 100),
    occurred_at     timestamptz NOT NULL,
    source_ref      jsonb NOT NULL DEFAULT '{}'::jsonb,
    created_at      timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS ix_brain_journal_date_importance
    ON brain_journal_entry(journal_date, importance DESC, occurred_at DESC);

CREATE INDEX IF NOT EXISTS ix_brain_journal_symbol_date
    ON brain_journal_entry(symbol, journal_date DESC)
    WHERE symbol IS NOT NULL;
