-- 0008_discovery_candidate.sql — discovery scanner output (#22).
--
-- One row per signal-firing per symbol per pass. Append-only. The UI lists
-- "candidates pending review" (status='proposed'); user confirms or rejects,
-- which feeds the circle-of-competence weight in domain_fit scoring (§6.1).

CREATE TABLE IF NOT EXISTS discovery_candidate (
    id              bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    symbol          text NOT NULL,
    signal_name     text NOT NULL,                -- 'volume_anomaly' | 'base_breakout' | …
    signal_value    double precision,             -- raw signal strength (each signal's own scale)
    domain_fit      double precision,             -- §6.1 score 0..100 (or null if not classified)
    proposed_tier   int NOT NULL DEFAULT 2 CHECK (proposed_tier IN (1, 2, 3)),
    status          text NOT NULL DEFAULT 'proposed'
                    CHECK (status IN ('proposed', 'confirmed', 'rejected', 'expired')),
    reasoning       text,                         -- short human-readable note
    config_version  text,
    proposed_at     timestamptz NOT NULL DEFAULT now(),
    decided_at      timestamptz,
    UNIQUE (symbol, signal_name, proposed_at)     -- multiple signals per pass OK; one per (sym,signal,time)
);
CREATE INDEX IF NOT EXISTS ix_dc_open ON discovery_candidate(status, proposed_at DESC)
    WHERE status = 'proposed';
CREATE INDEX IF NOT EXISTS ix_dc_symbol ON discovery_candidate(symbol, proposed_at DESC);
