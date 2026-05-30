#!/usr/bin/env bash
# Seed demo data: sample tickers + a couple of theses in different states.
# Idempotent — re-running just upserts.
set -euo pipefail

PSQL_URL="${PSQL_URL:-postgres://stocks:stocks_dev_only@localhost:5432/stocks?sslmode=disable}"

echo "Seeding demo tickers + theses against ${PSQL_URL%@*}@…"

psql "$PSQL_URL" -v ON_ERROR_STOP=1 <<'SQL'
-- Sample tracked tickers across clusters.
INSERT INTO ticker (symbol, cluster_id, tier, options_eligible, market_cap, adv_usd, domain_fit)
VALUES
  ('NVDA', 'logic_accelerators', 1, true,  3300000000000, 25000000000, 92),
  ('MU',   'memory_storage',     1, true,    140000000000,  3000000000, 88),
  ('AMD',  'logic_accelerators', 2, true,   240000000000,  4000000000, 78),
  ('AMAT', 'semi_cap_equipment', 2, true,   150000000000,  1800000000, 81),
  ('TSM',  'foundry_mfg',        2, false,  720000000000,  3200000000, 86),
  ('ANET', 'networking_interconnect', 2, true, 110000000000, 1500000000, 74),
  ('VRT',  'datacenter_power',   3, true,   45000000000,    900000000, 69),
  ('CDNS', 'eda_ip',             3, false,  85000000000,    600000000, 71)
ON CONFLICT (symbol) DO UPDATE SET
  cluster_id = EXCLUDED.cluster_id,
  tier = EXCLUDED.tier,
  options_eligible = EXCLUDED.options_eligible,
  domain_fit = EXCLUDED.domain_fit;

-- A couple of sample theses in different lifecycle states.
INSERT INTO thesis (thesis_id, symbol, cluster_id, state, edge_rationale, bull_case, bear_case,
                    invalidation_conditions, immutable_original, conviction_tier, instrument)
VALUES (
  'aaaaaaaa-0000-0000-0000-000000000001',
  'NVDA', 'logic_accelerators', 'armed',
  'Tier-1 demand visibility 2 quarters ahead of analyst consensus on HBM-bound DC',
  'CY27 DC capex still ramping; hyperscale ROIC math intact through CY26',
  'Custom ASIC mix shift; HBM supply normalization compresses pricing',
  '[
    {"type":"quantitative","name":"gm","expr":"gross_margin < 60"},
    {"type":"narrative","name":"hyperscaler_capex","assertion":"Top-3 hyperscalers cut capex >15% YoY"}
  ]'::jsonb,
  '{
    "edge_rationale":"Tier-1 demand visibility 2 quarters ahead of analyst consensus on HBM-bound DC",
    "invalidation_conditions":[
      {"type":"quantitative","name":"gm","expr":"gross_margin < 60"},
      {"type":"narrative","name":"hyperscaler_capex","assertion":"Top-3 hyperscalers cut capex >15% YoY"}
    ]
  }'::jsonb,
  'high', 'equity'
),
(
  'aaaaaaaa-0000-0000-0000-000000000002',
  'MU', 'memory_storage', 'building_conviction',
  'HBM4 ramp into 2026 with sustained DRAM tightness; lead-time to consensus thesis',
  'Capacity adds lag demand; pricing power lasts through CY26',
  'Hyperscaler internal HBM efforts; smartphone DRAM bust',
  '[
    {"type":"quantitative","name":"dram_pricing_qoq","expr":"dram_contract_qoq < -5"}
  ]'::jsonb,
  '{
    "edge_rationale":"HBM4 ramp into 2026 with sustained DRAM tightness",
    "invalidation_conditions":[
      {"type":"quantitative","name":"dram_pricing_qoq","expr":"dram_contract_qoq < -5"}
    ]
  }'::jsonb,
  'medium', 'leaps'
)
ON CONFLICT (thesis_id) DO UPDATE SET
  state = EXCLUDED.state,
  invalidation_conditions = EXCLUDED.invalidation_conditions;
SQL

echo "Done. Counts:"
psql "$PSQL_URL" -tA -c "SELECT 'tickers',count(*) FROM ticker WHERE status='active'
                          UNION ALL
                         SELECT 'theses',count(*) FROM thesis"
