-- 0035_source_task_freshness_due.sql
--
-- Satisfied source tasks are not terminal; they are fresh-until due_at. The
-- acquisition worker can then claim due satisfied tasks for recurring refreshes.

CREATE INDEX IF NOT EXISTS ix_source_task_freshness_due
    ON source_task(priority, due_at)
    WHERE state IN ('queued', 'no_rows', 'failed', 'rate_limited', 'blocked', 'satisfied');
