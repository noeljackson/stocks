-- 0025 — enforce one canonical open thesis per symbol (#141).
--
-- Older smoketest/dev runs could leave several non-closed theses open for the
-- same symbol. Keep the most recently updated thesis as canonical, retire the
-- rest through the normal `disqualified` terminal state, and add a partial
-- unique index so future code paths cannot reintroduce duplicate open theses.

DROP TABLE IF EXISTS _open_thesis_retirements;

CREATE TEMP TABLE _open_thesis_retirements AS
WITH ranked AS (
  SELECT thesis_id,
         symbol,
         state AS from_state,
         row_number() OVER (
           PARTITION BY symbol
           ORDER BY updated_at DESC, created_at DESC, thesis_id DESC
         ) AS rk,
         first_value(thesis_id) OVER (
           PARTITION BY symbol
           ORDER BY updated_at DESC, created_at DESC, thesis_id DESC
         ) AS canonical_thesis_id
    FROM thesis
   WHERE state NOT IN ('closed', 'disqualified')
)
SELECT thesis_id, symbol, from_state, canonical_thesis_id
  FROM ranked
 WHERE rk > 1;

UPDATE thesis t
   SET state = 'disqualified',
       updated_at = now()
  FROM _open_thesis_retirements r
 WHERE t.thesis_id = r.thesis_id
   AND t.state NOT IN ('closed', 'disqualified');

INSERT INTO thesis_state_history (thesis_id, from_state, to_state, rationale)
SELECT r.thesis_id,
       r.from_state,
       'disqualified',
       'Retired duplicate open thesis; canonical thesis is ' || r.canonical_thesis_id::text
  FROM _open_thesis_retirements r
 WHERE NOT EXISTS (
       SELECT 1
         FROM thesis_state_history h
        WHERE h.thesis_id = r.thesis_id
          AND h.to_state = 'disqualified'
          AND h.rationale = 'Retired duplicate open thesis; canonical thesis is ' || r.canonical_thesis_id::text
 );

INSERT INTO thesis_version_history
    (thesis_id, version, diff, rationale, weakens_invalidation)
SELECT r.canonical_thesis_id,
       t.version,
       jsonb_build_object(
           'event', 'duplicate_open_thesis_retired',
           'retired_thesis_id', r.thesis_id,
           'retired_from_state', r.from_state,
           'symbol', r.symbol
       ),
       'Retired duplicate open thesis ' || r.thesis_id::text,
       false
  FROM _open_thesis_retirements r
  JOIN thesis t ON t.thesis_id = r.canonical_thesis_id;

UPDATE attention_item ai
   SET status = 'dismissed',
       resolved_at = COALESCE(resolved_at, now()),
       resolution_kind = COALESCE(resolution_kind, 'superseded_by_canonical_thesis')
  FROM _open_thesis_retirements r
 WHERE ai.thesis_id = r.thesis_id
   AND ai.status = 'open';

CREATE UNIQUE INDEX IF NOT EXISTS ux_thesis_one_open_per_symbol
    ON thesis(symbol)
 WHERE state NOT IN ('closed', 'disqualified');

DROP TABLE IF EXISTS _open_thesis_retirements;
