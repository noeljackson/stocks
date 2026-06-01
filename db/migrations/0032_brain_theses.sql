-- 0032_brain_theses.sql
--
-- First-class top-down brain theses. Ticker theses answer "what do we think
-- about this symbol?" Brain theses answer "what macro/sector/theme view makes
-- this ticker worth watching, and what would invalidate that parent view?"

CREATE TABLE IF NOT EXISTS brain_thesis (
    id                       uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    scope                    text NOT NULL
                             CHECK (scope IN ('macro', 'sector', 'theme')),
    key                      text NOT NULL,
    name                     text NOT NULL,
    state                    text NOT NULL DEFAULT 'forming'
                             CHECK (state IN ('forming', 'active', 'weakening', 'invalidated', 'archived')),
    direction                text NOT NULL DEFAULT 'neutral'
                             CHECK (direction IN ('risk_on', 'risk_off', 'neutral', 'bullish', 'bearish', 'mixed')),
    summary                  text NOT NULL,
    core_claim               text NOT NULL,
    why_now                  text,
    evidence                 jsonb NOT NULL DEFAULT '[]',
    invalidation_conditions  jsonb NOT NULL DEFAULT '[]',
    beneficiaries            jsonb NOT NULL DEFAULT '[]',
    losers                   jsonb NOT NULL DEFAULT '[]',
    open_questions           jsonb NOT NULL DEFAULT '[]',
    missing_evidence         jsonb NOT NULL DEFAULT '[]',
    source_ref               jsonb NOT NULL DEFAULT '{}',
    freshness_target_minutes int NOT NULL DEFAULT 720,
    last_evaluated_at        timestamptz,
    version                  int NOT NULL DEFAULT 1,
    active                   bool NOT NULL DEFAULT true,
    created_at               timestamptz NOT NULL DEFAULT now(),
    updated_at               timestamptz NOT NULL DEFAULT now(),
    UNIQUE (scope, key)
);

CREATE INDEX IF NOT EXISTS ix_brain_thesis_active
    ON brain_thesis(scope, updated_at DESC) WHERE active = true;

