-- 0040_filing_metadata_source_tasks.sql
--
-- EDGAR submissions are a separate freshness contract from XBRL company facts.
-- XBRL answers "do we have structured fundamentals"; EDGAR answers "have we
-- checked recent SEC filing metadata for this symbol inside the brain SLA".

WITH counts AS (
    SELECT t.symbol,
           (SELECT count(*)
              FROM ingest_event e
             WHERE e.symbol = t.symbol
               AND e.source = 'edgar') AS filing_events,
           (SELECT max(e.ingested_at)
              FROM ingest_event e
             WHERE e.symbol = t.symbol
               AND e.source = 'edgar') AS filing_event_last_ingested_at
      FROM ticker t
     WHERE t.status = 'active'
), requirement_rows AS (
    INSERT INTO evidence_requirement
        (symbol, requirement_key, source_type, reason, priority, blocking_state,
         next_retry_at, source_ref, satisfied_at)
    SELECT symbol,
           'filing_metadata',
           'filings',
           'Need recent SEC submission metadata to catch 8-K, 10-Q, and 10-K events between slower fundamental fact refreshes.',
           'medium',
           CASE WHEN filing_events > 0 THEN 'satisfied' ELSE 'missing' END,
           CASE WHEN filing_events > 0 THEN NULL ELSE now() END,
           jsonb_build_object(
               'counts', jsonb_build_object(
                   'filing_events', filing_events,
                   'filing_event_last_ingested_at', filing_event_last_ingested_at
               ),
               'fetch_actions', jsonb_build_array('sec_edgar_submissions'),
               'backfilled_by', '0040_filing_metadata_source_tasks'
           ),
           CASE WHEN filing_events > 0 THEN now() ELSE NULL END
      FROM counts
    ON CONFLICT (symbol, requirement_key) DO UPDATE SET
        source_type = EXCLUDED.source_type,
        reason = EXCLUDED.reason,
        priority = EXCLUDED.priority,
        blocking_state = CASE
            WHEN evidence_requirement.blocking_state = 'fetching'
             AND EXCLUDED.blocking_state <> 'satisfied'
            THEN 'fetching'
            ELSE EXCLUDED.blocking_state
        END,
        next_retry_at = CASE
            WHEN EXCLUDED.blocking_state = 'satisfied' THEN NULL
            ELSE COALESCE(evidence_requirement.next_retry_at, now())
        END,
        source_ref = evidence_requirement.source_ref || EXCLUDED.source_ref,
        satisfied_at = CASE
            WHEN EXCLUDED.blocking_state = 'satisfied'
            THEN COALESCE(evidence_requirement.satisfied_at, now())
            ELSE NULL
        END,
        updated_at = now()
    RETURNING symbol
)
INSERT INTO source_task
    (source_type, requirement_key, action, scope, target_id, provider, limiter_key,
     state, priority, due_at, attempts, next_retry_at, last_error, source_ref)
SELECT 'filings',
       'filing_metadata',
       'sec_edgar_submissions',
       'symbol',
       c.symbol,
       'sec',
       'sec',
       CASE
           WHEN c.filing_events > 0
            AND c.filing_event_last_ingested_at >= now() - interval '30 minutes'
           THEN 'satisfied'
           ELSE 'queued'
       END,
       'medium',
       CASE
           WHEN c.filing_events > 0
            AND c.filing_event_last_ingested_at >= now() - interval '30 minutes'
           THEN c.filing_event_last_ingested_at + interval '30 minutes'
           ELSE now()
       END,
       0,
       NULL,
       NULL,
       jsonb_build_object(
           'created_by', '0040_filing_metadata_source_tasks',
           'evidence_counts', jsonb_build_object(
               'filing_events', c.filing_events,
               'filing_event_last_ingested_at', c.filing_event_last_ingested_at
           )
       )
  FROM counts c
  JOIN requirement_rows rr ON rr.symbol = c.symbol
ON CONFLICT (scope, target_id, requirement_key, action) DO UPDATE SET
    source_type = EXCLUDED.source_type,
    provider = EXCLUDED.provider,
    limiter_key = EXCLUDED.limiter_key,
    state = EXCLUDED.state,
    priority = EXCLUDED.priority,
    due_at = EXCLUDED.due_at,
    attempts = GREATEST(source_task.attempts, EXCLUDED.attempts),
    next_retry_at = EXCLUDED.next_retry_at,
    last_error = EXCLUDED.last_error,
    source_ref = source_task.source_ref || EXCLUDED.source_ref,
    updated_at = now();
