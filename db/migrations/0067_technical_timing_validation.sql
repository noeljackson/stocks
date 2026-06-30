-- Forward-only validation for derived technical timing states (#288).
--
-- Observations are captured when the app computes a tracked technical state.
-- Outcomes are filled later only after enough future daily bars exist.

CREATE TABLE IF NOT EXISTS technical_timing_observation (
    observation_id                    uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    symbol                            text NOT NULL,
    observed_at                       timestamptz NOT NULL,
    technical_state                   text NOT NULL,
    setup_kind                        text NOT NULL,
    entry_stance                      text NOT NULL,
    close                             numeric NOT NULL CHECK (close > 0),
    benchmark_symbol                  text NOT NULL DEFAULT 'QQQ',
    benchmark_close                   numeric CHECK (benchmark_close IS NULL OR benchmark_close > 0),
    horizon_bars                      int NOT NULL DEFAULT 20 CHECK (horizon_bars BETWEEN 1 AND 260),
    evaluation_due_at                 timestamptz NOT NULL,
    input_snapshot                    jsonb NOT NULL DEFAULT '{}'::jsonb,
    benchmark_snapshot                jsonb NOT NULL DEFAULT '{}'::jsonb,
    forward_return_pct                numeric,
    max_drawdown_pct                  numeric,
    benchmark_return_pct              numeric,
    benchmark_max_drawdown_pct        numeric,
    excess_return_pct                 numeric,
    evaluated_at                      timestamptz,
    created_at                        timestamptz NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX IF NOT EXISTS ux_technical_timing_observation_once
    ON technical_timing_observation(
        symbol,
        observed_at,
        technical_state,
        setup_kind,
        horizon_bars,
        benchmark_symbol
    );

CREATE INDEX IF NOT EXISTS ix_technical_timing_observation_due
    ON technical_timing_observation(evaluation_due_at, symbol)
 WHERE evaluated_at IS NULL;

CREATE INDEX IF NOT EXISTS ix_technical_timing_observation_state
    ON technical_timing_observation(technical_state, setup_kind, observed_at DESC);
