-- 0001_init.sql — core data model (SPEC §5, §6)
-- Plain Postgres (PG17+). No extensions needed: gen_random_uuid() is core.
-- Time-series tables use BRIN indexes on the time column (append-only, time-ordered).
-- TimescaleDB is a deferred drop-in: CREATE EXTENSION + create_hypertable() later if
-- data volume ever justifies it — no schema rewrite required.
-- Append-only tables are never UPDATEd by the app.

-- ============================================================
-- Taxonomy (SPEC §2): seed clusters, system-extensible.
-- ============================================================
CREATE TABLE IF NOT EXISTS cluster (
    id          text PRIMARY KEY,                 -- slug, e.g. 'memory_storage'
    name        text NOT NULL,
    kind        text NOT NULL DEFAULT 'seed'      -- 'seed' | 'emerging'
                CHECK (kind IN ('seed','emerging')),
    parent_id   text REFERENCES cluster(id),
    created_at  timestamptz NOT NULL DEFAULT now()
);

-- ============================================================
-- Ticker: persistent monitoring object (SPEC §5.1).
-- ============================================================
CREATE TABLE IF NOT EXISTS ticker (
    symbol            text PRIMARY KEY,
    cluster_id        text REFERENCES cluster(id),
    tier              int  NOT NULL DEFAULT 3 CHECK (tier IN (1,2,3)),
    status            text NOT NULL DEFAULT 'active' CHECK (status IN ('active','archived')),
    options_eligible  bool NOT NULL DEFAULT false,
    market_cap        numeric,
    adv_usd           numeric,                     -- avg daily $ volume (liquidity gate)
    domain_fit        numeric,                     -- 0..100 (SPEC §6.1)
    added_at          timestamptz NOT NULL DEFAULT now(),
    last_promoted_at  timestamptz,
    last_demoted_at   timestamptz
);
CREATE INDEX IF NOT EXISTS ix_ticker_tier    ON ticker(tier) WHERE status = 'active';
CREATE INDEX IF NOT EXISTS ix_ticker_cluster ON ticker(cluster_id);

-- ============================================================
-- Ticker context: 3 bands, each with its own freshness (SPEC §5.2).
-- Append-only; latest = max(version) per symbol.
-- ============================================================
CREATE TABLE IF NOT EXISTS ticker_context (
    symbol           text NOT NULL REFERENCES ticker(symbol),
    version          int  NOT NULL,
    structural       jsonb NOT NULL DEFAULT '{}',   -- quarters: fundamentals, 13F/short int (LAGGED)
    structural_as_of timestamptz,
    narrative        jsonb NOT NULL DEFAULT '{}',   -- days-weeks: themes, analyst trajectory, catalysts
    narrative_as_of  timestamptz,
    market           jsonb NOT NULL DEFAULT '{}',   -- daily: technicals, live options/flow, sentiment
    market_as_of     timestamptz,
    created_at       timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (symbol, version)
);

