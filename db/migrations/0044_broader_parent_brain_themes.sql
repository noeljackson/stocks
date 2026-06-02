-- 0044_broader_parent_brain_themes.sql
--
-- Broaden the parent brain beyond current tech/commodity themes. The product
-- edge is not a sector boundary; it is finding evidence-backed opportunities
-- wherever macro, sector, and ticker evidence can create a tradable view.

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

INSERT INTO brain_thesis
    (scope, key, name, state, direction, summary, core_claim, why_now,
     evidence, invalidation_conditions, beneficiaries, open_questions,
     missing_evidence, source_ref, freshness_target_minutes, last_evaluated_at)
VALUES
    (
        'theme',
        'financial_conditions_credit',
        'Financial Conditions and Credit Transmission',
        'forming',
        'mixed',
        'Financials, brokers, asset managers, and regional banks become investable when rates, credit, deposits, capital markets activity, or liquidity change faster than consensus positioning.',
        'The edge is detecting whether financial conditions are loosening or tightening before the affected equities and ETFs fully reflect the change.',
        'This parent view should connect macro rates/credit evidence to banks, brokers, asset managers, exchanges, and credit-sensitive hedges.',
        '[]'::jsonb,
        '[{"name":"credit_transmission_wrong","assertion":"Credit spreads, funding stress, deposits, or capital markets data move against the claimed financial-conditions impulse.","evidence_source":"fred, source_health:fmp, price_bar, earnings"}]'::jsonb,
        '["XLF", "KRE", "JPM", "GS", "MS", "BLK", "BX", "CME"]'::jsonb,
        '["Are credit spreads and funding stress improving or worsening?", "Are banks moving because of net-interest-margin reality or just rate-beta?", "Are asset managers and exchanges seeing real flow/volume acceleration?"]'::jsonb,
        '["credit_spreads", "deposit_beta", "capital_markets_activity", "fund_flows", "estimate_revisions"]'::jsonb,
        '{"seeded_by":"0044_broader_parent_brain_themes","reason":"broad_market_parent_coverage"}'::jsonb,
        720,
        NULL
    ),
    (
        'theme',
        'energy_supply_demand',
        'Energy Supply, Demand, and Services',
        'forming',
        'mixed',
        'Energy equities can become asymmetric when oil/gas prices, inventories, capex discipline, refining margins, LNG demand, or service activity move before estimates and positioning adjust.',
        'The edge is separating commodity-price beta from company-specific cash-flow, service-cycle, and capital-return evidence.',
        'Energy should feed both macro inflation/rates thinking and ticker-level opportunities in producers, services, LNG, and integrated majors.',
        '[]'::jsonb,
        '[{"name":"energy_impulse_fades","assertion":"Commodity price, inventory, margin, or capex evidence no longer supports the claimed energy impulse.","evidence_source":"commodity_price, inventories, estimates, earnings"}]'::jsonb,
        '["XLE", "XOM", "CVX", "COP", "OXY", "SLB", "LNG", "MPC"]'::jsonb,
        '["Is the move driven by supply discipline, demand, geopolitics, or dollar/rates?", "Which names have revision leverage rather than only spot-price beta?", "Are services leading or lagging producers?"]'::jsonb,
        '["oil_gas_price_history", "inventory_data", "rig_activity", "refining_margins", "energy_estimate_revisions"]'::jsonb,
        '{"seeded_by":"0044_broader_parent_brain_themes","reason":"broad_market_parent_coverage"}'::jsonb,
        720,
        NULL
    ),
    (
        'theme',
        'consumer_staples_margin',
        'Consumer Staples, Pricing, and Margin Defense',
        'forming',
        'mixed',
        'Staples and retailers can become investable when food/input inflation, pricing power, trade-down behavior, inventory, and margin recovery diverge from consensus.',
        'The edge is finding where defensive demand and margin evidence are improving or deteriorating before the market treats the group as generic defensives.',
        'This theme links wheat/food inflation and consumer health to retailers, packaged food, beverages, and household-products equities.',
        '[]'::jsonb,
        '[{"name":"staples_margin_claim_breaks","assertion":"Input costs, pricing, volume, or inventory evidence refutes the margin/pricing-power claim.","evidence_source":"earnings, estimates, news, commodity proxies"}]'::jsonb,
        '["XLP", "COST", "WMT", "PG", "KO", "PEP", "GIS", "KHC"]'::jsonb,
        '["Are margins improving from real cost relief or from one-time pricing?", "Is consumer trade-down helping retailers while hurting brands?", "Which staples are defensive leadership versus crowded safety trades?"]'::jsonb,
        '["food_input_costs", "same_store_sales", "gross_margin_revisions", "inventory_data", "consumer_spending"]'::jsonb,
        '{"seeded_by":"0044_broader_parent_brain_themes","reason":"broad_market_parent_coverage"}'::jsonb,
        720,
        NULL
    ),
    (
        'theme',
        'housing_rates_real_assets',
        'Housing, Rates, and Real Assets',
        'forming',
        'mixed',
        'Homebuilders, residential real estate, REITs, and building-products names can become investable when mortgage rates, affordability, supply, rents, and credit conditions change before estimates adjust.',
        'The edge is distinguishing a rates-relief rally from real order, margin, rent, and balance-sheet evidence.',
        'Housing/rates should connect macro duration views to builders, residential landlords, industrial REITs, and building-materials suppliers.',
        '[]'::jsonb,
        '[{"name":"housing_rate_relief_reverses","assertion":"Mortgage rates, affordability, orders, cancellations, rents, or credit conditions refute the housing impulse.","evidence_source":"fred:mortgage, price_bar, earnings, estimates"}]'::jsonb,
        '["XHB", "ITB", "DHI", "LEN", "PHM", "AMH", "PLD", "BLD"]'::jsonb,
        '["Are lower rates translating into orders and margins?", "Are REIT moves about duration or property-level fundamentals?", "Which housing expressions are extended before confirmation?"]'::jsonb,
        '["mortgage_rates", "housing_starts", "builder_orders", "rent_growth", "estimate_revisions"]'::jsonb,
        '{"seeded_by":"0044_broader_parent_brain_themes","reason":"broad_market_parent_coverage"}'::jsonb,
        720,
        NULL
    )
