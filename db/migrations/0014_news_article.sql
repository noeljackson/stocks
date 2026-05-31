-- 0014_news_article.sql — news ingest from multiple vendors + universal
-- sentiment classifier (#19).
--
-- Architecture (per docs/DATA_SOURCES.md):
-- Multiple vendors (Massive, FMP, future RSS/Twitter) emit articles. We
-- dedupe by URL when present, else by (symbol, title, published_at).
-- Sentiment comes from one of:
--   - upstream: vendor served it (Massive's insights[].sentiment)
--   - llm:      our own classifier scored it via prompts/score-sentiment.md
--   - none:     not scored yet (transient — service backfills these)
--
-- Append-mostly: rows insert on discovery; sentiment fields update once,
-- when the classifier finishes.

CREATE TABLE IF NOT EXISTS news_article (
    id                  bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    symbol              text NOT NULL REFERENCES ticker(symbol),
    title               text NOT NULL,
    body                text,
    url                 text,
    publisher           text,
    published_at        timestamptz NOT NULL,
    source              text NOT NULL CHECK (source IN ('massive', 'fmp', 'rss', 'manual')),
    -- Sentiment classification.
    sentiment           text CHECK (sentiment IN ('positive', 'neutral', 'negative')),
    sentiment_polarity  double precision,            -- -1.0 .. 1.0
    sentiment_confidence text CHECK (sentiment_confidence IN ('low','medium','high')),
    sentiment_source    text CHECK (sentiment_source IN ('upstream','llm','manual')),
    sentiment_rationale text,
    -- Audit when LLM-scored.
    prompt_name         text,
    prompt_hash         text,
    scored_at           timestamptz,
    -- Provenance.
    ingested_at         timestamptz NOT NULL DEFAULT now()
);

-- Dedup index: URL is the strongest identity; when null, fall back to
-- (symbol, title, published_at) which is good enough for vendor de-dupe.
CREATE UNIQUE INDEX IF NOT EXISTS ux_news_article_url
    ON news_article(url) WHERE url IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS ux_news_article_no_url
    ON news_article(symbol, title, published_at) WHERE url IS NULL;

CREATE INDEX IF NOT EXISTS ix_news_article_symbol_at
    ON news_article(symbol, published_at DESC);
CREATE INDEX IF NOT EXISTS ix_news_article_unscored
    ON news_article(id) WHERE sentiment IS NULL;
