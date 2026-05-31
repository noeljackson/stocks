-- 0013_estimate_revisions.sql — analyst consensus snapshots + revision events (#18).
--
-- Architecture (per docs/DATA_SOURCES.md decision log 2026-05-31):
-- The FMP /stable/analyst-estimates endpoint returns a current snapshot of
-- consensus per fiscal period (revenueAvg/High/Low, ebitda, ebit, netIncome,
-- epsAvg/High/Low, numAnalystsRevenue, numAnalystsEps). It does NOT carry a
-- revision timeline natively.
--
-- We build the timeline ourselves: snapshot daily into `estimate_snapshot`,
-- diff against the prior snapshot, emit one `estimate_revision` row per
-- (symbol, fiscal_period) where any metric drifted.
--
-- That gives us aggregate consensus drift detection — the SPEC §4 "#1
-- leading signal" — without paying for a pre-computed revision feed.

CREATE TABLE IF NOT EXISTS estimate_snapshot (
    id                       bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    symbol                   text NOT NULL,
    fiscal_period_end        date NOT NULL,        -- e.g. 2026-09-27 for AAPL Q4 FY2026
    period_kind              text NOT NULL CHECK (period_kind IN ('annual', 'quarter')),
    -- Consensus values at snapshot time. NULL = vendor didn't return that field.
    eps_avg                  double precision,
    eps_low                  double precision,
    eps_high                 double precision,
    revenue_avg              double precision,
    revenue_low              double precision,
    revenue_high             double precision,
    num_analysts_eps         int,
    num_analysts_revenue     int,
    -- Provenance + idempotency.
    snapshot_at              timestamptz NOT NULL DEFAULT now(),
    raw                      jsonb NOT NULL,        -- full row as returned, for audit
    UNIQUE (symbol, fiscal_period_end, period_kind, snapshot_at)
);
CREATE INDEX IF NOT EXISTS ix_est_snap_lookup
    ON estimate_snapshot(symbol, fiscal_period_end, period_kind, snapshot_at DESC);

-- Append-only revision events. One row per (symbol, fiscal_period) per detected
-- change. The discovery scanner + consensus components both read this table.
CREATE TABLE IF NOT EXISTS estimate_revision (
    id                       bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    symbol                   text NOT NULL,
    fiscal_period_end        date NOT NULL,
    period_kind              text NOT NULL CHECK (period_kind IN ('annual', 'quarter')),
    -- The two snapshots we diffed. NULL prev = first time we saw this period.
    prev_snapshot_id         bigint REFERENCES estimate_snapshot(id),
    curr_snapshot_id         bigint NOT NULL REFERENCES estimate_snapshot(id),
    -- Direction + magnitude. eps_delta_pct is computed as (curr-prev)/|prev|*100.
    -- NULL when prev is null OR prev is zero (no meaningful percent).
    eps_delta                double precision,
    eps_delta_pct            double precision,
    revenue_delta            double precision,
    revenue_delta_pct        double precision,
    -- Bucket label for the discovery scanner's velocity signal.
    -- 'up' / 'down' / 'mixed' (eps and revenue disagree) / 'coverage_change'
    -- (only num_analysts changed, no metric drift).
    direction                text NOT NULL CHECK (direction IN ('up','down','mixed','coverage_change','initial')),
    detected_at              timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS ix_est_rev_symbol_at
    ON estimate_revision(symbol, detected_at DESC);
CREATE INDEX IF NOT EXISTS ix_est_rev_period
    ON estimate_revision(symbol, fiscal_period_end);
