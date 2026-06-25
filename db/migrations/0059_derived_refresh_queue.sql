-- 0059_derived_refresh_queue.sql
--
-- Dependency queue for derived operator surfaces. Source data such as thesis,
-- ticker_context, evidence_item, source_task, attention_item, and parent brain
-- rows are facts. Brain links, review packets, the daily journal, and the
-- trade desk are derived views; this queue records when those projections need
-- to be refreshed or re-read from live inputs.

CREATE TABLE IF NOT EXISTS derived_refresh_task (
    id              bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    task_key        text NOT NULL UNIQUE,
    target_kind     text NOT NULL CHECK (target_kind IN (
                        'brain_link',
                        'brain_journal',
                        'trade_desk',
                        'review_packet'
                    )),
    target_id       text NOT NULL,
    symbol          text,
    reason          text NOT NULL,
    dependency_kind text NOT NULL,
    dependency_id   text,
    priority        text NOT NULL DEFAULT 'medium'
                    CHECK (priority IN ('blocking', 'high', 'medium', 'low')),
    state           text NOT NULL DEFAULT 'queued'
                    CHECK (state IN ('queued', 'running', 'satisfied', 'failed', 'blocked')),
    generation      int NOT NULL DEFAULT 1 CHECK (generation > 0),
    due_at          timestamptz NOT NULL DEFAULT now(),
    attempts        int NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    last_error      text,
    source_ref      jsonb NOT NULL DEFAULT '{}'::jsonb,
    started_at      timestamptz,
    completed_at    timestamptz,
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS ix_derived_refresh_due
    ON derived_refresh_task(state, due_at, priority, updated_at);

CREATE INDEX IF NOT EXISTS ix_derived_refresh_symbol
    ON derived_refresh_task(symbol, updated_at DESC)
    WHERE symbol IS NOT NULL;

CREATE OR REPLACE FUNCTION derived_refresh_priority_rank(p_priority text)
RETURNS int
LANGUAGE sql
IMMUTABLE
AS $$
    SELECT CASE p_priority
        WHEN 'blocking' THEN 0
        WHEN 'high' THEN 1
        WHEN 'medium' THEN 2
        ELSE 3
    END;
$$;

CREATE OR REPLACE FUNCTION enqueue_derived_refresh(
    p_target_kind text,
    p_target_id text,
    p_symbol text,
    p_reason text,
    p_dependency_kind text,
    p_dependency_id text,
    p_priority text,
    p_source_ref jsonb,
    p_due_at timestamptz DEFAULT now()
) RETURNS void
LANGUAGE plpgsql
AS $$
DECLARE
    v_task_key text := p_target_kind || ':' || p_target_id;
    v_source_ref jsonb := COALESCE(p_source_ref, '{}'::jsonb);
BEGIN
    INSERT INTO derived_refresh_task (
        task_key, target_kind, target_id, symbol, reason, dependency_kind,
        dependency_id, priority, state, due_at, source_ref
    )
    VALUES (
        v_task_key,
        p_target_kind,
        p_target_id,
        p_symbol,
        p_reason,
        p_dependency_kind,
        p_dependency_id,
        COALESCE(NULLIF(p_priority, ''), 'medium'),
        'queued',
        COALESCE(p_due_at, now()),
        jsonb_build_object(
            'last_reason', p_reason,
            'last_dependency_kind', p_dependency_kind,
            'last_dependency_id', p_dependency_id,
            'last_enqueued_at', now(),
            'last_event', v_source_ref
        )
    )
    ON CONFLICT (task_key) DO UPDATE SET
        symbol = COALESCE(EXCLUDED.symbol, derived_refresh_task.symbol),
        reason = EXCLUDED.reason,
        dependency_kind = EXCLUDED.dependency_kind,
        dependency_id = EXCLUDED.dependency_id,
        priority = CASE
            WHEN derived_refresh_priority_rank(EXCLUDED.priority)
               < derived_refresh_priority_rank(derived_refresh_task.priority)
            THEN EXCLUDED.priority
            ELSE derived_refresh_task.priority
        END,
        state = CASE
            WHEN derived_refresh_task.state = 'running' THEN 'running'
            ELSE 'queued'
        END,
        generation = derived_refresh_task.generation + 1,
        due_at = CASE
            WHEN derived_refresh_task.state IN ('queued', 'running', 'failed')
            THEN LEAST(derived_refresh_task.due_at, EXCLUDED.due_at)
            ELSE EXCLUDED.due_at
        END,
        started_at = CASE
            WHEN derived_refresh_task.state = 'running' THEN derived_refresh_task.started_at
            ELSE NULL
        END,
        completed_at = CASE
            WHEN derived_refresh_task.state = 'running' THEN derived_refresh_task.completed_at
            ELSE NULL
        END,
        last_error = NULL,
        source_ref = derived_refresh_task.source_ref || jsonb_build_object(
            'last_reason', EXCLUDED.reason,
            'last_dependency_kind', EXCLUDED.dependency_kind,
            'last_dependency_id', EXCLUDED.dependency_id,
            'last_enqueued_at', now(),
            'last_event', v_source_ref
        ),
        updated_at = now();
END;
$$;

CREATE OR REPLACE FUNCTION derived_refresh_utc_day(p_at timestamptz)
RETURNS text
LANGUAGE sql
STABLE
AS $$
    SELECT ((COALESCE(p_at, now()) AT TIME ZONE 'UTC')::date)::text;
$$;

CREATE OR REPLACE FUNCTION trg_derived_refresh_from_thesis()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    v_day text := derived_refresh_utc_day(COALESCE(NEW.updated_at, now()));
    v_changed bool;
    v_ref jsonb := jsonb_build_object(
        'thesis_id', NEW.thesis_id,
        'symbol', NEW.symbol,
        'state', NEW.state,
        'version', NEW.version,
        'direction', NEW.forecast->>'direction',
        'conviction_tier', NEW.conviction_tier,
        'system_confidence', NEW.system_confidence
    );
BEGIN
    IF TG_OP = 'INSERT' THEN
        v_changed := true;
    ELSE
        v_changed := NEW.state IS DISTINCT FROM OLD.state
            OR NEW.forecast IS DISTINCT FROM OLD.forecast
            OR NEW.conviction_tier IS DISTINCT FROM OLD.conviction_tier
            OR NEW.system_confidence IS DISTINCT FROM OLD.system_confidence
            OR NEW.version IS DISTINCT FROM OLD.version
            OR NEW.edge_rationale IS DISTINCT FROM OLD.edge_rationale
            OR NEW.last_evaluated_at IS DISTINCT FROM OLD.last_evaluated_at;
    END IF;

    IF v_changed THEN
        PERFORM enqueue_derived_refresh(
            'brain_link', NEW.symbol, NEW.symbol, 'thesis_changed',
            'thesis', NEW.thesis_id::text, 'high', v_ref
        );
        PERFORM enqueue_derived_refresh(
            'brain_journal', v_day, NULL, 'thesis_changed',
            'thesis', NEW.thesis_id::text, 'high', v_ref, now() + interval '5 seconds'
        );
        PERFORM enqueue_derived_refresh(
            'trade_desk', v_day, NULL, 'thesis_changed',
            'thesis', NEW.thesis_id::text, 'high', v_ref, now() + interval '5 seconds'
        );
        PERFORM enqueue_derived_refresh(
            'review_packet', NEW.symbol, NEW.symbol, 'thesis_changed',
            'thesis', NEW.thesis_id::text, 'medium', v_ref, now() + interval '10 seconds'
        );
    END IF;
    RETURN NEW;
END;
$$;

DROP TRIGGER IF EXISTS trg_derived_refresh_from_thesis ON thesis;
CREATE TRIGGER trg_derived_refresh_from_thesis
AFTER INSERT OR UPDATE ON thesis
FOR EACH ROW
EXECUTE FUNCTION trg_derived_refresh_from_thesis();

CREATE OR REPLACE FUNCTION trg_derived_refresh_from_ticker_context()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    v_day text := derived_refresh_utc_day(NEW.created_at);
    v_ref jsonb := jsonb_build_object(
        'symbol', NEW.symbol,
        'version', NEW.version,
        'created_at', NEW.created_at
    );
BEGIN
    PERFORM enqueue_derived_refresh(
        'brain_link', NEW.symbol, NEW.symbol, 'context_changed',
        'ticker_context', NEW.symbol || ':' || NEW.version::text, 'medium', v_ref
    );
    PERFORM enqueue_derived_refresh(
        'brain_journal', v_day, NULL, 'context_changed',
        'ticker_context', NEW.symbol || ':' || NEW.version::text, 'medium', v_ref, now() + interval '30 seconds'
    );
    PERFORM enqueue_derived_refresh(
        'trade_desk', v_day, NULL, 'context_changed',
        'ticker_context', NEW.symbol || ':' || NEW.version::text, 'medium', v_ref, now() + interval '30 seconds'
    );
    PERFORM enqueue_derived_refresh(
        'review_packet', NEW.symbol, NEW.symbol, 'context_changed',
        'ticker_context', NEW.symbol || ':' || NEW.version::text, 'medium', v_ref, now() + interval '15 seconds'
    );
    RETURN NEW;
END;
$$;

DROP TRIGGER IF EXISTS trg_derived_refresh_from_ticker_context ON ticker_context;
CREATE TRIGGER trg_derived_refresh_from_ticker_context
AFTER INSERT ON ticker_context
FOR EACH ROW
EXECUTE FUNCTION trg_derived_refresh_from_ticker_context();

CREATE OR REPLACE FUNCTION trg_derived_refresh_from_evidence_item()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    v_changed_at timestamptz := COALESCE(NEW.updated_at, NEW.created_at, now());
    v_day text := derived_refresh_utc_day(v_changed_at);
    v_changed bool;
    v_ref jsonb := jsonb_build_object(
        'evidence_item_id', NEW.id,
        'symbol', NEW.symbol,
        'kind', NEW.kind,
        'source', NEW.source,
        'source_id', NEW.source_id
    );
BEGIN
    IF TG_OP = 'INSERT' THEN
        v_changed := true;
    ELSE
        v_changed := NEW.updated_at IS DISTINCT FROM OLD.updated_at
            OR NEW.summary IS DISTINCT FROM OLD.summary
            OR NEW.strength IS DISTINCT FROM OLD.strength
            OR NEW.polarity IS DISTINCT FROM OLD.polarity
            OR NEW.source_ref IS DISTINCT FROM OLD.source_ref;
    END IF;

    IF v_changed THEN
        PERFORM enqueue_derived_refresh(
            'brain_journal', v_day, NULL, 'evidence_changed',
            'evidence_item', NEW.id::text, 'medium', v_ref, now() + interval '60 seconds'
        );
        PERFORM enqueue_derived_refresh(
            'trade_desk', v_day, NULL, 'evidence_changed',
            'evidence_item', NEW.id::text, 'medium', v_ref, now() + interval '60 seconds'
        );
        PERFORM enqueue_derived_refresh(
            'review_packet', NEW.symbol, NEW.symbol, 'evidence_changed',
            'evidence_item', NEW.id::text, 'medium', v_ref, now() + interval '20 seconds'
        );
    END IF;
    RETURN NEW;
END;
$$;

DROP TRIGGER IF EXISTS trg_derived_refresh_from_evidence_item ON evidence_item;
CREATE TRIGGER trg_derived_refresh_from_evidence_item
AFTER INSERT OR UPDATE ON evidence_item
FOR EACH ROW
EXECUTE FUNCTION trg_derived_refresh_from_evidence_item();

CREATE OR REPLACE FUNCTION trg_derived_refresh_from_source_task()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    v_changed_at timestamptz := COALESCE(NEW.updated_at, now());
    v_day text := derived_refresh_utc_day(v_changed_at);
    v_symbol text := CASE WHEN NEW.scope = 'symbol' THEN NEW.target_id ELSE NULL END;
    v_changed bool;
    v_ref jsonb := jsonb_build_object(
        'source_task_id', NEW.id,
        'scope', NEW.scope,
        'target_id', NEW.target_id,
        'action', NEW.action,
        'provider', NEW.provider,
        'state', NEW.state,
        'result', NEW.source_ref->>'result'
    );
BEGIN
    IF TG_OP = 'INSERT' THEN
        v_changed := true;
    ELSE
        v_changed := NEW.state IS DISTINCT FROM OLD.state
            OR NEW.source_ref IS DISTINCT FROM OLD.source_ref
            OR NEW.last_error IS DISTINCT FROM OLD.last_error
            OR NEW.updated_at IS DISTINCT FROM OLD.updated_at;
    END IF;

    IF v_changed THEN
        PERFORM enqueue_derived_refresh(
            'brain_journal', v_day, NULL, 'source_task_changed',
            'source_task', NEW.id::text, 'medium', v_ref, now() + interval '60 seconds'
        );
        PERFORM enqueue_derived_refresh(
            'trade_desk', v_day, NULL, 'source_task_changed',
            'source_task', NEW.id::text, 'medium', v_ref, now() + interval '60 seconds'
        );
        IF v_symbol IS NOT NULL THEN
            PERFORM enqueue_derived_refresh(
                'review_packet', v_symbol, v_symbol, 'source_task_changed',
                'source_task', NEW.id::text, 'medium', v_ref, now() + interval '20 seconds'
            );
        END IF;
    END IF;
    RETURN NEW;
END;
$$;

DROP TRIGGER IF EXISTS trg_derived_refresh_from_source_task ON source_task;
CREATE TRIGGER trg_derived_refresh_from_source_task
AFTER INSERT OR UPDATE ON source_task
FOR EACH ROW
EXECUTE FUNCTION trg_derived_refresh_from_source_task();

CREATE OR REPLACE FUNCTION trg_derived_refresh_from_attention_item()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    v_changed_at timestamptz := COALESCE(NEW.resolved_at, NEW.created_at, now());
    v_day text := derived_refresh_utc_day(v_changed_at);
    v_changed bool;
    v_ref jsonb := jsonb_build_object(
        'attention_id', NEW.id,
        'kind', NEW.kind,
        'symbol', NEW.symbol,
        'status', NEW.status,
        'fsm_state', NEW.fsm_state,
        'owner', NEW.owner
    );
BEGIN
    IF TG_OP = 'INSERT' THEN
        v_changed := true;
    ELSE
        v_changed := NEW.status IS DISTINCT FROM OLD.status
            OR NEW.fsm_state IS DISTINCT FROM OLD.fsm_state
            OR NEW.owner IS DISTINCT FROM OLD.owner
            OR NEW.resolved_at IS DISTINCT FROM OLD.resolved_at
            OR NEW.resurface_at IS DISTINCT FROM OLD.resurface_at;
    END IF;

    IF v_changed THEN
        PERFORM enqueue_derived_refresh(
            'brain_journal', v_day, NULL, 'attention_changed',
            'attention_item', NEW.id::text, 'medium', v_ref, now() + interval '10 seconds'
        );
        PERFORM enqueue_derived_refresh(
            'trade_desk', v_day, NULL, 'attention_changed',
            'attention_item', NEW.id::text, 'medium', v_ref, now() + interval '10 seconds'
        );
        IF NEW.symbol IS NOT NULL THEN
            PERFORM enqueue_derived_refresh(
                'review_packet', NEW.symbol, NEW.symbol, 'attention_changed',
                'attention_item', NEW.id::text, 'medium', v_ref, now() + interval '10 seconds'
            );
        END IF;
    END IF;
    RETURN NEW;
END;
$$;

DROP TRIGGER IF EXISTS trg_derived_refresh_from_attention_item ON attention_item;
CREATE TRIGGER trg_derived_refresh_from_attention_item
AFTER INSERT OR UPDATE ON attention_item
FOR EACH ROW
EXECUTE FUNCTION trg_derived_refresh_from_attention_item();

CREATE OR REPLACE FUNCTION trg_derived_refresh_from_brain_thesis()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    v_day text := derived_refresh_utc_day(COALESCE(NEW.updated_at, now()));
    v_symbol text;
    v_changed bool;
    v_ref jsonb := jsonb_build_object(
        'brain_thesis_id', NEW.id,
        'scope', NEW.scope,
        'key', NEW.key,
        'state', NEW.state,
        'direction', NEW.direction,
        'version', NEW.version
    );
BEGIN
    IF TG_OP = 'INSERT' THEN
        v_changed := true;
    ELSE
        v_changed := NEW.state IS DISTINCT FROM OLD.state
            OR NEW.direction IS DISTINCT FROM OLD.direction
            OR NEW.summary IS DISTINCT FROM OLD.summary
            OR NEW.core_claim IS DISTINCT FROM OLD.core_claim
            OR NEW.version IS DISTINCT FROM OLD.version
            OR NEW.updated_at IS DISTINCT FROM OLD.updated_at;
    END IF;

    IF v_changed THEN
        PERFORM enqueue_derived_refresh(
            'brain_journal', v_day, NULL, 'parent_brain_changed',
            'brain_thesis', NEW.id::text, 'high', v_ref, now() + interval '15 seconds'
        );
        PERFORM enqueue_derived_refresh(
            'trade_desk', v_day, NULL, 'parent_brain_changed',
            'brain_thesis', NEW.id::text, 'high', v_ref, now() + interval '15 seconds'
        );
        FOR v_symbol IN
            SELECT symbol FROM brain_thesis_ticker WHERE brain_thesis_id = NEW.id
        LOOP
            PERFORM enqueue_derived_refresh(
                'brain_link', v_symbol, v_symbol, 'parent_brain_changed',
                'brain_thesis', NEW.id::text, 'medium', v_ref
            );
            PERFORM enqueue_derived_refresh(
                'review_packet', v_symbol, v_symbol, 'parent_brain_changed',
                'brain_thesis', NEW.id::text, 'medium', v_ref
            );
        END LOOP;
    END IF;
    RETURN NEW;
END;
$$;

DROP TRIGGER IF EXISTS trg_derived_refresh_from_brain_thesis ON brain_thesis;
CREATE TRIGGER trg_derived_refresh_from_brain_thesis
AFTER INSERT OR UPDATE ON brain_thesis
FOR EACH ROW
EXECUTE FUNCTION trg_derived_refresh_from_brain_thesis();

CREATE OR REPLACE FUNCTION trg_derived_refresh_from_brain_thesis_ticker()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    v_symbol text;
    v_brain_thesis_id uuid;
    v_role text;
    v_conviction int;
    v_ref jsonb;
BEGIN
    IF TG_OP = 'DELETE' THEN
        v_symbol := OLD.symbol;
        v_brain_thesis_id := OLD.brain_thesis_id;
        v_role := OLD.role;
        v_conviction := OLD.conviction;
    ELSE
        v_symbol := NEW.symbol;
        v_brain_thesis_id := NEW.brain_thesis_id;
        v_role := NEW.role;
        v_conviction := NEW.conviction;
    END IF;

    v_ref := jsonb_build_object(
        'brain_thesis_id', v_brain_thesis_id,
        'symbol', v_symbol,
        'role', v_role,
        'conviction', v_conviction,
        'operation', TG_OP
    );

    PERFORM enqueue_derived_refresh(
        'brain_link', v_symbol, v_symbol, 'brain_link_mapping_changed',
        'brain_thesis_ticker', v_brain_thesis_id::text || ':' || v_symbol, 'high', v_ref
    );
    PERFORM enqueue_derived_refresh(
        'review_packet', v_symbol, v_symbol, 'brain_link_mapping_changed',
        'brain_thesis_ticker', v_brain_thesis_id::text || ':' || v_symbol, 'medium', v_ref
    );
    PERFORM enqueue_derived_refresh(
        'trade_desk', derived_refresh_utc_day(now()), NULL, 'brain_link_mapping_changed',
        'brain_thesis_ticker', v_brain_thesis_id::text || ':' || v_symbol, 'medium', v_ref, now() + interval '15 seconds'
    );
    IF TG_OP = 'DELETE' THEN
        RETURN OLD;
    END IF;
    RETURN NEW;
END;
$$;

DROP TRIGGER IF EXISTS trg_derived_refresh_from_brain_thesis_ticker ON brain_thesis_ticker;
CREATE TRIGGER trg_derived_refresh_from_brain_thesis_ticker
AFTER INSERT OR UPDATE OR DELETE ON brain_thesis_ticker
FOR EACH ROW
EXECUTE FUNCTION trg_derived_refresh_from_brain_thesis_ticker();
