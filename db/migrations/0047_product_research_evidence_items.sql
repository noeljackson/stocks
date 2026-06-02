-- 0047_product_research_evidence_items.sql
--
-- Backfill product/theme research retrievals into the normalized evidence
-- stream so context and thesis loops can consume them alongside price,
-- news, estimates, ratings, and filings.

INSERT INTO evidence_item
    (symbol, kind, observed_at, source, source_id, source_ref, summary,
     strength, polarity, url, created_at)
SELECT
    re.symbol,
    'product_research',
    COALESCE(re.published_at, re.retrieved_at),
    'web_research',
    'research_evidence:' || re.id::text,
    jsonb_build_object(
        'table', 'research_evidence',
        'id', re.id,
        'provider', re.provider,
        'query', re.query,
        'publisher', re.publisher,
        'credibility', re.credibility,
        'source_type', re.source_type,
        'tags', re.tags
    ) || COALESCE(re.source_ref, '{}'::jsonb),
    left(re.title, 500),
    CASE re.credibility
        WHEN 'primary' THEN 0.9
        WHEN 'credible_media' THEN 0.75
        WHEN 'industry' THEN 0.6
        ELSE 0.4
    END,
    NULL,
    re.url,
    re.retrieved_at
  FROM research_evidence re
ON CONFLICT (source, source_id) DO NOTHING;
