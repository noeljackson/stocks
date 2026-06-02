-- 0046_discovery_price_action_evidence.sql
--
-- Backfill discovery signal firings into first-class evidence. Research
-- nominations are not price action; they stay represented by discovery/attention
-- review state instead of pretending to be market evidence.

INSERT INTO evidence_item
    (symbol, kind, observed_at, source, source_id, source_ref,
     summary, strength, polarity, url, created_at)
SELECT
    dc.symbol,
    'price_action',
    dc.proposed_at,
    'discovery',
    'discovery_candidate:' || dc.id::text,
    jsonb_build_object(
        'table', 'discovery_candidate',
        'id', dc.id,
        'signal_name', dc.signal_name,
        'signal_value', dc.signal_value,
        'status', dc.status,
        'config_version', dc.config_version
    ),
    left(dc.reasoning, 500),
    LEAST(1.0, GREATEST(0.25, COALESCE(abs(dc.signal_value), 0.0) / 100.0)),
    CASE dc.signal_name
        WHEN 'early_accumulation' THEN 0.45
        WHEN 'breakout_confirmation' THEN 0.65
        WHEN 'extended_momentum' THEN 0.25
        WHEN 'consensus_arrival' THEN 0.0
        WHEN 'possible_exhaustion' THEN -0.65
        WHEN 'existing_thesis_trigger' THEN 0.0
        ELSE 0.0
    END,
    NULL,
    dc.proposed_at
  FROM discovery_candidate dc
 WHERE dc.signal_name <> 'research_nomination'
ON CONFLICT (source, source_id) DO NOTHING;
