-- 0038_evidence_items.sql
--
-- First-class source facts. Context and thesis prose are interpretations;
-- evidence_item rows are the discrete observed facts they should trace back to.

CREATE TABLE IF NOT EXISTS evidence_item (
    id            bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    symbol        text NOT NULL,
    kind          text NOT NULL CHECK (kind IN (
                    'filing',
                    'estimate_revision',
                    'rating_change',
                    'news',
                    'price_action',
                    'regime',
                    'context_shift',
                    'crowd_sentiment',
                    'product_research'
                  )),
    observed_at   timestamptz NOT NULL,
    source        text NOT NULL,
    source_id     text NOT NULL,
    source_ref    jsonb NOT NULL DEFAULT '{}'::jsonb,
    summary       text NOT NULL,
    strength      double precision CHECK (strength IS NULL OR (strength >= 0.0 AND strength <= 1.0)),
    polarity      double precision CHECK (polarity IS NULL OR (polarity >= -1.0 AND polarity <= 1.0)),
    url           text,
    created_at    timestamptz NOT NULL DEFAULT now(),
    UNIQUE (source, source_id)
);

CREATE INDEX IF NOT EXISTS ix_evidence_item_symbol_at
    ON evidence_item(symbol, observed_at DESC);

CREATE INDEX IF NOT EXISTS ix_evidence_item_kind_at
    ON evidence_item(kind, observed_at DESC);

CREATE TABLE IF NOT EXISTS thesis_evidence (
    thesis_id     uuid NOT NULL REFERENCES thesis(thesis_id) ON DELETE CASCADE,
    evidence_id   bigint NOT NULL REFERENCES evidence_item(id) ON DELETE CASCADE,
    weight        double precision CHECK (weight IS NULL OR (weight >= 0.0 AND weight <= 1.0)),
    added_by      text NOT NULL DEFAULT 'system' CHECK (added_by IN ('system', 'user', 'llm')),
    created_at    timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (thesis_id, evidence_id)
);

INSERT INTO evidence_item
    (symbol, kind, observed_at, source, source_id, source_ref, summary,
     strength, polarity, url, created_at)
SELECT
    n.symbol,
    'news',
    n.published_at,
    n.source,
    'news_article:' || n.id::text,
    jsonb_build_object(
        'table', 'news_article',
        'id', n.id,
        'publisher', n.publisher,
        'sentiment', n.sentiment,
        'sentiment_source', n.sentiment_source
    ),
    left(n.title, 500),
    CASE n.sentiment_confidence
        WHEN 'high' THEN 0.9
        WHEN 'medium' THEN 0.65
        WHEN 'low' THEN 0.4
        ELSE 0.35
    END,
    n.sentiment_polarity,
    n.url,
    n.ingested_at
  FROM news_article n
ON CONFLICT (source, source_id) DO NOTHING;

INSERT INTO evidence_item
    (symbol, kind, observed_at, source, source_id, source_ref, summary,
     strength, polarity, url)
SELECT
    er.symbol,
    'estimate_revision',
    er.detected_at,
    'fmp_estimates',
    'estimate_revision:' || er.id::text,
    jsonb_build_object(
        'table', 'estimate_revision',
        'id', er.id,
        'fiscal_period_end', er.fiscal_period_end,
        'period_kind', er.period_kind,
        'prev_snapshot_id', er.prev_snapshot_id,
        'curr_snapshot_id', er.curr_snapshot_id,
        'eps_delta_pct', er.eps_delta_pct,
        'revenue_delta_pct', er.revenue_delta_pct,
        'direction', er.direction
    ),
    left(
      er.symbol || ' ' || er.period_kind || ' estimate revision ' || er.direction ||
      COALESCE(' EPS ' || round(er.eps_delta_pct::numeric, 1)::text || '%', '') ||
      COALESCE(' revenue ' || round(er.revenue_delta_pct::numeric, 1)::text || '%', ''),
      500
    ),
    LEAST(
      1.0,
      GREATEST(
        COALESCE(abs(er.eps_delta_pct), 0.0),
        COALESCE(abs(er.revenue_delta_pct), 0.0),
        CASE WHEN er.direction = 'initial' THEN 5.0 ELSE 0.0 END
      ) / 20.0
    ),
    CASE er.direction
        WHEN 'up' THEN 0.7
        WHEN 'down' THEN -0.7
        WHEN 'mixed' THEN 0.0
        ELSE NULL
    END,
    NULL
  FROM estimate_revision er
 WHERE er.direction <> 'initial'
ON CONFLICT (source, source_id) DO NOTHING;

DELETE FROM evidence_item
 WHERE source = 'fmp_estimates'
   AND source_ref->>'direction' = 'initial';

INSERT INTO evidence_item
    (symbol, kind, observed_at, source, source_id, source_ref, summary,
     strength, polarity, url, created_at)
SELECT
    ape.symbol,
    'rating_change',
    ape.published_at,
    'fmp_opinion',
    'analyst_price_target_event:' || ape.id::text,
    jsonb_build_object(
        'table', 'analyst_price_target_event',
        'id', ape.id,
        'analyst_name', ape.analyst_name,
        'analyst_company', ape.analyst_company,
        'price_target', ape.price_target,
        'adj_price_target', ape.adj_price_target,
        'price_when_posted', ape.price_when_posted
    ),
    left(ape.news_title, 500),
    0.6,
    CASE
        WHEN ape.adj_price_target IS NULL OR ape.price_when_posted IS NULL OR ape.price_when_posted = 0 THEN NULL
        WHEN ape.adj_price_target > ape.price_when_posted * 1.05 THEN 0.5
        WHEN ape.adj_price_target < ape.price_when_posted * 0.95 THEN -0.5
        ELSE 0.0
    END,
    ape.news_url,
    ape.ingested_at
  FROM analyst_price_target_event ape
ON CONFLICT (source, source_id) DO NOTHING;
