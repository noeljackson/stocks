-- 0005_condition_view.sql — flatten thesis conditions for query (#9).
--
-- The per-condition shape extension (target, deadline_at, evidence_source,
-- status, last_checked_at, last_observed_value) is purely an in-JSONB
-- change — no new columns on `thesis`. The legacy { name, type, expr,
-- assertion } shape stays valid; new fields are additive and default at
-- the application layer.
--
-- This view exists so the staleness service (#11) + substance gates (#10) +
-- condition evaluator (#14) can write tractable SQL like:
--
--   SELECT thesis_id, role, name FROM v_condition
--    WHERE status = 'pending' AND deadline_at < now();
--
-- Without it, every query would re-walk all four jsonb_array_elements calls.

CREATE OR REPLACE VIEW v_condition AS
SELECT
    t.thesis_id,
    t.symbol,
    t.state                          AS thesis_state,
    flat.role,
    flat.position,
    flat.cond->>'name'               AS name,
    flat.cond->>'type'               AS condition_type,
    flat.cond->>'expr'               AS expr,
    flat.cond->>'assertion'          AS assertion,
    flat.cond->'target'              AS target,
    NULLIF(flat.cond->>'deadline_at', '')::timestamptz   AS deadline_at,
    flat.cond->>'evidence_source'    AS evidence_source,
    COALESCE(flat.cond->>'status', 'pending')            AS status,
    NULLIF(flat.cond->>'last_checked_at', '')::timestamptz AS last_checked_at,
    flat.cond->'last_observed_value' AS last_observed_value,
    flat.cond                        AS raw
FROM thesis t
CROSS JOIN LATERAL (
    SELECT cond, ord AS position, role FROM (
        SELECT value AS cond, ordinality AS ord, 'conviction'  AS role
          FROM jsonb_array_elements(COALESCE(t.conviction_conditions,  '[]'::jsonb)) WITH ORDINALITY
        UNION ALL
        SELECT value, ordinality, 'trigger'
          FROM jsonb_array_elements(COALESCE(t.trigger_conditions,      '[]'::jsonb)) WITH ORDINALITY
        UNION ALL
        SELECT value, ordinality, 'invalidation'
          FROM jsonb_array_elements(COALESCE(t.invalidation_conditions, '[]'::jsonb)) WITH ORDINALITY
        UNION ALL
        SELECT value, ordinality, 'fulfillment'
          FROM jsonb_array_elements(COALESCE(t.fulfillment_conditions,  '[]'::jsonb)) WITH ORDINALITY
    ) inner_flat
) flat;

COMMENT ON VIEW v_condition IS
'Flattened per-condition view across all four condition arrays on `thesis`. '
'Used by the staleness service (#11), substance gates (#10), and condition evaluator (#14).';
