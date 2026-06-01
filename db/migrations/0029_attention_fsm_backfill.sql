-- 0029_attention_fsm_backfill.sql
--
-- 0028 added the operational FSM columns with conservative defaults. Reclassify
-- still-open rows that predate producer adoption so diagnostics and the
-- attention rail describe what owns the work.

UPDATE attention_item
   SET fsm_state = CASE
         WHEN kind = 'thesis_actionable' THEN 'actionable'
         WHEN kind = 'risk_review' AND severity = 'blocked' THEN 'blocked'
         WHEN kind = 'thesis_incomplete' AND source = 'consensus' THEN 'evaluating'
         WHEN kind IN ('thesis_incomplete', 'context_stale') AND source = 'context' THEN 'waiting_on_data'
         ELSE fsm_state
       END,
       owner = CASE
         WHEN kind = 'thesis_incomplete' AND source = 'consensus' THEN 'cognition'
         WHEN kind IN ('thesis_incomplete', 'context_stale') AND source = 'context' THEN 'source'
         ELSE owner
       END,
       state_reason = COALESCE(state_reason, kind)
 WHERE status = 'open'
   AND fsm_state <> 'operator_deferred';
