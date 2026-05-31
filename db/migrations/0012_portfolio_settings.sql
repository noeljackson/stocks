-- 0012_portfolio_settings.sql — operator-set portfolio frame (#26).
--
-- The risk overlay (SPEC §7) needs a real account size to turn the proposed
-- intent's $-notional into a percentage. Without it, every veto is fictional.
--
-- For v0 (pre-IBKR bridge, #25) the operator sets the account size manually.
-- The high-water mark is also operator-set, and is the anchor for drawdown
-- calculations (drawdown = (high_water_mark - current_equity) / high_water_mark).
-- Current equity = account_size_usd + SUM(realized_pnl over closed positions).
--
-- Singleton row (id=1). UPSERT pattern in code.
CREATE TABLE IF NOT EXISTS portfolio_settings (
    id                    int PRIMARY KEY DEFAULT 1 CHECK (id = 1),
    account_size_usd      numeric,           -- NULL = unset; risk overlay logs and falls back honestly
    high_water_mark_usd   numeric,           -- NULL = anchor at account_size_usd
    updated_at            timestamptz NOT NULL DEFAULT now(),
    updated_by            text                                -- 'user' | 'ibkr-sync' | 'seed'
);

-- Seed an empty row so the upsert lands on an existing PK.
INSERT INTO portfolio_settings (id) VALUES (1) ON CONFLICT (id) DO NOTHING;
