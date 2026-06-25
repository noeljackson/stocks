-- 0058_live_brain_ticker_conviction.sql
-- Brain parent-theme links are seeded/static, but ticker theses evolve. Keep
-- Brain surfaces ranked by the strongest currently known ticker signal while
-- preserving the original mapping conviction for auditability.

CREATE OR REPLACE FUNCTION brain_ticker_live_conviction(
    mapping_conviction integer,
    thesis_conviction_tier text,
    thesis_system_confidence text,
    thesis_forecast jsonb
) RETURNS integer
LANGUAGE sql
IMMUTABLE
AS $$
    SELECT GREATEST(
        COALESCE(mapping_conviction, 50),
        COALESCE(
            CASE lower(NULLIF(thesis_conviction_tier, ''))
                WHEN 'very_high' THEN 90
                WHEN 'high' THEN 85
                WHEN 'medium' THEN 65
                WHEN 'low' THEN 45
                ELSE NULL
            END,
            CASE lower(NULLIF(thesis_system_confidence, ''))
                WHEN 'very_high' THEN 90
                WHEN 'high' THEN 85
                WHEN 'medium' THEN 65
                WHEN 'low' THEN 45
                ELSE NULL
            END,
            CASE lower(NULLIF(thesis_forecast->>'confidence', ''))
                WHEN 'very_high' THEN 90
                WHEN 'high' THEN 85
                WHEN 'medium' THEN 65
                WHEN 'low' THEN 45
                ELSE NULL
            END,
            CASE lower(NULLIF(thesis_forecast->>'system_confidence', ''))
                WHEN 'very_high' THEN 90
                WHEN 'high' THEN 85
                WHEN 'medium' THEN 65
                WHEN 'low' THEN 45
                ELSE NULL
            END,
            0
        )
    );
$$;