ON CONFLICT (scope, key) DO UPDATE SET
    name = EXCLUDED.name,
    summary = EXCLUDED.summary,
    core_claim = EXCLUDED.core_claim,
    why_now = EXCLUDED.why_now,
    invalidation_conditions = EXCLUDED.invalidation_conditions,
    beneficiaries = EXCLUDED.beneficiaries,
    open_questions = EXCLUDED.open_questions,
    missing_evidence = EXCLUDED.missing_evidence,
    source_ref = brain_thesis.source_ref || EXCLUDED.source_ref,
    freshness_target_minutes = EXCLUDED.freshness_target_minutes,
    updated_at = now();

WITH mapped(theme_key, symbol, role, rationale, conviction) AS (
    VALUES
      ('financial_conditions_credit', 'XLF', 'proxy', 'Broad financial-sector ETF proxy for the parent financial-conditions view.', 70),
      ('financial_conditions_credit', 'KRE', 'proxy', 'Regional-bank proxy for deposit, funding, credit, and rate-sensitivity stress.', 65),
      ('financial_conditions_credit', 'JPM', 'leader', 'Money-center bank expression with credit, deposits, NII, and capital-markets exposure.', 68),
      ('financial_conditions_credit', 'GS', 'beneficiary', 'Capital-markets and investment-banking recovery expression.', 58),
      ('financial_conditions_credit', 'MS', 'beneficiary', 'Wealth, asset-management, and capital-markets expression.', 55),
      ('financial_conditions_credit', 'BLK', 'beneficiary', 'Asset-manager expression for flows, risk appetite, and fee leverage.', 60),
      ('financial_conditions_credit', 'BX', 'beneficiary', 'Alternative-asset expression tied to liquidity, realizations, and credit conditions.', 55),
      ('financial_conditions_credit', 'CME', 'hedge', 'Exchange/volatility beneficiary when rate and commodity uncertainty increases.', 50),
      ('energy_supply_demand', 'XLE', 'proxy', 'Broad energy-sector ETF proxy for commodity and producer confirmation.', 70),
      ('energy_supply_demand', 'XOM', 'leader', 'Integrated major with scale, capital returns, and commodity exposure.', 65),
      ('energy_supply_demand', 'CVX', 'leader', 'Integrated major with commodity and capital-return exposure.', 62),
      ('energy_supply_demand', 'COP', 'beneficiary', 'Producer expression with oil/gas cash-flow leverage.', 60),
      ('energy_supply_demand', 'OXY', 'beneficiary', 'Producer expression with oil leverage and balance-sheet sensitivity.', 55),
      ('energy_supply_demand', 'SLB', 'supplier', 'Oilfield-services expression for upstream capex and activity.', 58),
      ('energy_supply_demand', 'LNG', 'beneficiary', 'LNG infrastructure/export expression tied to gas demand and spreads.', 55),
      ('energy_supply_demand', 'MPC', 'beneficiary', 'Refining-margin expression.', 50),
      ('consumer_staples_margin', 'XLP', 'proxy', 'Broad staples ETF proxy for defensive leadership and crowding.', 70),
      ('consumer_staples_margin', 'COST', 'leader', 'Retailer with traffic, membership, and trade-down resilience.', 65),
      ('consumer_staples_margin', 'WMT', 'leader', 'Retailer and trade-down beneficiary with grocery exposure.', 65),
      ('consumer_staples_margin', 'PG', 'leader', 'Household-products margin/pricing-power expression.', 58),
      ('consumer_staples_margin', 'KO', 'beneficiary', 'Beverage pricing-power and defensive-demand expression.', 52),
      ('consumer_staples_margin', 'PEP', 'beneficiary', 'Beverage/snack pricing and margin expression.', 52),
      ('consumer_staples_margin', 'GIS', 'customer', 'Packaged-food margin expression already linked to agriculture input costs.', 48),
      ('consumer_staples_margin', 'KHC', 'customer', 'Packaged-food margin expression already linked to agriculture input costs.', 48),
      ('housing_rates_real_assets', 'XHB', 'proxy', 'Homebuilder ETF proxy for housing/rates confirmation.', 70),
      ('housing_rates_real_assets', 'ITB', 'proxy', 'Home construction ETF proxy for housing/rates confirmation.', 70),
      ('housing_rates_real_assets', 'DHI', 'leader', 'Large builder expression for orders, incentives, margins, and rates sensitivity.', 64),
      ('housing_rates_real_assets', 'LEN', 'leader', 'Large builder expression for orders, incentives, margins, and rates sensitivity.', 62),
      ('housing_rates_real_assets', 'PHM', 'beneficiary', 'Builder expression for demand and margin evidence.', 58),
      ('housing_rates_real_assets', 'AMH', 'beneficiary', 'Residential rental/home exposure tied to rates, rents, and affordability.', 54),
      ('housing_rates_real_assets', 'PLD', 'beneficiary', 'Industrial REIT/duration and property-fundamental expression.', 50),
      ('housing_rates_real_assets', 'BLD', 'supplier', 'Building-products supplier tied to construction activity and margins.', 52)
)
INSERT INTO ticker(symbol, tier, status)
SELECT DISTINCT symbol, 3, 'active'
  FROM mapped
