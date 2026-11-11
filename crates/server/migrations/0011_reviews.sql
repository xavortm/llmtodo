-- The owner's answers to review rounds. The steps and checks stay in the
-- `+++ Review` section of the description. Each answer also writes an
-- activity event with the field `review`.
CREATE TABLE reviews (
    id         TEXT PRIMARY KEY,
    issue_id   TEXT NOT NULL REFERENCES issues (id) ON DELETE CASCADE,
    round      INTEGER NOT NULL,
    verdict    TEXT NOT NULL CHECK (verdict IN ('approved', 'changes_requested')),
    feedback   TEXT NOT NULL DEFAULT '',
    author_id  TEXT REFERENCES actors (id),
    created_at TEXT NOT NULL,
    UNIQUE (issue_id, round)
) STRICT;
