-- 0024 — canonicalize proactive discovery nominations (#142).
--
-- `pool_inspection` was the first name for proactive discovery-pool review.
-- The product concept is now `research_nomination`: a reasoned nomination for
-- the monitored universe, not a trade signal. Normalize stored rows once so
-- application code only has to understand the canonical vocabulary.

UPDATE discovery_candidate
   SET signal_name = 'research_nomination',
       config_version = CASE
           WHEN config_version IS NULL OR config_version = 'pool_inspection:v1'
           THEN 'research_nomination:v1'
           ELSE config_version
       END,
       reasoning = CASE
           WHEN reasoning IS NULL THEN reasoning
           ELSE replace(
               replace(
                   replace(
                       reasoning,
                       'Proactive research inspection:',
                       'Research nomination:'
                   ),
                   'Queued so context/thesis can inspect relevance, not because a trade signal fired.',
                   'Confirming adds it to the monitored universe/watchlists and runs context/thesis; this is not a trade signal.'
               ),
               'Available data:',
               'Available evidence:'
           )
       END
 WHERE signal_name = 'pool_inspection';

UPDATE attention_item ai
   SET title = COALESCE(ai.symbol, dc.symbol) || ': research nomination',
       reason = CASE
           WHEN ai.reason IS NULL THEN ai.reason
           ELSE replace(
               replace(
                   replace(
                       ai.reason,
                       'Proactive research inspection:',
                       'Research nomination:'
                   ),
                   'Queued so context/thesis can inspect relevance, not because a trade signal fired.',
                   'Confirming adds it to the monitored universe/watchlists and runs context/thesis; this is not a trade signal.'
               ),
               'Available data:',
               'Available evidence:'
           )
       END,
       source_ref = ai.source_ref
           || jsonb_build_object(
               'interpretation_kind', 'research_nomination',
               'config_version', 'research_nomination:v1'
           )
  FROM discovery_candidate dc
 WHERE ai.candidate_id = dc.id
   AND ai.kind = 'candidate_review'
   AND dc.signal_name = 'research_nomination';

WITH ranked AS (
  SELECT id,
         row_number() OVER (
           PARTITION BY symbol, signal_name
           ORDER BY proposed_at DESC
         ) AS rk
    FROM discovery_candidate
   WHERE status = 'proposed'
     AND signal_name = 'research_nomination'
)
UPDATE discovery_candidate
   SET status = 'superseded'
 WHERE id IN (SELECT id FROM ranked WHERE rk > 1);

UPDATE attention_item ai
   SET status = 'dismissed',
       resolved_at = COALESCE(resolved_at, now()),
       resolution_kind = COALESCE(resolution_kind, 'superseded_by_research_nomination_rename')
  FROM discovery_candidate dc
 WHERE ai.candidate_id = dc.id
   AND ai.kind = 'candidate_review'
   AND ai.status = 'open'
   AND dc.status = 'superseded'
   AND dc.signal_name = 'research_nomination';
