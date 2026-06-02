-- 0043_research_nomination_review_budget.sql
--
-- Research nominations are proactive review suggestions, not urgent market
-- alerts. Keep the best 100 open so the operator has a usable queue, and
-- supersede the rest with their attention items closed.

WITH ranked AS (
    SELECT id,
           row_number() OVER (
               ORDER BY proposed_tier ASC,
                        COALESCE(signal_value, 0) DESC,
                        COALESCE(domain_fit, 0) DESC,
                        proposed_at DESC,
                        symbol ASC
           ) AS rn
      FROM discovery_candidate
     WHERE status = 'proposed'
       AND signal_name = 'research_nomination'
),
stale AS (
    UPDATE discovery_candidate dc
       SET status = 'superseded',
           decided_at = COALESCE(decided_at, now())
      FROM ranked r
     WHERE dc.id = r.id
       AND r.rn > 100
 RETURNING dc.id, dc.symbol, dc.signal_name
),
matched AS (
    SELECT ai.id, ai.fsm_state, stale.symbol, stale.signal_name
      FROM attention_item ai
      JOIN stale ON ai.candidate_id = stale.id
     WHERE ai.kind = 'candidate_review'
       AND ai.status = 'open'
     FOR UPDATE OF ai
),
updated AS (
    UPDATE attention_item ai
       SET status = 'dismissed',
           fsm_state = 'dismissed',
           owner = 'system',
           resolved_at = COALESCE(resolved_at, now()),
           resolution_kind = 'superseded_by_research_nomination_review_budget',
           resolution_ref = jsonb_build_object(
               'symbol', matched.symbol,
               'signal_name', matched.signal_name,
               'keep', 100
           ),
           next_retry_at = NULL,
           resurface_at = NULL,
           state_reason = 'superseded_by_research_nomination_review_budget'
      FROM matched
     WHERE ai.id = matched.id
 RETURNING ai.id,
           matched.fsm_state AS from_state,
           ai.fsm_state AS to_state,
           ai.owner,
           ai.state_reason,
           ai.next_retry_at,
           ai.resurface_at,
           ai.resolution_ref
),
inserted AS (
    INSERT INTO attention_state_history
         (attention_id, from_state, to_state, owner, reason,
          next_retry_at, resurface_at, source_ref)
    SELECT id, from_state, to_state, owner, state_reason,
           next_retry_at, resurface_at, resolution_ref
      FROM updated
 RETURNING 1
)
SELECT COUNT(*) AS superseded_research_nominations
  FROM stale;
