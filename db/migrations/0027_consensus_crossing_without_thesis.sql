-- 0027_consensus_crossing_without_thesis.sql
--
-- Consensus threshold crossings are not thesis lifecycle events unless a
-- thesis exists. Older runs emitted thesis.updated for symbols that only had a
-- consensus score crossing, which made the UI claim a thesis had updated while
-- no thesis row existed.

UPDATE alert
   SET symbol = COALESCE(symbol, payload->>'symbol'),
       thesis_id = COALESCE(
           thesis_id,
           CASE
               WHEN payload ? 'thesis_id' AND payload->>'thesis_id' <> ''
                    AND payload->>'thesis_id' ~* '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
               THEN (
                   SELECT th.thesis_id
                     FROM thesis th
                    WHERE th.thesis_id = (payload->>'thesis_id')::uuid
               )
               ELSE NULL
           END
       )
 WHERE kind = 'state_transition'
   AND payload ? 'symbol';

WITH orphan_consensus AS (
    SELECT a.id,
           a.payload->>'symbol' AS symbol,
           NULLIF(a.payload->>'score', '')::double precision AS score,
           a.payload AS payload
      FROM alert a
     WHERE a.kind = 'state_transition'
       AND a.thesis_id IS NULL
       AND a.payload ? 'measurement_crossed'
       AND a.payload ? 'symbol'
       AND NOT EXISTS (
             SELECT 1
               FROM thesis th
              WHERE th.symbol = a.payload->>'symbol'
                AND th.state NOT IN ('closed', 'disqualified')
       )
)
INSERT INTO attention_item
    (kind, symbol, severity, title, reason, source, source_ref)
SELECT 'thesis_incomplete',
       symbol,
       'review',
       symbol || ' needs a thesis after consensus crossing',
       'Consensus score ' || round(score::numeric, 1) ||
       ' crossed the measurement threshold, but ' || symbol ||
       ' has no open thesis. Run cognition to create or decline a current standing view.',
       'consensus',
       jsonb_build_object(
           'trigger', 'consensus_crossing_without_thesis',
           'score', score,
           'backfilled_from_alert_id', id,
           'payload', payload
       )
  FROM orphan_consensus
 WHERE symbol IS NOT NULL
ON CONFLICT DO NOTHING;

WITH orphan_consensus AS (
    SELECT a.id
      FROM alert a
     WHERE a.kind = 'state_transition'
       AND a.thesis_id IS NULL
       AND a.payload ? 'measurement_crossed'
       AND a.payload ? 'symbol'
       AND NOT EXISTS (
             SELECT 1
               FROM thesis th
              WHERE th.symbol = a.payload->>'symbol'
                AND th.state NOT IN ('closed', 'disqualified')
       )
)
UPDATE alert a
   SET acknowledged = true
  FROM orphan_consensus o
 WHERE a.id = o.id;
