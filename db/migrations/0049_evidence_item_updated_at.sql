-- 0049_evidence_item_updated_at.sql
--
-- Evidence rows can be updated after insertion: news sentiment can be scored
-- later, research relevance can be refreshed, and source_ref details can be
-- merged on conflict. Cognition freshness should react to that availability
-- change, not only to the original insert time.

ALTER TABLE evidence_item
    ADD COLUMN IF NOT EXISTS updated_at timestamptz;

UPDATE evidence_item
   SET updated_at = created_at
 WHERE updated_at IS NULL;

ALTER TABLE evidence_item
    ALTER COLUMN updated_at SET DEFAULT now();

ALTER TABLE evidence_item
    ALTER COLUMN updated_at SET NOT NULL;

CREATE INDEX IF NOT EXISTS ix_evidence_item_symbol_updated
    ON evidence_item(symbol, updated_at DESC);
