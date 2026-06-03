-- 0057_fmp_profile_calendar.sql
--
-- FMP profile and earnings data fill the metadata/catalyst gap: profile gives
-- market cap, sector, industry, exchange, and issuer identity; earnings gives
-- upcoming/recent report dates plus EPS/revenue actuals and estimates.

ALTER TABLE evidence_item
    DROP CONSTRAINT IF EXISTS evidence_item_kind_check;

ALTER TABLE evidence_item
    ADD CONSTRAINT evidence_item_kind_check CHECK (kind IN (
        'filing',
        'estimate_revision',
        'rating_change',
        'news',
        'price_action',
        'regime',
        'context_shift',
        'crowd_sentiment',
        'product_research',
        'earnings_calendar'
    ));

CREATE TABLE IF NOT EXISTS company_profile (
    symbol                 text PRIMARY KEY,
    company_name           text,
    currency               text,
    market_cap             double precision,
    beta                   double precision,
    exchange               text,
    exchange_full_name     text,
    industry               text,
    sector                 text,
    country                text,
    website                text,
    description            text,
    ceo                    text,
    full_time_employees    bigint,
    ipo_date               date,
    is_etf                 boolean,
    is_adr                 boolean,
    is_fund                boolean,
    is_actively_trading    boolean,
    raw                    jsonb NOT NULL,
    profile_at             timestamptz NOT NULL DEFAULT now(),
    updated_at             timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS ix_company_profile_sector_industry
    ON company_profile(sector, industry);

CREATE TABLE IF NOT EXISTS earnings_calendar_event (
    id                    bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    symbol                text NOT NULL,
    report_date           date NOT NULL,
    eps_actual            double precision,
    eps_estimated         double precision,
    revenue_actual        double precision,
    revenue_estimated     double precision,
    last_updated          date,
    raw                   jsonb NOT NULL,
    ingested_at           timestamptz NOT NULL DEFAULT now(),
    updated_at            timestamptz NOT NULL DEFAULT now(),
    UNIQUE(symbol, report_date)
);

CREATE INDEX IF NOT EXISTS ix_earnings_calendar_event_symbol_date
    ON earnings_calendar_event(symbol, report_date DESC);
