-- 0017_discovery_pool.sql — broad scan pool for discovery (#88).
--
-- The active universe (`ticker`) is what we deeply monitor: pull bars, fetch
-- XBRL, draft theses. The discovery_pool is the much larger "what could
-- become a candidate" set — pulled nightly from FMP's company-screener API.
--
-- Discovery scans the pool, fires signals on its members, and confirmed
-- candidates promote into `ticker` (existing confirm_discovery_candidate
-- already does `INSERT INTO ticker ... ON CONFLICT DO NOTHING` at tier=2).

CREATE TABLE IF NOT EXISTS discovery_pool (
    symbol            text PRIMARY KEY,
    company_name      text,
    sector            text,
    industry          text,
    market_cap        bigint,
    last_seen_at      timestamptz NOT NULL DEFAULT now(),
    -- When did we first observe this symbol in the screener? Useful for
    -- "newly-listed" attention signals later.
    first_seen_at     timestamptz NOT NULL DEFAULT now(),
    -- Have we backfilled bars+facts for it yet? Set true after the first
    -- successful XBRL/price pull so the scanner knows what's ready to
    -- evaluate vs what's still cold.
    backfilled        bool NOT NULL DEFAULT false,
    -- Honest exit: when a name drops below screener criteria, we keep the
    -- row but mark it. Lets us track "dropped" candidates and not re-pull
    -- their bars on every cron.
    dropped_at        timestamptz
);
CREATE INDEX IF NOT EXISTS ix_pool_active
    ON discovery_pool(symbol) WHERE dropped_at IS NULL;
CREATE INDEX IF NOT EXISTS ix_pool_sector
    ON discovery_pool(sector, market_cap DESC) WHERE dropped_at IS NULL;
