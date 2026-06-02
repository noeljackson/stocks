-- 0045_decision_replay.sql
--
-- Point-in-time decision replay snapshots (#94). These rows freeze the source
-- state visible when the operator recorded a decision, so later reflection does
-- not grade against evidence that arrived afterward.

CREATE TABLE IF NOT EXISTS decision_replay (
    decision_id       uuid PRIMARY KEY REFERENCES decision(decision_id) ON DELETE CASCADE,
    symbol            text NOT NULL,
    thesis_id         uuid REFERENCES thesis(thesis_id),
    context_version   int,
    thesis_snapshot   jsonb NOT NULL DEFAULT '{}'::jsonb,
    consensus_score   double precision,
    risk_verdict      jsonb NOT NULL DEFAULT '{}'::jsonb,
    evidence_ids      bigint[] NOT NULL DEFAULT ARRAY[]::bigint[],
    evidence_snapshot jsonb NOT NULL DEFAULT '[]'::jsonb,
    system_confidence text,
    chart_range_seen  text,
    captured_at       timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS ix_decision_replay_symbol_captured
    ON decision_replay(symbol, captured_at DESC);

