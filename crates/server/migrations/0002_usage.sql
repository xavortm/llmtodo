-- Token usage per work session and model.
CREATE TABLE work_session_usage (
    work_session_id TEXT NOT NULL REFERENCES work_sessions (id) ON DELETE CASCADE,
    model           TEXT NOT NULL,
    requests        INTEGER NOT NULL DEFAULT 0,
    input           INTEGER NOT NULL DEFAULT 0,
    output          INTEGER NOT NULL DEFAULT 0,
    cache_write_5m  INTEGER NOT NULL DEFAULT 0,
    cache_write_1h  INTEGER NOT NULL DEFAULT 0,
    cache_read      INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (work_session_id, model)
) STRICT;

-- How much of each transcript's cumulative usage is already attributed.
-- Usage that arrives with a write that touches no ticket stays here
-- unattributed and goes to the next ticket write.
CREATE TABLE transcript_usage_seen (
    transcript     TEXT NOT NULL,
    model          TEXT NOT NULL,
    requests       INTEGER NOT NULL DEFAULT 0,
    input          INTEGER NOT NULL DEFAULT 0,
    output         INTEGER NOT NULL DEFAULT 0,
    cache_write_5m INTEGER NOT NULL DEFAULT 0,
    cache_write_1h INTEGER NOT NULL DEFAULT 0,
    cache_read     INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (transcript, model)
) STRICT;