ON CONFLICT (symbol) DO NOTHING;

WITH mapped(theme_key, symbol, role, rationale, conviction) AS (
    VALUES
      ('financial_conditions_credit', 'XLF', 'proxy', 'Broad financial-sector ETF proxy for the parent financial-conditions view.', 70),
      ('financial_conditions_credit', 'KRE', 'proxy', 'Regional-bank proxy for deposit, funding, credit, and rate-sensitivity stress.', 65),
      ('financial_conditions_credit', 'JPM', 'leader', 'Money-center bank expression with credit, deposits, NII, and capital-markets exposure.', 68),
      ('financial_conditions_credit', 'GS', 'beneficiary', 'Capital-markets and investment-banking recovery expression.', 58),
      ('financial_conditions_credit', 'MS', 'beneficiary', 'Wealth, asset-management, and capital-markets expression.', 55),
      ('financial_conditions_credit', 'BLK', 'beneficiary', 'Asset-manager expression for flows, risk appetite, and fee leverage.', 60),
      ('financial_conditions_credit', 'BX', 'beneficiary', 'Alternative-asset expression tied to liquidity, realizations, and credit conditions.', 55),
      ('financial_conditions_credit', 'CME', 'hedge', 'Exchange/volatility beneficiary when rate and commodity uncertainty increases.', 50),
      ('energy_supply_demand', 'XLE', 'proxy', 'Broad energy-sector ETF proxy for commodity and producer confirmation.', 70),
      ('energy_supply_demand', 'XOM', 'leader', 'Integrated major with scale, capital returns, and commodity exposure.', 65),
      ('energy_supply_demand', 'CVX', 'leader', 'Integrated major with commodity and capital-return exposure.', 62),
      ('energy_supply_demand', 'COP', 'beneficiary', 'Producer expression with oil/gas cash-flow leverage.', 60),
      ('energy_supply_demand', 'OXY', 'beneficiary', 'Producer expression with oil leverage and balance-sheet sensitivity.', 55),
      ('energy_supply_demand', 'SLB', 'supplier', 'Oilfield-services expression for upstream capex and activity.', 58),
      ('energy_supply_demand', 'LNG', 'beneficiary', 'LNG infrastructure/export expression tied to gas demand and spreads.', 55),
      ('energy_supply_demand', 'MPC', 'beneficiary', 'Refining-margin expression.', 50),
      ('consumer_staples_margin', 'XLP', 'proxy', 'Broad staples ETF proxy for defensive leadership and crowding.', 70),
      ('consumer_staples_margin', 'COST', 'leader', 'Retailer with traffic, membership, and trade-down resilience.', 65),
      ('consumer_staples_margin', 'WMT', 'leader', 'Retailer and trade-down beneficiary with grocery exposure.', 65),
      ('consumer_staples_margin', 'PG', 'leader', 'Household-products margin/pricing-power expression.', 58),
      ('consumer_staples_margin', 'KO', 'beneficiary', 'Beverage pricing-power and defensive-demand expression.', 52),
      ('consumer_staples_margin', 'PEP', 'beneficiary', 'Beverage/snack pricing and margin expression.', 52),
      ('consumer_staples_margin', 'GIS', 'customer', 'Packaged-food margin expression already linked to agriculture input costs.', 48),
      ('consumer_staples_margin', 'KHC', 'customer', 'Packaged-food margin expression already linked to agriculture input costs.', 48),
      ('housing_rates_real_assets', 'XHB', 'proxy', 'Homebuilder ETF proxy for housing/rates confirmation.', 70),
      ('housing_rates_real_assets', 'ITB', 'proxy', 'Home construction ETF proxy for housing/rates confirmation.', 70),
      ('housing_rates_real_assets', 'DHI', 'leader', 'Large builder expression for orders, incentives, margins, and rates sensitivity.', 64),
      ('housing_rates_real_assets', 'LEN', 'leader', 'Large builder expression for orders, incentives, margins, and rates sensitivity.', 62),
      ('housing_rates_real_assets', 'PHM', 'beneficiary', 'Builder expression for demand and margin evidence.', 58),
      ('housing_rates_real_assets', 'AMH', 'beneficiary', 'Residential rental/home exposure tied to rates, rents, and affordability.', 54),
      ('housing_rates_real_assets', 'PLD', 'beneficiary', 'Industrial REIT/duration and property-fundamental expression.', 50),
      ('housing_rates_real_assets', 'BLD', 'supplier', 'Building-products supplier tied to construction activity and margins.', 52)
)
INSERT INTO brain_thesis_ticker (brain_thesis_id, symbol, role, rationale, conviction)
SELECT bt.id, m.symbol, m.role, m.rationale, m.conviction
  FROM mapped m
  JOIN brain_thesis bt ON bt.key = m.theme_key
ON CONFLICT (brain_thesis_id, symbol) DO UPDATE SET
    role = EXCLUDED.role,
    rationale = EXCLUDED.rationale,
    conviction = EXCLUDED.conviction;
