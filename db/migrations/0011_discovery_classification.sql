-- 0011_discovery_classification.sql — LLM classification + decision audit (#55).
--
-- When discovery fires, the classification service (#55) proposes which
-- watchlist(s) the candidate fits. Persisted here for UI review. On
-- confirm/reject, decided_at + user_rationale are recorded — feeds the
-- circle-of-competence weight in §6.1 domain_fit scoring.

CREATE TABLE IF NOT EXISTS discovery_classification (
    id              bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    candidate_id    bigint NOT NULL REFERENCES discovery_candidate(id) ON DELETE CASCADE,
    -- Proposed lists: jsonb array of { watchlist_id, watchlist_name, confidence, rationale }.
    -- Stored as JSONB so we don't need a fan-out child table for what's effectively
    -- a small per-candidate prediction set.
    proposed_lists  jsonb NOT NULL DEFAULT '[]',
    -- Optional "we don't have a list for this yet" output.
    suggested_new_list jsonb,
    prompt_name     text NOT NULL,
    prompt_hash     text NOT NULL,
    created_at      timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS ix_dclass_candidate ON discovery_classification(candidate_id);
