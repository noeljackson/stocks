-- 0004_llm_invocation.sql — audit trail for every LLM call (#6).
--
-- Every cognition-layer service goes through prompts::invoke which records a
-- row here with prompt_name + prompt_hash + token counts. Pairs with the
-- existing config_version pattern (market_state, alerts) — same idea, applied
-- to prompts: every output attributable to the prompt that produced it.

CREATE TABLE IF NOT EXISTS llm_invocation (
    id              bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    prompt_name     text NOT NULL,
    prompt_hash     text NOT NULL,       -- sha256 of the prompt file content
    provider        text NOT NULL,       -- "anthropic" | "openai_compat" | "mock"
    model           text NOT NULL,
    input_tokens    int  NOT NULL DEFAULT 0,
    output_tokens   int  NOT NULL DEFAULT 0,
    latency_ms      int  NOT NULL DEFAULT 0,
    -- Coarse summaries (first N chars or sha hash) so we have audit signal
    -- without retaining full prompt/response payloads in the audit table.
    request_summary text,
    response_summary text,
    at              timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS ix_llm_invocation_prompt   ON llm_invocation(prompt_name, at);
CREATE INDEX IF NOT EXISTS ix_llm_invocation_at_brin  ON llm_invocation USING brin(at);
