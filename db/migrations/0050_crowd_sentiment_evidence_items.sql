-- 0050_crowd_sentiment_evidence_items.sql
--
-- Normalize CBOE put/call and VIX observations into evidence_item so macro
-- crowd sentiment participates in the same fact stream as news, filings,
-- estimates, ratings, research, discovery price action, and regime changes.

INSERT INTO evidence_item
    (symbol, kind, observed_at, source, source_id, source_ref, summary,
     strength, polarity, created_at, updated_at)
SELECT
    'MARKET',
    'crowd_sentiment',
    cs.observed_at::timestamptz,
    'cboe',
    'crowd_sentiment:' || cs.source || ':' || cs.metric || ':' || cs.observed_at::text,
    jsonb_build_object(
        'table', 'crowd_sentiment',
        'source', cs.source,
        'metric', cs.metric,
        'value', cs.value,
        'observed_at', cs.observed_at
    ),
    CASE cs.metric
        WHEN 'equity_pcr' THEN 'CBOE equity put/call ratio ' || to_char(cs.value, 'FM999999990.00')
        WHEN 'vix_close' THEN 'CBOE VIX close ' || to_char(cs.value, 'FM999999990.00')
        ELSE 'CBOE ' || cs.metric || ' ' || to_char(cs.value, 'FM999999990.00')
    END,
    CASE cs.metric
        WHEN 'equity_pcr' THEN LEAST(1.0, abs(cs.value - 0.70) / 0.40)
        WHEN 'vix_close' THEN LEAST(1.0, abs(cs.value - 18.0) / 20.0)
        ELSE NULL
    END,
    CASE
        WHEN cs.metric = 'equity_pcr' AND cs.value >= 0.90 THEN -0.5
        WHEN cs.metric = 'equity_pcr' AND cs.value <= 0.55 THEN 0.3
        WHEN cs.metric = 'equity_pcr' THEN 0.0
        WHEN cs.metric = 'vix_close' AND cs.value >= 25.0 THEN -0.6
        WHEN cs.metric = 'vix_close' AND cs.value <= 14.0 THEN 0.3
        WHEN cs.metric = 'vix_close' THEN 0.0
        ELSE NULL
    END,
    cs.ingested_at,
    cs.ingested_at
  FROM crowd_sentiment cs
 WHERE cs.source IN ('cboe_pcr', 'cboe_vix')
ON CONFLICT (source, source_id) DO UPDATE SET
    source_ref = EXCLUDED.source_ref,
    summary = EXCLUDED.summary,
    strength = EXCLUDED.strength,
    polarity = EXCLUDED.polarity,
    updated_at = CASE
        WHEN evidence_item.source_ref IS DISTINCT FROM EXCLUDED.source_ref
          OR evidence_item.summary IS DISTINCT FROM EXCLUDED.summary
          OR evidence_item.strength IS DISTINCT FROM EXCLUDED.strength
          OR evidence_item.polarity IS DISTINCT FROM EXCLUDED.polarity
        THEN now()
        ELSE evidence_item.updated_at
    END;
