-- 0054_system_confidence_human_conviction.sql
-- Separate machine-assessed thesis confidence from operator conviction.

ALTER TABLE thesis
    ADD COLUMN IF NOT EXISTS system_confidence text,
    ADD COLUMN IF NOT EXISTS system_confidence_components jsonb NOT NULL DEFAULT '{}'::jsonb;

ALTER TABLE thesis
    DROP CONSTRAINT IF EXISTS thesis_system_confidence_check;

ALTER TABLE thesis
    ADD CONSTRAINT thesis_system_confidence_check
    CHECK (
        system_confidence IS NULL
        OR system_confidence IN ('low', 'medium', 'high', 'very_high')
    );

UPDATE thesis
   SET system_confidence = CASE
           WHEN lower(forecast->>'system_confidence') IN ('very_high', 'high', 'medium', 'low')
             THEN lower(forecast->>'system_confidence')
           WHEN lower(forecast->>'confidence') IN ('very_high', 'high', 'medium', 'low')
             THEN lower(forecast->>'confidence')
           WHEN conviction_tier IN ('high', 'medium', 'low')
             THEN conviction_tier
           ELSE system_confidence
       END
 WHERE system_confidence IS NULL;

UPDATE thesis
   SET system_confidence_components = jsonb_strip_nulls(
           jsonb_build_object(
               'backfilled_from', CASE
                   WHEN lower(forecast->>'system_confidence') IN ('very_high', 'high', 'medium', 'low')
                     THEN 'forecast.system_confidence'
                   WHEN lower(forecast->>'confidence') IN ('very_high', 'high', 'medium', 'low')
                     THEN 'forecast.confidence'
                   WHEN conviction_tier IN ('high', 'medium', 'low')
                     THEN 'conviction_tier'
                   ELSE NULL
               END,
               'conviction_tier', conviction_tier
           )
       )
 WHERE system_confidence_components = '{}'::jsonb
   AND system_confidence IS NOT NULL;

ALTER TABLE decision
    ADD COLUMN IF NOT EXISTS human_conviction text,
    ADD COLUMN IF NOT EXISTS reason text;

ALTER TABLE decision
    DROP CONSTRAINT IF EXISTS decision_human_conviction_check;

ALTER TABLE decision
    ADD CONSTRAINT decision_human_conviction_check
    CHECK (
        human_conviction IS NULL
        OR human_conviction IN ('low', 'medium', 'high')
    );
