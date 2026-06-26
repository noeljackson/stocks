-- IBKR read-only broker sync (#25).
--
-- Manual fills remain thesis-linked. Broker-imported rows may arrive before the
-- operator links them to a thesis, so broker fills can be stored without a
-- thesis_id while still preserving the raw execution payload.

ALTER TABLE position
    ADD COLUMN IF NOT EXISTS source text NOT NULL DEFAULT 'manual',
    ADD COLUMN IF NOT EXISTS broker text,
    ADD COLUMN IF NOT EXISTS broker_account text,
    ADD COLUMN IF NOT EXISTS broker_con_id bigint,
    ADD COLUMN IF NOT EXISTS broker_contract jsonb NOT NULL DEFAULT '{}',
    ADD COLUMN IF NOT EXISTS broker_last_sync_at timestamptz;

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
          FROM pg_constraint
         WHERE conrelid = 'position'::regclass
           AND conname = 'position_source_check'
    ) THEN
        ALTER TABLE position
            ADD CONSTRAINT position_source_check
            CHECK (source IN ('manual','broker'));
    END IF;
END $$;

CREATE UNIQUE INDEX IF NOT EXISTS ux_position_broker_open
    ON position(broker, broker_account, broker_con_id, side, instrument)
 WHERE source = 'broker'
   AND closed_at IS NULL
   AND broker IS NOT NULL
   AND broker_account IS NOT NULL
   AND broker_con_id IS NOT NULL;

ALTER TABLE position_fill
    ALTER COLUMN thesis_id DROP NOT NULL,
    ADD COLUMN IF NOT EXISTS broker text,
    ADD COLUMN IF NOT EXISTS broker_account text,
    ADD COLUMN IF NOT EXISTS broker_execution_id text;

CREATE UNIQUE INDEX IF NOT EXISTS ux_position_fill_broker_execution
    ON position_fill(broker, broker_account, broker_execution_id)
 WHERE source = 'broker'
   AND broker IS NOT NULL
   AND broker_account IS NOT NULL
   AND broker_execution_id IS NOT NULL;