CREATE TABLE IF NOT EXISTS brain_thesis_version_history (
    id              bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    brain_thesis_id uuid NOT NULL REFERENCES brain_thesis(id) ON DELETE CASCADE,
    version         int NOT NULL,
    diff            jsonb NOT NULL DEFAULT '{}',
    rationale       text,
    at              timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS ix_btvh_thesis
    ON brain_thesis_version_history(brain_thesis_id, at DESC);

CREATE TABLE IF NOT EXISTS brain_thesis_ticker (
    brain_thesis_id uuid NOT NULL REFERENCES brain_thesis(id) ON DELETE CASCADE,
    symbol          text NOT NULL REFERENCES ticker(symbol),
    role            text NOT NULL DEFAULT 'candidate'
                    CHECK (role IN ('leader', 'challenger', 'supplier', 'customer', 'beneficiary', 'hedge', 'candidate')),
    rationale       text,
    conviction      int CHECK (conviction BETWEEN 0 AND 100),
    created_at      timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (brain_thesis_id, symbol)
);

CREATE INDEX IF NOT EXISTS ix_btt_symbol
    ON brain_thesis_ticker(symbol);

CREATE TABLE IF NOT EXISTS brain_thesis_watchlist (
    brain_thesis_id uuid NOT NULL REFERENCES brain_thesis(id) ON DELETE CASCADE,
    watchlist_id    uuid NOT NULL REFERENCES watchlist(id) ON DELETE CASCADE,
    rationale       text,
    created_at      timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (brain_thesis_id, watchlist_id)
);

CREATE INDEX IF NOT EXISTS ix_btw_watchlist
    ON brain_thesis_watchlist(watchlist_id);

INSERT INTO brain_thesis
    (scope, key, name, state, direction, summary, core_claim, why_now,
     evidence, invalidation_conditions, beneficiaries, open_questions,
     missing_evidence, source_ref, freshness_target_minutes, last_evaluated_at)
VALUES
    (
        'macro',
        'macro_regime',
        'Macro Regime',
        'forming',
        'neutral',
        'Macro posture is neutral until the regime loop promotes a stronger rates/liquidity/growth view.',
        'The system should not let ticker-level enthusiasm override missing or stale macro evidence.',
        'FRED, breadth, volatility, credit, and earnings breadth are required parent evidence for aggressive risk decisions.',
        '[]'::jsonb,
        '[{"name":"risk_off_break","assertion":"Credit, rates, breadth, or volatility deteriorates enough to make aggressive long exposure a contradiction.","evidence_source":"fred:credit+rates, market_state:breadth+vol"}]'::jsonb,
        '[]'::jsonb,
        '["Refresh FRED macro series", "Add breadth and credit internals to market_state", "Reconcile macro view against sector theses"]'::jsonb,
        '["fred_macro", "market_breadth", "credit_spreads", "earnings_breadth"]'::jsonb,
        '{"seeded_by":"0032_brain_theses","reason":"top_down_brain_bootstrap"}'::jsonb,
        720,
        NULL
    ),
    (
        'theme',
        'ai_compute_infrastructure',
        'AI Compute Infrastructure',
        'forming',
        'mixed',
        'AI capex remains the parent theme, but the system must distinguish leaders from challengers and suppliers instead of treating all AI exposure as equal.',
        'The investable edge is not generic AI demand; it is finding where accelerator, networking, memory, power, and software adoption evidence is diffusing slower than price consensus.',
        'Ticker theses should inherit this theme only when product, customer, estimate, or supply-chain evidence makes that ticker a better expression than the obvious mega-cap beta.',
        '[{"source":"research_evidence","claim":"Product/customer adoption evidence is required before declaring an AI infrastructure edge."}]'::jsonb,
        '[{"name":"theme_priced_out","assertion":"Theme beneficiaries move on consensus headlines without confirming estimate/product evidence.","evidence_source":"price_bar+news_article+estimate_snapshot"}]'::jsonb,
        '["NVDA", "AMD", "AVGO", "MRVL", "MU", "DELL", "HPE"]'::jsonb,
        '["Which challengers have real customer traction?", "Which suppliers have estimate revisions not yet broadly diffused?", "Which names are already extended consensus arrivals?"]'::jsonb,
        '["theme_estimate_revision_breadth", "customer_adoption_research", "relative_strength_by_role"]'::jsonb,
        '{"seeded_by":"0032_brain_theses","reason":"top_down_brain_bootstrap"}'::jsonb,
        720,
        NULL
    ),
    (
        'theme',
        'memory_hbm',
        'Memory and HBM',
        'forming',
        'mixed',
        'HBM and memory tightness can create asymmetric revisions, but only if supply, pricing, and customer allocation evidence lead consensus.',
        'Memory/HBM tickers should be ranked by revision velocity, contract visibility, and capacity constraints rather than by generic AI linkage.',
        'This theme is relevant when price action, estimate revisions, and product/customer evidence agree before the broad retail narrative catches up.',
        '[]'::jsonb,
        '[{"name":"supply_glut","assertion":"Capacity additions or pricing deterioration refute the tightness claim.","evidence_source":"filings, estimates, industry research"}]'::jsonb,
        '["MU", "NVDA", "AMD"]'::jsonb,
        '["Is HBM pricing still tightening?", "Are estimate revisions company-specific or just sector beta?"]'::jsonb,
        '["hbm_pricing", "capacity_expansion", "estimate_revisions"]'::jsonb,
        '{"seeded_by":"0032_brain_theses","reason":"top_down_brain_bootstrap"}'::jsonb,
        720,
        NULL
    ),
    (
        'theme',
        'optical_networking',
        'Optical Networking',
        'forming',
        'mixed',
        'AI cluster scale increases optical and connectivity demand, but the edge depends on which suppliers show real order acceleration.',
        'Optical/networking theses need evidence of design wins, backlog quality, and estimate revisions that are not already fully reflected in the chart.',
        'Use this theme to explain proactive review of connectivity suppliers and to reject stretched names where the move already represents consensus arrival.',
        '[]'::jsonb,
        '[{"name":"orders_fail_to_convert","assertion":"Design-win or backlog claims do not convert into revenue/estimate revisions.","evidence_source":"earnings, estimates, filings"}]'::jsonb,
        '["CRDO", "ANET", "MRVL", "LITE"]'::jsonb,
        '["Which suppliers have direct AI-cluster exposure?", "Which are extended without confirming revisions?"]'::jsonb,
        '["customer_design_wins", "estimate_revisions", "order_backlog"]'::jsonb,
        '{"seeded_by":"0032_brain_theses","reason":"top_down_brain_bootstrap"}'::jsonb,
        720,
        NULL
    ),
    (
        'sector',
        'enterprise_security_identity',
        'Enterprise Security and Identity',
        'forming',
        'neutral',
        'Security and identity names need evidence of durable budget priority, not only software multiple recovery.',
        'The parent thesis is that security consolidation can create company-specific inflections when growth durability or margin leverage changes before consensus.',
        'This should steer OKTA-style monitoring theses toward retention, net expansion, large-customer wins, and estimate revisions.',
        '[]'::jsonb,
        '[{"name":"budget_digestion","assertion":"Enterprise software budgets remain constrained and consolidation fails to improve growth durability.","evidence_source":"earnings, estimates, news"}]'::jsonb,
        '["OKTA", "CRWD", "ZS", "PANW"]'::jsonb,
        '["Are revisions improving because of company execution or sector multiple expansion?", "Is consolidation a real customer behavior or a narrative?"]'::jsonb,
        '["customer_win_research", "estimate_revisions", "earnings_transcripts"]'::jsonb,
        '{"seeded_by":"0032_brain_theses","reason":"top_down_brain_bootstrap"}'::jsonb,
        720,
        NULL
    )
ON CONFLICT (scope, key) DO NOTHING;

WITH mapped(theme_key, symbol, role, rationale, conviction) AS (
    VALUES
      ('ai_compute_infrastructure', 'NVDA', 'leader', 'Default leader expression for accelerator platform and AI infrastructure demand.', 70),
      ('ai_compute_infrastructure', 'AMD', 'challenger', 'Challenger expression; requires product/customer traction evidence.', 60),
      ('ai_compute_infrastructure', 'AVGO', 'supplier', 'Custom silicon/networking supplier expression.', 55),
      ('ai_compute_infrastructure', 'MRVL', 'supplier', 'Networking/custom silicon supplier expression.', 50),
      ('ai_compute_infrastructure', 'MU', 'supplier', 'Memory/HBM supplier expression inside the AI capex chain.', 65),
      ('ai_compute_infrastructure', 'DELL', 'beneficiary', 'Server infrastructure beneficiary; must prove AI server margin/volume edge.', 45),
      ('ai_compute_infrastructure', 'HPE', 'beneficiary', 'Enterprise/server infrastructure beneficiary; requires order and margin evidence.', 45),
      ('memory_hbm', 'MU', 'leader', 'Direct memory/HBM expression.', 70),
      ('memory_hbm', 'NVDA', 'customer', 'Key demand-side anchor for HBM capacity and allocation.', 40),
      ('memory_hbm', 'AMD', 'customer', 'Challenger accelerator demand can affect HBM allocation and supplier leverage.', 40),
      ('optical_networking', 'CRDO', 'candidate', 'Pureer optical/connectivity expression requiring order/revision confirmation.', 65),
      ('optical_networking', 'ANET', 'leader', 'Networking leader expression; watch for AI cluster demand diffusion.', 55),
      ('optical_networking', 'MRVL', 'supplier', 'Connectivity/custom silicon overlap expression.', 45),
      ('optical_networking', 'LITE', 'supplier', 'Optical supplier expression if present in the universe.', 45),
      ('enterprise_security_identity', 'OKTA', 'candidate', 'Identity consolidation expression; watch growth durability and retention evidence.', 60),
      ('enterprise_security_identity', 'CRWD', 'leader', 'Security platform leader expression if present in the universe.', 55),
      ('enterprise_security_identity', 'ZS', 'challenger', 'Zero-trust/security software expression if present in the universe.', 45),
      ('enterprise_security_identity', 'PANW', 'leader', 'Security platform leader expression if present in the universe.', 50)
)
INSERT INTO brain_thesis_ticker (brain_thesis_id, symbol, role, rationale, conviction)
SELECT bt.id, m.symbol, m.role, m.rationale, m.conviction
  FROM mapped m
  JOIN brain_thesis bt ON bt.key = m.theme_key
 WHERE EXISTS (SELECT 1 FROM ticker t WHERE t.symbol = m.symbol)
ON CONFLICT (brain_thesis_id, symbol) DO NOTHING;
