-- 0056_analyst_rating_events.sql
--
-- FMP `grades-latest-news` is a global sell-side rating event feed. This is
-- distinct from price-target news: it captures upgrades, downgrades,
-- initiations, and reiterations as discrete catalyst evidence.

CREATE TABLE IF NOT EXISTS analyst_rating_event (
    id                 bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    symbol             text NOT NULL,
    published_at       timestamptz NOT NULL,
    news_url           text,
    news_title         text NOT NULL,
    news_base_url      text,
    news_publisher     text,
    grading_company    text,
    action             text,
    new_grade          text,
    previous_grade     text,
    price_when_posted  double precision,
    raw                jsonb NOT NULL,
    ingested_at        timestamptz NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX IF NOT EXISTS ux_analyst_rating_event_url
    ON analyst_rating_event(symbol, news_url)
    WHERE news_url IS NOT NULL;

CREATE UNIQUE INDEX IF NOT EXISTS ux_analyst_rating_event_no_url
    ON analyst_rating_event(symbol, news_title, published_at)
    WHERE news_url IS NULL;

CREATE INDEX IF NOT EXISTS ix_analyst_rating_event_symbol_at
    ON analyst_rating_event(symbol, published_at DESC);
