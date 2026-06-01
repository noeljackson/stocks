-- Manual execution bridge: decisions can now create trade tickets, append-only
-- fills, and real positions linked back to theses.

ALTER TABLE position
    ADD COLUMN IF NOT EXISTS side text;

UPDATE position
   SET side = CASE WHEN qty < 0 THEN 'short' ELSE 'long' END
 WHERE side IS NULL;

ALTER TABLE position DROP CONSTRAINT IF EXISTS position_instrument_check;
ALTER TABLE position
    ADD CONSTRAINT position_instrument_check
    CHECK (instrument IN ('equity','leaps','options'));

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
          FROM pg_constraint
         WHERE conrelid = 'position'::regclass
           AND conname = 'position_side_check'
    ) THEN
        ALTER TABLE position
            ADD CONSTRAINT position_side_check
            CHECK (side IS NULL OR side IN ('long','short','call','put','hedge'));
    END IF;
END $$;

CREATE TABLE IF NOT EXISTS trade_ticket (
    ticket_id       uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    thesis_id       uuid NOT NULL REFERENCES thesis(thesis_id),
    decision_id     uuid REFERENCES decision(decision_id),
    symbol          text NOT NULL REFERENCES ticker(symbol),
    action          text NOT NULL CHECK (action IN ('enter','exit','resize','skip')),
    side            text CHECK (side IN ('long','short','call','put','hedge') OR side IS NULL),
    instrument      text CHECK (instrument IN ('equity','leaps','options') OR instrument IS NULL),
    intended_size   jsonb NOT NULL DEFAULT '{}',
    risk_result     jsonb NOT NULL DEFAULT '{}',
    status          text NOT NULL DEFAULT 'proposed'
                    CHECK (status IN ('proposed','accepted','rejected','filled','cancelled')),
    created_at      timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS ix_trade_ticket_thesis_created
    ON trade_ticket(thesis_id, created_at DESC);
CREATE INDEX IF NOT EXISTS ix_trade_ticket_symbol_created
    ON trade_ticket(symbol, created_at DESC);

CREATE TABLE IF NOT EXISTS position_fill (
    fill_id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    position_id     uuid NOT NULL REFERENCES position(position_id),
    ticket_id       uuid REFERENCES trade_ticket(ticket_id),
    decision_id     uuid REFERENCES decision(decision_id),
    thesis_id       uuid NOT NULL REFERENCES thesis(thesis_id),
    symbol          text NOT NULL REFERENCES ticker(symbol),
    side            text NOT NULL CHECK (side IN ('long','short','call','put','hedge')),
    instrument      text NOT NULL CHECK (instrument IN ('equity','leaps','options')),
    qty             numeric NOT NULL CHECK (qty > 0),
    price           numeric NOT NULL CHECK (price > 0),
    fees            numeric NOT NULL DEFAULT 0 CHECK (fees >= 0),
    filled_at       timestamptz NOT NULL DEFAULT now(),
    source          text NOT NULL DEFAULT 'manual' CHECK (source IN ('manual','broker')),
    notes           text,
    raw             jsonb NOT NULL DEFAULT '{}',
    created_at      timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS ix_position_fill_position_time
    ON position_fill(position_id, filled_at DESC);
CREATE INDEX IF NOT EXISTS ix_position_fill_symbol_time
    ON position_fill(symbol, filled_at DESC);
