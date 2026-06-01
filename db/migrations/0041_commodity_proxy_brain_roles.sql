-- 0041_commodity_proxy_brain_roles.sql
--
-- Parent brain themes need to distinguish operating-company expressions from
-- tradable factor proxies. CPER/WEAT/XME price bars can satisfy commodity price
-- history, while inventories, USDA, weather, and demand indicators remain
-- separate missing evidence.

ALTER TABLE brain_thesis_ticker
    DROP CONSTRAINT IF EXISTS brain_thesis_ticker_role_check;

ALTER TABLE brain_thesis_ticker
    ADD CONSTRAINT brain_thesis_ticker_role_check
    CHECK (role IN (
        'leader',
        'challenger',
        'supplier',
        'customer',
        'beneficiary',
        'hedge',
        'candidate',
        'proxy'
    ));

WITH mapped(theme_key, symbol, role, rationale, conviction) AS (
    VALUES
      ('copper_industrial_metals', 'CPER', 'proxy', 'Direct copper ETF proxy; use price history as the commodity-price expression until futures/inventory feeds are wired.', 75),
      ('copper_industrial_metals', 'XME', 'proxy', 'Metals and mining ETF proxy for industrial-metals factor confirmation.', 62),
      ('copper_industrial_metals', 'FCX', 'leader', 'Direct copper/mining operating leverage expression.', 72),
      ('copper_industrial_metals', 'SCCO', 'leader', 'Direct copper/mining operating leverage expression.', 72),
      ('copper_industrial_metals', 'NUE', 'beneficiary', 'Steel/materials expression for industrial construction and infrastructure demand.', 55),
      ('wheat_agriculture_food', 'WEAT', 'proxy', 'Direct wheat ETF proxy; use price history as the commodity-price expression until futures/USDA/weather feeds are wired.', 75),
      ('wheat_agriculture_food', 'ADM', 'beneficiary', 'Agricultural processing/trading expression for crop and food-chain volatility.', 68),
      ('wheat_agriculture_food', 'BG', 'beneficiary', 'Agricultural processing/trading expression for crop and food-chain volatility.', 68),
      ('wheat_agriculture_food', 'MOS', 'supplier', 'Fertilizer/input-cost expression tied to crop economics.', 62),
      ('wheat_agriculture_food', 'CF', 'supplier', 'Fertilizer/input-cost expression tied to crop economics.', 62),
      ('wheat_agriculture_food', 'DE', 'supplier', 'Farm equipment expression tied to crop income and capex.', 58),
      ('wheat_agriculture_food', 'GIS', 'customer', 'Packaged-food margin expression that can be hurt or helped by crop-cost pass-through.', 45),
      ('wheat_agriculture_food', 'KHC', 'customer', 'Packaged-food margin expression that can be hurt or helped by crop-cost pass-through.', 45)
)
INSERT INTO ticker(symbol, tier, status)
SELECT DISTINCT symbol, 3, 'active'
  FROM mapped
ON CONFLICT (symbol) DO NOTHING;

WITH mapped(theme_key, symbol, role, rationale, conviction) AS (
    VALUES
      ('copper_industrial_metals', 'CPER', 'proxy', 'Direct copper ETF proxy; use price history as the commodity-price expression until futures/inventory feeds are wired.', 75),
      ('copper_industrial_metals', 'XME', 'proxy', 'Metals and mining ETF proxy for industrial-metals factor confirmation.', 62),
      ('copper_industrial_metals', 'FCX', 'leader', 'Direct copper/mining operating leverage expression.', 72),
      ('copper_industrial_metals', 'SCCO', 'leader', 'Direct copper/mining operating leverage expression.', 72),
      ('copper_industrial_metals', 'NUE', 'beneficiary', 'Steel/materials expression for industrial construction and infrastructure demand.', 55),
      ('wheat_agriculture_food', 'WEAT', 'proxy', 'Direct wheat ETF proxy; use price history as the commodity-price expression until futures/USDA/weather feeds are wired.', 75),
      ('wheat_agriculture_food', 'ADM', 'beneficiary', 'Agricultural processing/trading expression for crop and food-chain volatility.', 68),
      ('wheat_agriculture_food', 'BG', 'beneficiary', 'Agricultural processing/trading expression for crop and food-chain volatility.', 68),
      ('wheat_agriculture_food', 'MOS', 'supplier', 'Fertilizer/input-cost expression tied to crop economics.', 62),
      ('wheat_agriculture_food', 'CF', 'supplier', 'Fertilizer/input-cost expression tied to crop economics.', 62),
      ('wheat_agriculture_food', 'DE', 'supplier', 'Farm equipment expression tied to crop income and capex.', 58),
      ('wheat_agriculture_food', 'GIS', 'customer', 'Packaged-food margin expression that can be hurt or helped by crop-cost pass-through.', 45),
      ('wheat_agriculture_food', 'KHC', 'customer', 'Packaged-food margin expression that can be hurt or helped by crop-cost pass-through.', 45)
)
INSERT INTO brain_thesis_ticker (brain_thesis_id, symbol, role, rationale, conviction)
SELECT bt.id, m.symbol, m.role, m.rationale, m.conviction
  FROM mapped m
  JOIN brain_thesis bt ON bt.key = m.theme_key
ON CONFLICT (brain_thesis_id, symbol) DO UPDATE SET
    role = EXCLUDED.role,
    rationale = EXCLUDED.rationale,
    conviction = EXCLUDED.conviction;
