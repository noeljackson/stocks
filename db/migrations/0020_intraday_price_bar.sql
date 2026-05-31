-- Intraday price bars for chart intervals finer than 1D.
-- Stored separately from price_bar so daily detectors do not accidentally
-- aggregate intraday rows as duplicate daily volume.
CREATE TABLE IF NOT EXISTS price_bar_intraday (
    symbol     text NOT NULL,
    interval   text NOT NULL,
    ts         timestamptz NOT NULL,
    open       numeric,
    high       numeric,
    low        numeric,
    close      numeric,
    volume     numeric,
    source     text NOT NULL DEFAULT 'fmp',
    fetched_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (symbol, interval, ts)
);

CREATE INDEX IF NOT EXISTS ix_price_bar_intraday_symbol_interval_ts
    ON price_bar_intraday(symbol, interval, ts DESC);
CREATE INDEX IF NOT EXISTS ix_price_bar_intraday_ts_brin
    ON price_bar_intraday USING brin(ts);
