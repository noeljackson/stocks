-- 0015_crowd_sentiment.sql — macro crowd sentiment markers (#20).
--
-- Free-first strategy per docs/DATA_SOURCES.md §5. Each source emits one
-- (source, metric, observed_at) tuple per pull. Stored long for query
-- flexibility — consensus.retail_attention component reads latest values
-- per source and composes into a single retail-attention score.

CREATE TABLE IF NOT EXISTS crowd_sentiment (
    id          bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    source      text NOT NULL,           -- 'cboe_pcr' | 'cboe_vix' | 'cnn_fng' | 'aaii' | ...
    metric      text NOT NULL,           -- 'equity_pcr' | 'vix_close' | 'fng_score' | 'bullish_pct' | ...
    value       double precision NOT NULL,
    observed_at date NOT NULL,           -- the trading day or survey date the value pertains to
    raw         jsonb,                   -- vendor-specific extras for audit
    ingested_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (source, metric, observed_at)
);
CREATE INDEX IF NOT EXISTS ix_crowd_sentiment_lookup
    ON crowd_sentiment(source, metric, observed_at DESC);