-- ============================================================
-- Thesis: the lifecycle object = the state machine (SPEC §5.3).
-- ============================================================
CREATE TABLE IF NOT EXISTS thesis (
    thesis_id               uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    symbol                  text NOT NULL REFERENCES ticker(symbol),
    cluster_id              text REFERENCES cluster(id),
    cluster_thesis          text,                          -- parent theme
    state                   text NOT NULL DEFAULT 'forming'
        CHECK (state IN ('forming','building_conviction','armed','actionable',
                         'position_open','exiting','closed','disqualified')),
    -- the "why"
    bull_case               text,
    bear_case               text,
    edge_rationale          text NOT NULL,                 -- REQUIRED (the diffusion gap)
    historical_analogs      jsonb NOT NULL DEFAULT '[]',
    -- validation instrument (does NOT drive exits)
    forecast                jsonb,                         -- {direction, magnitude_rough, horizon}
    -- conditions: each item {type: quantitative|narrative, expr|assertion, ...}
    conviction_conditions   jsonb NOT NULL DEFAULT '[]',
    trigger_conditions      jsonb NOT NULL DEFAULT '[]',
    invalidation_conditions jsonb NOT NULL DEFAULT '[]',
    fulfillment_conditions  jsonb NOT NULL DEFAULT '[]',
    -- execution linkage (advisory)
    conviction_tier         text CHECK (conviction_tier IN ('high','medium','low') OR conviction_tier IS NULL),
    instrument              text CHECK (instrument IN ('equity','leaps') OR instrument IS NULL),
    intended_size           jsonb,
    -- integrity (SPEC §5.3 goalpost detector)
    version                 int  NOT NULL DEFAULT 1,
    immutable_original      jsonb NOT NULL DEFAULT '{}',   -- frozen edge_rationale + invalidation @ v1
    created_at              timestamptz NOT NULL DEFAULT now(),
    updated_at              timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS ix_thesis_state  ON thesis(state);
CREATE INDEX IF NOT EXISTS ix_thesis_symbol ON thesis(symbol);

-- Append-only audit: every state transition (corpus / lead-time accounting).
CREATE TABLE IF NOT EXISTS thesis_state_history (
    id             bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    thesis_id      uuid NOT NULL REFERENCES thesis(thesis_id),
    from_state     text,
    to_state       text NOT NULL,
    rationale      text,
    config_version text,
    at             timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS ix_tsh_thesis ON thesis_state_history(thesis_id, at);

-- Append-only: thesis revisions + goalpost-weakening flag.
CREATE TABLE IF NOT EXISTS thesis_version_history (
    id                    bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    thesis_id             uuid NOT NULL REFERENCES thesis(thesis_id),
    version               int  NOT NULL,
    diff                  jsonb NOT NULL DEFAULT '{}',
    rationale             text,
    weakens_invalidation  bool NOT NULL DEFAULT false,    -- "are we moving the goalpost?"
    at                    timestamptz NOT NULL DEFAULT now()
);

-- ============================================================
-- Market state / regime (SPEC §5.4). Time-series.
-- ============================================================
CREATE TABLE IF NOT EXISTS market_state (
    as_of          timestamptz NOT NULL PRIMARY KEY,
    regime         text NOT NULL CHECK (regime IN ('risk_on','neutral','risk_off')),
    capitulation   bool NOT NULL DEFAULT false,
    indicators     jsonb NOT NULL DEFAULT '{}',   -- {name@version: value}
    subsector_rs   jsonb NOT NULL DEFAULT '{}',   -- relative strength per cluster
    config_version text
);

-- ============================================================
-- Execution / position state (SPEC §5.5).
-- ============================================================
CREATE TABLE IF NOT EXISTS position (
    position_id     uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    thesis_id       uuid REFERENCES thesis(thesis_id),
    symbol          text NOT NULL REFERENCES ticker(symbol),
    instrument      text NOT NULL CHECK (instrument IN ('equity','leaps')),
    qty             numeric NOT NULL,
    avg_price       numeric NOT NULL,
    delta_notional  numeric,           -- counts toward 15% single-name cap
    premium_at_risk numeric,           -- counts toward options aggregate cap
    opened_at       timestamptz NOT NULL DEFAULT now(),
    closed_at       timestamptz,
    realized_pnl    numeric
);
CREATE INDEX IF NOT EXISTS ix_position_open ON position(symbol) WHERE closed_at IS NULL;

-- Append-only decision log.
CREATE TABLE IF NOT EXISTS decision (
    decision_id  uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    thesis_id    uuid REFERENCES thesis(thesis_id),
    action       text NOT NULL,        -- e.g. 'enter','exit','skip','resize'
    user_choice  text,                 -- 'confirmed','rejected','deferred'
    sizing       jsonb,
    at           timestamptz NOT NULL DEFAULT now()
);

-- ============================================================
-- Alerts: emitted significant shifts (SPEC §3 FR7). Feeds UI + lead-time.
-- ============================================================
CREATE TABLE IF NOT EXISTS alert (
    id            bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    thesis_id     uuid REFERENCES thesis(thesis_id),
    symbol        text,
    kind          text NOT NULL CHECK (kind IN ('state_transition','alignment','consensus','risk')),
    payload       jsonb NOT NULL DEFAULT '{}',
    acknowledged  bool NOT NULL DEFAULT false,
    created_at    timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS ix_alert_unack ON alert(created_at) WHERE acknowledged = false;

-- ============================================================
-- Append-only raw ingest log = the self-built point-in-time corpus
-- (SPEC §4 quality stance, FR14). content_hash UNIQUE enforces dedup.
-- ============================================================
CREATE TABLE IF NOT EXISTS ingest_event (
    id           bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    source       text NOT NULL,        -- 'edgar','fred','price','news',...
    kind         text NOT NULL,        -- '10-K','8-K','series','bar',...
    symbol       text,                 -- null for market-wide
    payload      jsonb NOT NULL,
    content_hash text NOT NULL UNIQUE,
    source_ts    timestamptz,          -- event's own timestamp, if known
    ingested_at  timestamptz NOT NULL DEFAULT now()   -- when WE recorded it (PIT anchor)
);
CREATE INDEX IF NOT EXISTS ix_ingest_symbol  ON ingest_event(symbol, ingested_at);
CREATE INDEX IF NOT EXISTS ix_ingest_ts_brin ON ingest_event USING brin(ingested_at);

-- ============================================================
-- Price bars (market band raw). Time-series.
-- ============================================================
CREATE TABLE IF NOT EXISTS price_bar (
    symbol  text NOT NULL,
    ts      timestamptz NOT NULL,
    open    numeric, high numeric, low numeric, close numeric,
    volume  numeric,
    PRIMARY KEY (symbol, ts)
);
CREATE INDEX IF NOT EXISTS ix_price_bar_ts_brin ON price_bar USING brin(ts);

-- ============================================================
-- Indicator values (registry outputs). symbol='' for market-wide.
-- ============================================================
CREATE TABLE IF NOT EXISTS indicator_value (
    name    text NOT NULL,
    version text NOT NULL,
    symbol  text NOT NULL DEFAULT '',
    ts      timestamptz NOT NULL,
    value   numeric,
    PRIMARY KEY (name, version, symbol, ts)
);
CREATE INDEX IF NOT EXISTS ix_indicator_ts_brin ON indicator_value USING brin(ts);

-- ============================================================
-- Config / registry: versioned config blobs (SPEC §3 kernel+registries).
-- Every alert/market_state stamps the config_version that produced it.
-- ============================================================
CREATE TABLE IF NOT EXISTS config (
    name        text NOT NULL,        -- 'regime','discovery_signals','domain_fit','consensus',...
    version     int  NOT NULL,
    body        jsonb NOT NULL,
    active      bool NOT NULL DEFAULT false,
    created_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (name, version)
);
-- exactly one active version per config name
CREATE UNIQUE INDEX IF NOT EXISTS ux_config_active ON config(name) WHERE active;
