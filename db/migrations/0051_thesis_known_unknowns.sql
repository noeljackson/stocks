-- 0051_thesis_known_unknowns.sql
-- First-class unknowns that would materially change thesis confidence.

ALTER TABLE thesis
    ADD COLUMN IF NOT EXISTS known_unknowns jsonb NOT NULL DEFAULT '[]'::jsonb;

UPDATE thesis
   SET known_unknowns = '[]'::jsonb
 WHERE known_unknowns IS NULL;

UPDATE thesis
   SET known_unknowns = jsonb_build_array(
       jsonb_build_object(
           'question', 'What fresh evidence would materially change the ' || symbol || ' thesis?',
           'watch_for', 'next material filing, estimate revision, news event, or price-action inflection',
           'evidence_source', 'normalized evidence_item stream',
           'status', 'open'
       )
   )
 WHERE known_unknowns = '[]'::jsonb;
