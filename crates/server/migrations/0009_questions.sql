-- Owner questions, derived from the `+++ Owner questions` section of each
-- description. The Markdown stays the record. The server rewrites an issue's
-- rows in the transaction that changes its description.
--
-- The answered_ columns describe the revision that took the question from
-- open to answered. They are NULL when no revision shows that step.
CREATE TABLE questions (
    issue_id          TEXT NOT NULL REFERENCES issues (id) ON DELETE CASCADE,
    position          INTEGER NOT NULL,
    number            INTEGER NOT NULL,
    text              TEXT NOT NULL,
    context           TEXT NOT NULL,
    options           TEXT NOT NULL,
    answer            TEXT,
    answered_version  INTEGER,
    answered_by       TEXT REFERENCES actors (id),
    answered_question TEXT,
    PRIMARY KEY (issue_id, position)
) STRICT;

CREATE INDEX questions_open ON questions (issue_id) WHERE answer IS NULL;

-- SQL cannot parse Markdown, so the server fills the table in Rust when it
-- next opens the database, and removes this row in the same transaction.
INSERT INTO settings (key, value) VALUES ('questions_backfill', 'pending');
