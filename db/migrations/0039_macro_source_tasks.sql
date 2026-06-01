-- 0039_macro_source_tasks.sql
--
-- Macro/factor sources are not symbol-scoped, but they still need first-class
-- source_task rows so diagnostics can show ownership, retry, and freshness.

INSERT INTO source_task
    (source_type, requirement_key, action, scope, target_id, provider, limiter_key,
     state, priority, due_at, attempts, next_retry_at, last_error, source_ref)
VALUES
    (
        'macro',
        'macro_regime',
        'fred_macro',
        'benchmark',
        'macro_regime',
        'fred',
        'fred',
        'queued',
        'high',
        now(),
        0,
        NULL,
        NULL,
        '{"created_by":"0039_macro_source_tasks","reason":"macro regime freshness"}'::jsonb
    ),
    (
        'macro',
        'macro_regime',
        'cboe_crowd_sentiment',
        'benchmark',
        'macro_regime',
        'cboe',
        'cboe',
        'queued',
        'high',
        now(),
        0,
        NULL,
        NULL,
        '{"created_by":"0039_macro_source_tasks","reason":"macro crowd sentiment freshness"}'::jsonb
    )
ON CONFLICT (scope, target_id, requirement_key, action) DO UPDATE SET
    source_type = EXCLUDED.source_type,
    provider = EXCLUDED.provider,
    limiter_key = EXCLUDED.limiter_key,
    priority = EXCLUDED.priority,
    source_ref = source_task.source_ref || EXCLUDED.source_ref,
    updated_at = now();
