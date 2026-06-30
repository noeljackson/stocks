-- IBKR paper order adapter (#297).
--
-- Order placement remains disabled by default and paper-only. The config table
-- requires a DU-prefixed IBKR paper account, and broker orders are constrained
-- to environment_scope='paper'.

CREATE TABLE IF NOT EXISTS automation_paper_order_config (
    config_id                         int PRIMARY KEY DEFAULT 1 CHECK (config_id = 1),
    enabled                           bool NOT NULL DEFAULT false,
    broker                            text NOT NULL DEFAULT 'ibkr' CHECK (broker = 'ibkr'),
    account_mode                      text NOT NULL DEFAULT 'paper' CHECK (account_mode = 'paper'),
    broker_account                    text CHECK (broker_account IS NULL OR broker_account ~ '^DU[0-9A-Z]+$'),
    max_position_snapshot_age_seconds int NOT NULL DEFAULT 120
                                      CHECK (max_position_snapshot_age_seconds BETWEEN 1 AND 3600),
    updated_by                        text,
    created_at                        timestamptz NOT NULL DEFAULT now(),
    updated_at                        timestamptz NOT NULL DEFAULT now()
);

INSERT INTO automation_paper_order_config (config_id, enabled, broker, account_mode)
VALUES (1, false, 'ibkr', 'paper')
ON CONFLICT (config_id) DO NOTHING;

CREATE TABLE IF NOT EXISTS automation_broker_order (
    order_id                 uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    reconciliation_id        uuid NOT NULL REFERENCES automation_execution_reconciliation(reconciliation_id),
    desired_position_id      uuid NOT NULL REFERENCES desired_strategy_position(desired_position_id),
    proof_id                 uuid REFERENCES automation_proof(proof_id),
    sleeve_id                uuid NOT NULL REFERENCES automation_strategy_sleeve(sleeve_id),
    symbol                   text NOT NULL REFERENCES ticker(symbol),
    environment_scope        text NOT NULL CHECK (environment_scope = 'paper'),
    broker                   text NOT NULL CHECK (broker = 'ibkr'),
    broker_account           text NOT NULL CHECK (broker_account ~ '^DU[0-9A-Z]+$'),
    client_order_id          text NOT NULL,
    broker_order_id          text,
    parent_client_order_id   text,
    order_role               text NOT NULL CHECK (order_role IN ('parent', 'take_profit', 'stop_loss')),
    action                   text NOT NULL CHECK (action IN ('buy', 'sell', 'sell_short', 'buy_to_cover')),
    position_side            text NOT NULL CHECK (position_side IN ('long', 'short')),
    order_type               text NOT NULL CHECK (order_type IN ('market', 'limit', 'stop')),
    quantity                 numeric NOT NULL CHECK (quantity > 0),
    limit_price              numeric CHECK (limit_price IS NULL OR limit_price > 0),
    stop_price               numeric CHECK (stop_price IS NULL OR stop_price > 0),
    transmit                 bool NOT NULL DEFAULT true,
    status                   text NOT NULL DEFAULT 'planned'
                             CHECK (status IN (
                               'planned', 'submitted', 'filled', 'partially_filled',
                               'rejected', 'cancelled', 'error', 'unknown'
                             )),
    raw                      jsonb NOT NULL DEFAULT '{}'::jsonb,
    created_at               timestamptz NOT NULL DEFAULT now(),
    updated_at               timestamptz NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX IF NOT EXISTS ux_automation_broker_order_client
    ON automation_broker_order(broker, broker_account, client_order_id);

CREATE INDEX IF NOT EXISTS ix_automation_broker_order_reconciliation
    ON automation_broker_order(reconciliation_id, created_at DESC);

CREATE INDEX IF NOT EXISTS ix_automation_broker_order_symbol
    ON automation_broker_order(symbol, status, updated_at DESC);

CREATE TABLE IF NOT EXISTS automation_broker_order_event (
    event_id          bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    order_id          uuid REFERENCES automation_broker_order(order_id),
    reconciliation_id uuid NOT NULL REFERENCES automation_execution_reconciliation(reconciliation_id),
    symbol            text NOT NULL REFERENCES ticker(symbol),
    broker            text NOT NULL CHECK (broker = 'ibkr'),
    broker_account    text NOT NULL CHECK (broker_account ~ '^DU[0-9A-Z]+$'),
    client_order_id   text NOT NULL,
    broker_order_id   text,
    event_kind        text NOT NULL CHECK (event_kind IN (
                        'submitted', 'status', 'fill', 'partial_fill',
                        'rejected', 'cancelled', 'error', 'unknown'
                      )),
    status            text NOT NULL,
    filled_quantity   numeric CHECK (filled_quantity IS NULL OR filled_quantity >= 0),
    fill_price        numeric CHECK (fill_price IS NULL OR fill_price > 0),
    message           text,
    raw               jsonb NOT NULL DEFAULT '{}'::jsonb,
    created_at        timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS ix_automation_broker_order_event_reconciliation
    ON automation_broker_order_event(reconciliation_id, created_at DESC);

CREATE INDEX IF NOT EXISTS ix_automation_broker_order_event_symbol
    ON automation_broker_order_event(symbol, created_at DESC);
