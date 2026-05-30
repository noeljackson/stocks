-- 0002_seed.sql — taxonomy seed (SPEC §2) + config v1 defaults (SPEC §4, §6).
-- Re-runnable: uses ON CONFLICT DO NOTHING.

-- ---- Taxonomy seed: 9 clusters (system may add 'emerging' ones later) ----
INSERT INTO cluster (id, name, kind) VALUES
  ('logic_accelerators',  'Logic / accelerators (GPUs, custom ASICs)', 'seed'),
  ('memory_storage',      'Memory & storage (DRAM, HBM, NAND)',        'seed'),
  ('semi_cap_equipment',  'Semi capital equipment (fab tools)',         'seed'),
  ('foundry_mfg',         'Foundry / manufacturing',                    'seed'),
  ('networking_interconnect','Networking & interconnect (switching, optical, NICs/DPUs)','seed'),
  ('datacenter_power',    'Datacenter power, cooling & electrical',      'seed'),
  ('eda_ip',              'EDA / semiconductor IP',                     'seed'),
  ('cloud_hyperscale',    'Cloud / hyperscale infra & datacenter REITs','seed'),
  ('infra_software',      'Infrastructure software (observability, data, dev-infra)','seed')
ON CONFLICT (id) DO NOTHING;

-- ---- Config v1: regime classifier (SPEC §4). Thresholds are defaults; tune freely. ----
INSERT INTO config (name, version, body, active) VALUES
  ('regime', 1, '{
    "states": ["risk_on","neutral","risk_off"],
    "rules": {
      "risk_on":  {"spx_vs_sma12m": ">0", "hy_oas_pct": "<5", "breadth_pct_above_200d": ">50"},
      "risk_off": {"spx_vs_sma12m": "<0", "hy_oas_pct": ">7", "breadth_pct_above_200d": "<35"}
    },
    "capitulation": {
      "any_of": ["vix>25", "vix9d_over_vix>1.10", "aaii_bulls<30", "fear_greed<25", "put_call>1.10"]
    }
  }', true)
ON CONFLICT (name, version) DO NOTHING;

-- ---- Config v1: domain-fit scoring (SPEC §6.1) ----
INSERT INTO config (name, version, body, active) VALUES
  ('domain_fit', 1, '{
    "weights": {"cluster_membership": 40, "revenue_exposure": 25, "value_chain_adjacency": 20, "competence_affinity": 15},
    "revenue_exposure_threshold_pct": 40,
    "promotion_threshold": 60,
    "hard_filters": {"min_market_cap_usd": 1000000000, "min_adv_usd": 5000000}
  }', true)
ON CONFLICT (name, version) DO NOTHING;

-- ---- Config v1: consensus score (SPEC §6.2) ----
INSERT INTO config (name, version, body, active) VALUES
  ('consensus', 1, '{
    "weights": {"coverage_expansion": 25, "estimate_revision_saturation": 20, "mainstream_coverage": 20, "retail_attention": 15, "price_extension": 20},
    "measurement_threshold": 60,
    "exit_threshold": 70
  }', true)
ON CONFLICT (name, version) DO NOTHING;

-- ---- Config v1: risk overlay hard limits (SPEC §7) ----
INSERT INTO config (name, version, body, active) VALUES
  ('risk', 1, '{
    "single_name_delta_notional_pct": 15,
    "options_premium_aggregate_pct": 20,
    "cash_floor_pct": 20,
    "drawdown_brake": [{"at_pct": -10, "size_mult": 0.5}, {"at_pct": -20, "halt_new": true}],
    "subsector_cluster_pct": 35,
    "concurrent_positions": 7
  }', true)
ON CONFLICT (name, version) DO NOTHING;

-- ---- Config v1: discovery signals (placeholder registry; SPEC §2 funnel) ----
INSERT INTO config (name, version, body, active) VALUES
  ('discovery_signals', 1, '{
    "signals": [
      {"name": "estimate_revision_inflection", "weight": 1.0, "enabled": false},
      {"name": "base_breakout",                "weight": 1.0, "enabled": true},
      {"name": "volume_anomaly",               "weight": 0.8, "enabled": true},
      {"name": "filing_news_cluster",          "weight": 0.6, "enabled": false}
    ],
    "promote_to_tier2_threshold": 1.0
  }', true)
ON CONFLICT (name, version) DO NOTHING;
