-- 0026_backfill_evidence_requirements.sql
--
-- Existing active tickers may predate the evidence_requirement queue. Backfill
-- the deterministic evidence checklist so the UI and cognition sweep can see
-- both missing and already-satisfied inputs immediately.

WITH counts AS (
    SELECT t.symbol,
           (SELECT count(*) FROM price_bar p WHERE p.symbol = t.symbol) AS price_bars,
           (SELECT count(*) FROM company_fact f WHERE f.symbol = t.symbol) AS company_facts,
           (SELECT count(*) FROM news_article n
             WHERE n.symbol = t.symbol
               AND n.published_at > now() - interval '30 days') AS recent_news,
           (SELECT count(*) FROM estimate_snapshot e WHERE e.symbol = t.symbol) AS estimate_snapshots
      FROM ticker t
     WHERE t.status = 'active'
), requirements AS (
    SELECT *
      FROM (VALUES
        ('price_history', 'price', 'blocking',
         'Need daily OHLCV bars before evaluating technical setup or context freshness.',
         ARRAY['fmp_price_backfill']::text[]),
        ('company_facts', 'fundamentals', 'high',
         'Need SEC/XBRL company facts before making fundamental claims.',
         ARRAY['sec_company_tickers_cik_lookup', 'sec_companyfacts_xbrl']::text[]),
        ('recent_news', 'news', 'high',
         'Need recent narrative evidence before deciding whether the market has new information.',
         ARRAY['fmp_news', 'massive_news', 'llm_sentiment_scoring']::text[]),
        ('analyst_estimates', 'estimates', 'high',
         'Need analyst estimate snapshots before evaluating revision/consensus drift.',
         ARRAY['fmp_analyst_estimates']::text[])
      ) AS r(requirement_key, source_type, priority, reason, fetch_actions)
), assessed AS (
    SELECT c.symbol,
           r.requirement_key,
           r.source_type,
           r.priority,
           r.reason,
           r.fetch_actions,
           c.price_bars,
           c.company_facts,
           c.recent_news,
           c.estimate_snapshots,
           CASE r.requirement_key
                WHEN 'price_history' THEN c.price_bars > 0
                WHEN 'company_facts' THEN c.company_facts > 0
                WHEN 'recent_news' THEN c.recent_news > 0
                WHEN 'analyst_estimates' THEN c.estimate_snapshots > 0
                ELSE false
           END AS satisfied
      FROM counts c
     CROSS JOIN requirements r
)
INSERT INTO evidence_requirement
    (symbol, requirement_key, source_type, reason, priority, blocking_state,
     next_retry_at, source_ref, satisfied_at)
SELECT symbol,
       requirement_key,
       source_type,
       reason,
       priority,
       CASE WHEN satisfied THEN 'satisfied' ELSE 'missing' END,
       CASE WHEN satisfied THEN NULL ELSE now() END,
       jsonb_build_object(
           'counts', jsonb_build_object(
               'price_bars', price_bars,
               'company_facts', company_facts,
               'recent_news', recent_news,
               'estimate_snapshots', estimate_snapshots
           ),
           'fetch_actions', to_jsonb(fetch_actions),
           'backfilled_by', '0026_backfill_evidence_requirements'
       ),
       CASE WHEN satisfied THEN now() ELSE NULL END
  FROM assessed
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
    source_ref = EXCLUDED.source_ref,
    satisfied_at = CASE
        WHEN EXCLUDED.blocking_state = 'satisfied'
        THEN COALESCE(evidence_requirement.satisfied_at, now())
        ELSE NULL
    END,
    updated_at = now();
