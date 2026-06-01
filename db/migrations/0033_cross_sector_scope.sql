-- 0033_cross_sector_scope.sql
--
-- Broaden discovery/brain framing from a fixed tech-infrastructure boundary to
-- evidence-backed opportunities across liquid equities. Tech infrastructure is
-- a current theme, not the product's hard edge.

UPDATE discovery_candidate dc
   SET domain_fit = scored.domain_fit,
       proposed_tier = CASE
           WHEN scored.domain_fit >= 80 THEN 1
           WHEN scored.domain_fit >= 60 THEN 2
           ELSE 3
       END
  FROM (
      SELECT dc.id,
             CASE
               WHEN dp.industry ILIKE '%Semiconductor%' THEN 88.0
               WHEN dp.industry ILIKE '%Communication Equipment%'
                 OR dp.industry ILIKE '%Computer Hardware%' THEN 82.0
               WHEN dp.industry ILIKE '%Electrical%'
                 OR dp.industry ILIKE '%Utility%'
                 OR dp.industry ILIKE '%Utilities%'
                 OR dp.sector = 'Utilities' THEN 76.0
               WHEN dp.industry ILIKE '%Copper%'
                 OR dp.industry ILIKE '%Steel%'
                 OR dp.industry ILIKE '%Aluminum%'
                 OR dp.industry ILIKE '%Industrial Metals%'
                 OR dp.industry ILIKE '%Other Industrial Metals%'
                 OR dp.industry ILIKE '%Silver%'
                 OR dp.industry ILIKE '%Gold%'
                 OR dp.industry ILIKE '%Metals%'
                 OR dp.industry ILIKE '%Mining%'
                 OR dp.industry ILIKE '%Construction Materials%'
                 OR dp.industry ILIKE '%Industrial Materials%'
                 OR dp.industry ILIKE '%Chemicals%' THEN 72.0
               WHEN dp.industry ILIKE '%Agricultural%'
                 OR dp.industry ILIKE '%Farm%'
                 OR dp.industry ILIKE '%Food%'
                 OR dp.sector = 'Consumer Defensive' THEN 70.0
               WHEN dp.industry ILIKE '%Engineering & Construction%' THEN 68.0
               WHEN dp.sector = 'Energy' THEN 68.0
               WHEN dp.industry ILIKE '%Software%'
                 OR dp.sector = 'Technology' THEN 70.0
               WHEN dp.sector = 'Basic Materials' THEN 62.0
               WHEN dp.sector = 'Financial Services' THEN 62.0
               WHEN dp.sector = 'Real Estate' THEN 58.0
               WHEN dp.sector IN ('Healthcare', 'Consumer Cyclical', 'Communication Services') THEN 55.0
               ELSE 45.0
             END AS domain_fit
        FROM discovery_candidate dc
        JOIN discovery_pool dp ON dp.symbol = dc.symbol
       WHERE dc.signal_name = 'research_nomination'
         AND dc.status = 'proposed'
  ) scored
 WHERE dc.id = scored.id;

INSERT INTO brain_thesis
    (scope, key, name, state, direction, summary, core_claim, why_now,
     evidence, invalidation_conditions, beneficiaries, open_questions,
     missing_evidence, source_ref, freshness_target_minutes, last_evaluated_at)
VALUES
    (
        'theme',
        'copper_industrial_metals',
        'Copper and Industrial Metals',
        'forming',
        'mixed',
        'Copper and industrial metals can be investable when grid buildout, electrification, China/EM demand, supply discipline, and inventories create revision pressure.',
        'The edge is not owning copper because it is fashionable; it is detecting when supply/demand evidence is moving faster than miners, equipment suppliers, or commodity proxies are priced.',
        'Copper matters as a macro/inflation input and as a tradable theme through miners, metals ETFs, industrial suppliers, and downstream margin pressure.',
        '[]'::jsonb,
        '[{"name":"copper_demand_fades","assertion":"Inventories rise or construction/grid demand weakens enough to refute the tightness claim.","evidence_source":"commodity_price, inventories, earnings, macro"}]'::jsonb,
        '["FCX", "SCCO", "NUE", "XME", "CPER"]'::jsonb,
        '["Are inventories tightening or only price momentum?", "Which equities have operating leverage to copper rather than generic materials beta?", "Who is hurt by input-cost inflation?"]'::jsonb,
        '["commodity_price_history", "inventory_data", "producer_estimate_revisions", "china_demand_indicators"]'::jsonb,
        '{"seeded_by":"0033_cross_sector_scope","reason":"commodity_factor_bootstrap"}'::jsonb,
        720,
        NULL
    ),
    (
        'theme',
        'wheat_agriculture_food',
        'Wheat, Agriculture, and Food Inflation',
        'forming',
        'mixed',
        'Wheat and agriculture shocks can create investable moves through crop prices, fertilizer, farm equipment, food producers, staples margins, and inflation/rates expectations.',
        'The edge is detecting when weather, geopolitics, inventories, or crop-price evidence changes faster than related equities and inflation-sensitive assets adjust.',
        'Wheat should feed both macro/regime views and tradable expression search; it is not merely an out-of-scope commodity.',
        '[]'::jsonb,
        '[{"name":"crop_shock_normalizes","assertion":"Weather/geopolitical/inventory pressure normalizes and removes the commodity-price impulse.","evidence_source":"commodity_price, USDA, weather, news"}]'::jsonb,
        '["WEAT", "ADM", "BG", "MOS", "CF", "DE", "GIS", "KHC"]'::jsonb,
        '["Is the wheat move supply-driven, demand-driven, or currency/geopolitical?", "Which companies benefit versus suffer margin pressure?", "Does food inflation change rates or consumer-staples leadership?"]'::jsonb,
        '["commodity_price_history", "usda_crop_reports", "weather_risk", "fertilizer_estimate_revisions"]'::jsonb,
        '{"seeded_by":"0033_cross_sector_scope","reason":"commodity_factor_bootstrap"}'::jsonb,
        720,
        NULL
    )
ON CONFLICT (scope, key) DO NOTHING;

WITH mapped(theme_key, symbol, role, rationale, conviction) AS (
    VALUES
      ('copper_industrial_metals', 'FCX', 'leader', 'Direct copper/mining operating leverage expression.', 72),
      ('copper_industrial_metals', 'SCCO', 'leader', 'Direct copper/mining operating leverage expression.', 72),
      ('copper_industrial_metals', 'NUE', 'beneficiary', 'Steel/materials expression for industrial construction and infrastructure demand.', 55),
      ('wheat_agriculture_food', 'ADM', 'beneficiary', 'Agricultural processing/trading expression for crop and food-chain volatility.', 68),
      ('wheat_agriculture_food', 'BG', 'beneficiary', 'Agricultural processing/trading expression for crop and food-chain volatility.', 68),
      ('wheat_agriculture_food', 'MOS', 'supplier', 'Fertilizer/input-cost expression tied to crop economics.', 62),
      ('wheat_agriculture_food', 'CF', 'supplier', 'Fertilizer/input-cost expression tied to crop economics.', 62),
      ('wheat_agriculture_food', 'DE', 'supplier', 'Farm equipment expression tied to crop income and capex.', 58)
)
INSERT INTO brain_thesis_ticker (brain_thesis_id, symbol, role, rationale, conviction)
SELECT bt.id, m.symbol, m.role, m.rationale, m.conviction
  FROM mapped m
  JOIN brain_thesis bt ON bt.key = m.theme_key
 WHERE EXISTS (SELECT 1 FROM ticker t WHERE t.symbol = m.symbol)
ON CONFLICT (brain_thesis_id, symbol) DO NOTHING;
