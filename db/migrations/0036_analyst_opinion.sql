-- 0036_analyst_opinion.sql
--
-- Analyst opinion is separate from analyst estimates. Estimates answer
-- "what are sell-side financial forecasts"; opinion answers "what price/rating
-- consensus is already visible to the market?"

CREATE TABLE IF NOT EXISTS analyst_price_target_snapshot (
    id                bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    symbol            text NOT NULL,
    target_high       double precision,
    target_low        double precision,
    target_consensus  double precision,
    target_median     double precision,
    snapshot_at       timestamptz NOT NULL DEFAULT now(),
    raw               jsonb NOT NULL,
    UNIQUE (symbol, snapshot_at)
);

CREATE INDEX IF NOT EXISTS ix_analyst_price_target_snapshot_symbol_at
    ON analyst_price_target_snapshot(symbol, snapshot_at DESC);

CREATE TABLE IF NOT EXISTS analyst_recommendation_snapshot (
    id            bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    symbol        text NOT NULL,
    as_of_date    date,
    strong_buy    int,
    buy           int,
    hold          int,
    sell          int,
    strong_sell   int,
    snapshot_at   timestamptz NOT NULL DEFAULT now(),
    raw           jsonb NOT NULL,
    UNIQUE (symbol, as_of_date, snapshot_at)
);

CREATE INDEX IF NOT EXISTS ix_analyst_recommendation_snapshot_symbol_at
    ON analyst_recommendation_snapshot(symbol, snapshot_at DESC);

CREATE TABLE IF NOT EXISTS analyst_price_target_event (
    id                 bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    symbol             text NOT NULL,
    published_at       timestamptz NOT NULL,
    news_url           text,
    news_title         text NOT NULL,
    analyst_name       text,
    analyst_company    text,
    price_target       double precision,
    adj_price_target   double precision,
    price_when_posted  double precision,
    news_publisher     text,
    news_base_url      text,
    raw                jsonb NOT NULL,
    ingested_at        timestamptz NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX IF NOT EXISTS ux_analyst_price_target_event_url
    ON analyst_price_target_event(symbol, news_url)
    WHERE news_url IS NOT NULL;

CREATE UNIQUE INDEX IF NOT EXISTS ux_analyst_price_target_event_no_url
    ON analyst_price_target_event(symbol, news_title, published_at)
    WHERE news_url IS NULL;

CREATE INDEX IF NOT EXISTS ix_analyst_price_target_event_symbol_at
    ON analyst_price_target_event(symbol, published_at DESC);
