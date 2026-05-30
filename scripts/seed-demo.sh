#!/usr/bin/env bash
# Seed demo data: sample tickers across the seed cluster taxonomy.
# Idempotent — re-running just upserts.
#
# We deliberately do NOT seed theses here (#16) — those are LLM-drafted by the
# thesis engine against real ingest data. Run:
#     make ingest             # accumulate EDGAR/FRED corpus
#     make refresh-context SYMBOL=NVDA
#     make draft-thesis SYMBOL=NVDA
# to produce a real thesis. The system shouldn't ship fiction even as demo data.
set -euo pipefail

PSQL_URL="${PSQL_URL:-postgres://stocks:stocks_dev_only@localhost:5432/stocks?sslmode=disable}"

echo "Seeding demo tickers against ${PSQL_URL%@*}@…"

psql "$PSQL_URL" -v ON_ERROR_STOP=1 <<'SQL'
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
SQL

echo "Done. Counts:"
psql "$PSQL_URL" -tA -c "SELECT 'tickers',count(*) FROM ticker WHERE status='active'
                          UNION ALL
                         SELECT 'theses',count(*) FROM thesis"
