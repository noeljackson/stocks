-- 0048_sec_filing_evidence_items.sql
--
-- Backfill SEC filing metadata from raw EDGAR ingest into the normalized
-- evidence stream. Raw ingest_event rows remain the point-in-time audit source;
-- evidence_item rows are the facts that context, thesis, and review loops can
-- consume directly.

DELETE FROM evidence_item
 WHERE source = 'edgar'
   AND kind = 'filing'
   AND (
       (source_id LIKE 'edgar_filing:%' AND source_id !~ '^edgar_filing:[^:]+:')
       OR (source_id LIKE 'edgar_event:%' AND source_id !~ '^edgar_event:[^:]+:')
   );

INSERT INTO evidence_item
    (symbol, kind, observed_at, source, source_id, source_ref,
     summary, strength, polarity, url, created_at)
SELECT
    e.symbol,
    'filing',
    COALESCE(e.source_ts, e.ingested_at),
    'edgar',
    CASE
        WHEN f.accession IS NULL THEN 'edgar_event:' || e.symbol || ':' || e.content_hash
        ELSE 'edgar_filing:' || e.symbol || ':' || f.accession
    END,
    jsonb_build_object(
        'table', 'ingest_event',
        'id', e.id,
        'source', 'edgar',
        'content_hash', e.content_hash,
        'kind', e.kind,
        'cik', e.payload->>'cik',
        'form', f.form,
        'accession', f.accession,
        'filing_date', f.filing_date,
        'primary_document', e.payload->>'primary_document',
        'url', f.url
    ),
    left(e.symbol || ' ' || f.form || ' filed' || COALESCE(' ' || f.filing_date, ''), 500),
    CASE
        WHEN regexp_replace(upper(f.form), '/A$', '') IN ('10-K', '10-Q', '8-K', 'S-1', 'S-3', 'S-4')
            THEN 0.65
        ELSE 0.45
    END,
    NULL,
    f.url,
    e.ingested_at
  FROM ingest_event e
 CROSS JOIN LATERAL (
    SELECT
        COALESCE(NULLIF(e.payload->>'form', ''), e.kind) AS form,
        NULLIF(e.payload->>'filing_date', '') AS filing_date,
        NULLIF(e.payload->>'accession', '') AS accession,
        NULLIF(e.payload->>'url', '') AS url
 ) f
 WHERE e.source = 'edgar'
   AND NULLIF(e.symbol, '') IS NOT NULL
ON CONFLICT (source, source_id) DO NOTHING;
