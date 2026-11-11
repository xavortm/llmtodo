-- An agent that reads or fills in an issue without a claim. One mark per
-- issue: a new mark replaces the old one. It never blocks other writers.
-- Reads drop a mark whose seen_at is older than the expiry.
CREATE TABLE analyzing (
    issue_id TEXT PRIMARY KEY REFERENCES issues (id) ON DELETE CASCADE,
    actor_id TEXT NOT NULL REFERENCES actors (id),
    session_id TEXT NOT NULL,
    started_at TEXT NOT NULL,
    seen_at TEXT NOT NULL
);
