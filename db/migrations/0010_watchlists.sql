-- 0010_watchlists.sql — user-curated multi-list ticker organization (#54).
--
-- watchlist        = a named, color-coded grouping the user creates
-- watchlist_member = M:N between watchlist and ticker; one ticker can live
--                    on many lists; same list can hold many tickers.
-- added_by         = audit hint: 'user' | 'discovery:<signal>' | 'manual'

CREATE TABLE IF NOT EXISTS watchlist (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name        text NOT NULL,
    description text,
    color       text,                                  -- UI hint (hex or token)
    is_system   bool NOT NULL DEFAULT false,           -- auto-managed lists (e.g. "discovery-pending")
    created_at  timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX IF NOT EXISTS ux_watchlist_name ON watchlist(LOWER(name));

CREATE TABLE IF NOT EXISTS watchlist_member (
    watchlist_id uuid NOT NULL REFERENCES watchlist(id) ON DELETE CASCADE,
    symbol       text NOT NULL REFERENCES ticker(symbol),
    added_at     timestamptz NOT NULL DEFAULT now(),
    added_by     text,                                 -- 'user' | 'discovery:volume_anomaly' | 'manual' | 'seed'
    PRIMARY KEY (watchlist_id, symbol)
);
CREATE INDEX IF NOT EXISTS ix_wlm_symbol ON watchlist_member(symbol);

-- Seed: a handful of starter watchlists. The unique index is on LOWER(name)
-- (case-insensitive); use the matching expression-index conflict target.
INSERT INTO watchlist (name, description, color, is_system) VALUES
  ('Tier 1 active',      'Currently deeply monitored names',                 '#a6e3a1', true),
  ('Discovery pending',  'Auto-populated from discovery_candidate review',   '#f9e2af', true),
  ('LEAPS candidates',   'Names with the chain-liquidity gate passing',      '#89b4fa', false),
  ('Watching after Q',   'Names to revisit after their next earnings print', '#cba6f7', false)
ON CONFLICT (LOWER(name)) DO NOTHING;

-- Auto-populate "Tier 1 active" from existing tier=1 rows.
INSERT INTO watchlist_member (watchlist_id, symbol, added_by)
SELECT w.id, t.symbol, 'seed'
  FROM watchlist w, ticker t
 WHERE w.name = 'Tier 1 active' AND t.tier = 1 AND t.status = 'active'
ON CONFLICT DO NOTHING;
