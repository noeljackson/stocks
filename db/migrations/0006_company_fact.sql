-- 0006_company_fact.sql — structured financial facts extracted from SEC XBRL (#32).
--
-- The EDGAR adapter (src/ingest/edgar.rs) records submission METADATA — that
-- a 10-Q was filed, when, with which accession number. It never fetched the
-- filing content, so the context maintainer kept NULLing revenue_yoy_pct etc.
--
-- The XBRL adapter (src/ingest/xbrl.rs) fills the gap by hitting SEC's
-- structured-data endpoint (`/api/xbrl/companyfacts/CIK<N>.json`) and
-- extracting the standard us-gaap concepts per filing. One row per
-- (company, concept, period_end, accession) — unique key prevents dups
-- across re-polls. Append-only in spirit; the SEC restates rarely but when
-- it does the unique constraint catches it (we keep both rows since they're
-- different accessions = different filings).

CREATE TABLE IF NOT EXISTS company_fact (
    id              bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    symbol          text NOT NULL,
    cik             text NOT NULL,
    taxonomy        text NOT NULL,        -- 'us-gaap' | 'ifrs-full' | 'dei'
    concept         text NOT NULL,        -- 'Revenues' | 'GrossProfit' | …
    period_end      date NOT NULL,
    period_start    date,                 -- NULL for instant-as-of (e.g. cash on hand)
    value           numeric NOT NULL,
    unit            text NOT NULL,        -- 'USD' | 'shares' | 'USD/shares'
    form            text,                 -- '10-K' | '10-Q' | '8-K' …
    fiscal_year     int,
    fiscal_period   text,                 -- 'FY' | 'Q1' | 'Q2' | 'Q3' | 'Q4'
    accession       text,
    filed_at        date,
    ingested_at     timestamptz NOT NULL DEFAULT now()
);
-- Postgres unique constraints can't wrap expressions inline, so we use a
-- unique INDEX over (... , COALESCE(accession, '')) instead. Same effect.
CREATE UNIQUE INDEX IF NOT EXISTS ux_company_fact_dedup
    ON company_fact(symbol, taxonomy, concept, period_end, COALESCE(accession, ''));
CREATE INDEX IF NOT EXISTS ix_company_fact_lookup
    ON company_fact(symbol, concept, period_end DESC);
CREATE INDEX IF NOT EXISTS ix_company_fact_filed_brin
    ON company_fact USING brin(filed_at);

COMMENT ON TABLE company_fact IS
'Structured financial facts from SEC XBRL (#32). Source: '
'https://data.sec.gov/api/xbrl/companyfacts/CIK<N>.json. Unique key on '
'(symbol, taxonomy, concept, period_end, accession) catches re-polls.';
