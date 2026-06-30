-- 0065_automation_capital_allocator.sql
--
-- Virtual sleeve allocation support (#295). This keeps ownership and
-- allocation accounting separate from net broker positions.

CREATE TABLE IF NOT EXISTS automation_allocation_policy (
    id                           bool PRIMARY KEY DEFAULT true CHECK (id),
    max_strategy_allocation_pct  numeric CHECK (max_strategy_allocation_pct IS NULL OR (max_strategy_allocation_pct > 0 AND max_strategy_allocation_pct <= 1)),
    max_symbol_allocation_pct    numeric CHECK (max_symbol_allocation_pct IS NULL OR (max_symbol_allocation_pct > 0 AND max_symbol_allocation_pct <= 1)),
    max_portfolio_allocation_pct numeric CHECK (max_portfolio_allocation_pct IS NULL OR (max_portfolio_allocation_pct > 0 AND max_portfolio_allocation_pct <= 1)),
    updated_by                   text NOT NULL DEFAULT 'system',
    updated_at                   timestamptz NOT NULL DEFAULT now()
);

INSERT INTO automation_allocation_policy
    (id, max_strategy_allocation_pct, max_symbol_allocation_pct, max_portfolio_allocation_pct)
VALUES (true, 0.10, 0.15, 0.80)
ON CONFLICT (id) DO NOTHING;

ALTER TABLE automation_strategy_sleeve
    ADD COLUMN IF NOT EXISTS unrealized_pnl numeric NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS last_mark_price numeric,
    ADD COLUMN IF NOT EXISTS last_mark_at timestamptz;

CREATE TABLE IF NOT EXISTS automation_sleeve_fill_attribution (
    attribution_id    uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    sleeve_id         uuid NOT NULL REFERENCES automation_strategy_sleeve(sleeve_id) ON DELETE CASCADE,
    position_fill_id  uuid REFERENCES position_fill(fill_id),
    position_id       uuid REFERENCES position(position_id),
    symbol            text NOT NULL REFERENCES ticker(symbol),
    side              text NOT NULL,
    quantity          numeric NOT NULL CHECK (quantity > 0),
    notional_usd      numeric NOT NULL CHECK (notional_usd >= 0),
    realized_pnl_delta numeric NOT NULL DEFAULT 0,
    source_ref        jsonb NOT NULL DEFAULT '{}'::jsonb,
    attributed_at     timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS ix_automation_sleeve_fill_sleeve
    ON automation_sleeve_fill_attribution(sleeve_id, attributed_at DESC);

CREATE INDEX IF NOT EXISTS ix_automation_sleeve_fill_symbol
    ON automation_sleeve_fill_attribution(symbol, attributed_at DESC);
