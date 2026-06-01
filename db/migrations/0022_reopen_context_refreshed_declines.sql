-- Context refresh does not supersede a declined thesis attempt. Only a
-- successful thesis draft or operator dismissal should resolve it.
WITH ranked AS (
    SELECT id,
           symbol,
           row_number() OVER (PARTITION BY symbol ORDER BY created_at DESC, id DESC) AS rn
      FROM attention_item
     WHERE kind = 'thesis_incomplete'
       AND status = 'resolved'
       AND resolution_kind = 'context_refreshed'
), reopen AS (
    SELECT r.id
      FROM ranked r
     WHERE r.rn = 1
       AND NOT EXISTS (
           SELECT 1
             FROM attention_item open_item
            WHERE open_item.kind = 'thesis_incomplete'
              AND open_item.status = 'open'
              AND open_item.symbol = r.symbol
       )
)
UPDATE attention_item ai
   SET status = 'open',
       resolved_at = NULL,
       resolution_kind = NULL,
       source_ref = source_ref || '{"reopened_by_migration":"0022_reopen_context_refreshed_declines"}'::jsonb
  FROM reopen
 WHERE ai.id = reopen.id;
