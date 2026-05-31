-- 0019 — collapse stale duplicate open discovery candidates (#105).
--
-- Background: prior to the persist-time dedup landing, the discovery cron
-- created a new `discovery_candidate` row every pass (every ~5 min) for any
-- symbol whose signal still fired, accumulating ~30 proposed rows per ticker.
-- The UI already DISTINCT-ON's them at read time, but the underlying table
-- carries the cruft. This migration keeps only the MOST RECENT proposed row
-- per (symbol, signal_name); older duplicates are marked status='superseded'
-- so the open queue is clean going forward.
--
-- Safe to re-run: only rows that still match the duplicate predicate are
-- touched; once cleaned, the WHERE clause yields zero rows.

-- 1. Allow 'superseded' as a candidate status. (Schema is text-typed, no
--    explicit CHECK on status — but be explicit in case one's added later.)
DO $$
BEGIN
  IF EXISTS (
    SELECT 1 FROM pg_constraint
     WHERE conname = 'discovery_candidate_status_check'
  ) THEN
    ALTER TABLE discovery_candidate DROP CONSTRAINT discovery_candidate_status_check;
    ALTER TABLE discovery_candidate
      ADD CONSTRAINT discovery_candidate_status_check
      CHECK (status IN ('proposed', 'confirmed', 'rejected', 'superseded'));
  END IF;
END$$;

-- 2. Mark older duplicates as superseded.
WITH ranked AS (
  SELECT id,
         row_number() OVER (
           PARTITION BY symbol, signal_name
           ORDER BY proposed_at DESC
         ) AS rk
    FROM discovery_candidate
   WHERE status = 'proposed'
)
UPDATE discovery_candidate
   SET status = 'superseded'
 WHERE id IN (SELECT id FROM ranked WHERE rk > 1);

-- 3. Drop any open attention_items that pointed at the now-superseded rows.
UPDATE attention_item ai
   SET status = 'dismissed',
       resolved_at = COALESCE(resolved_at, now())
  FROM discovery_candidate dc
 WHERE ai.candidate_id = dc.id
   AND ai.kind = 'candidate_review'
   AND ai.status = 'open'
   AND dc.status = 'superseded';
