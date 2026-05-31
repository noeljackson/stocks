-- 0007_consensus_score.sql — per-symbol consensus score over time (#21).
--
-- SPEC §6.2 calls consensus "the most-reused event in the system": it fires
-- the FULFILLMENT/exit transition for discovery theses ("sell to the crowd"),
-- AND it's the validation anchor for lead-time (consensus_at − alert_at).
-- Both consumers need the per-symbol score timestamped and persisted.
--
-- Append-only — we want the full history so the reflection layer (#23) can
-- look back at when crossings happened relative to alerts.

CREATE TABLE IF NOT EXISTS consensus_score (
    id                  bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    symbol              text NOT NULL,
    score               double precision NOT NULL,            -- 0..100
    components          jsonb NOT NULL,                       -- per-component contributions for audit
    measurement_crossed bool NOT NULL DEFAULT false,          -- score >= 60 (lead-time anchor)
    exit_crossed        bool NOT NULL DEFAULT false,          -- score >= 70 (fires fulfillment)
    config_version      text,
    computed_at         timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS ix_consensus_score_symbol  ON consensus_score(symbol, computed_at DESC);
CREATE INDEX IF NOT EXISTS ix_consensus_score_at_brin ON consensus_score USING brin(computed_at);

COMMENT ON TABLE consensus_score IS
'Per-symbol consensus rolling score (SPEC §6.2). Append-only. '
'measurement_crossed firing = "consensus formed" timestamp (lead-time anchor); '
'exit_crossed firing = thesis fulfillment trigger for the symbol''s discovery theses.';
