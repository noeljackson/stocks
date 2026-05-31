-- 0009_thesis_suggestion.sql — sharpen + challenge LLM output (#12, #13).
--
-- Both passes write here. The thesis row stays immutable until the user
-- accepts a suggestion — keeps LLM proposals reviewable + dismissible.
--
-- Sharpen pass output: kind='condition_suggestion', content includes
--   the proposed Condition + which role it should land under.
-- Challenge pass output: kind='flag', content describes the weak spot
--   + suggested fix.

CREATE TABLE IF NOT EXISTS thesis_suggestion (
    id              bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    thesis_id       uuid REFERENCES thesis(thesis_id),
    kind            text NOT NULL CHECK (kind IN ('condition_suggestion', 'flag')),
    content         jsonb NOT NULL,
    status          text NOT NULL DEFAULT 'pending'
                    CHECK (status IN ('pending', 'accepted', 'rejected', 'dismissed')),
    user_rationale  text,                          -- why the user accepted / dismissed
    prompt_name     text NOT NULL,
    prompt_hash     text NOT NULL,
    created_at      timestamptz NOT NULL DEFAULT now(),
    decided_at      timestamptz
);
CREATE INDEX IF NOT EXISTS ix_ts_open ON thesis_suggestion(thesis_id, status, created_at DESC);
