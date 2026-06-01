-- 0030_thesis_last_evaluated.sql
--
-- updated_at means the thesis content/version changed. The brain loop also
-- needs to record no-change evaluations so open theses do not get re-run every
-- sweep just because no material version was needed.

ALTER TABLE thesis
  ADD COLUMN IF NOT EXISTS last_evaluated_at timestamptz;

UPDATE thesis
   SET last_evaluated_at = updated_at
 WHERE last_evaluated_at IS NULL;

CREATE INDEX IF NOT EXISTS ix_thesis_last_evaluated
    ON thesis(last_evaluated_at)
 WHERE state NOT IN ('closed', 'disqualified');
