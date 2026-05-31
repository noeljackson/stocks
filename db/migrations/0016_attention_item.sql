-- 0016_attention_item.sql — operator attention queue (#86).
--
-- Events tell you what happened. Attention tells you what needs judgment.
-- Decisions record what the human chose.
--
-- Producers emit attention items when state crosses a "needs eyes" threshold.
-- Resolvers (candidate confirm/reject, decision recorded, context refresh,
-- alert ack, outcome scored) close out items by id.

CREATE TABLE IF NOT EXISTS attention_item (
    id              bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    kind            text NOT NULL CHECK (kind IN (
                        'candidate_review',
                        'context_stale',
                        'thesis_incomplete',
                        'thesis_actionable',
                        'risk_review',
                        'invalidation_hit',
                        'outcome_ready'
                    )),
    symbol          text,
    thesis_id       uuid REFERENCES thesis(thesis_id),
    candidate_id    bigint REFERENCES discovery_candidate(id) ON DELETE CASCADE,
    severity        text NOT NULL CHECK (severity IN ('info', 'review', 'decision', 'blocked')),
    status          text NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'resolved', 'dismissed')),
    title           text NOT NULL,
    reason          text,
    source          text NOT NULL CHECK (source IN ('discovery', 'thesis', 'risk', 'context', 'consensus', 'reflection', 'system')),
    source_ref      jsonb NOT NULL DEFAULT '{}'::jsonb,
    created_at      timestamptz NOT NULL DEFAULT now(),
    resolved_at     timestamptz,
    resolution_kind text,
    resolution_ref  jsonb
);

-- Dedup constraint: at most one OPEN item per (kind, candidate_id) and per
-- (kind, thesis_id) at a time. Producers can call upsert-style without
-- spamming the queue with duplicates of the same actionable situation.
CREATE UNIQUE INDEX IF NOT EXISTS ux_attention_open_candidate
    ON attention_item(kind, candidate_id)
    WHERE status = 'open' AND candidate_id IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS ux_attention_open_thesis
    ON attention_item(kind, thesis_id)
    WHERE status = 'open' AND thesis_id IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS ux_attention_open_symbol
    ON attention_item(kind, symbol)
    WHERE status = 'open' AND thesis_id IS NULL AND candidate_id IS NULL AND symbol IS NOT NULL;

CREATE INDEX IF NOT EXISTS ix_attention_open
    ON attention_item(severity, created_at DESC) WHERE status = 'open';
CREATE INDEX IF NOT EXISTS ix_attention_symbol
    ON attention_item(symbol, created_at DESC);
