-- 0003_reflection.sql — forward-only calibration (SPEC §3 FR9, §6.2).
--
-- Append-only. Every prediction recorded when a thesis becomes actionable;
-- the matching outcome row is written later (manually for v0, automatically
-- once price/consensus ingestion lands). Calibration metrics are computed
-- on the fly by joining these two tables.

CREATE TABLE IF NOT EXISTS prediction (
    prediction_id  uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    thesis_id      uuid REFERENCES thesis(thesis_id),
    symbol         text,                                       -- denormalised for query speed
    kind           text NOT NULL CHECK (kind IN (
                       'direction',                            -- probabilistic up/down call
                       'lead_time_to_consensus',               -- early-flag claim
                       'fulfillment'                           -- thesis-completion claim
                   )),
    -- Shape varies by kind:
    --   direction:              {"prob_up": 0.72, "horizon_days": 90}
    --   lead_time_to_consensus: {"consensus_signal": "analyst_revision_majority"}
    --   fulfillment:            {"target_pct": 30, "horizon_days": 180}
    claim          jsonb NOT NULL,
    config_version text,                                       -- which thesis-engine config produced this
    at             timestamptz NOT NULL DEFAULT now(),
    horizon_at     timestamptz                                 -- when this prediction can be scored
);
CREATE INDEX IF NOT EXISTS ix_prediction_thesis ON prediction(thesis_id, at);
CREATE INDEX IF NOT EXISTS ix_prediction_at_brin ON prediction USING brin(at);

CREATE TABLE IF NOT EXISTS outcome (
    outcome_id     bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    prediction_id  uuid NOT NULL REFERENCES prediction(prediction_id),
    -- Shape varies by prediction.kind:
    --   direction:              {"realised_up": true}
    --   lead_time_to_consensus: {"consensus_at": "2026-08-01T..."}  → score = (consensus_at - prediction.at).days
    --   fulfillment:            {"realised_pct": 22, "at_horizon": true}
    observed       jsonb NOT NULL,
    observed_at    timestamptz NOT NULL DEFAULT now(),
    score          double precision,                           -- Brier (0..1) or signed lead-time-days
    UNIQUE (prediction_id)                                     -- one outcome per prediction
);
CREATE INDEX IF NOT EXISTS ix_outcome_observed_at_brin ON outcome USING brin(observed_at);
