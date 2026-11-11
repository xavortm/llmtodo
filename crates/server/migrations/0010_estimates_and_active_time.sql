-- Estimates are set before work starts and stay frozen unless the spec
-- changes. Both are empty until someone sets them.
ALTER TABLE issues ADD COLUMN estimate_minutes INTEGER;
ALTER TABLE issues ADD COLUMN estimate_cost_usd REAL;

-- Active working time of an agent session on one ticket, from transcript
-- timestamps. It adds up the same way as token usage.
ALTER TABLE work_sessions ADD COLUMN active_seconds INTEGER NOT NULL DEFAULT 0;

-- How much of each transcript's cumulative active time is already given to
-- a ticket.
CREATE TABLE transcript_time_seen (
    transcript     TEXT PRIMARY KEY,
    active_seconds INTEGER NOT NULL
) STRICT;
