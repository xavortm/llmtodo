-- LLM-588: a reviewer agent reviews before the owner.
--
-- A review status gets a role. In an `agent` status a reviewer agent passes
-- the ticket on to the owner or asks for changes. In an `owner` status the
-- owner approves or asks for changes. NULL on a review status means the
-- owner, so a team with one review status keeps today's behaviour.
ALTER TABLE statuses ADD COLUMN review_role TEXT CHECK (review_role IN ('agent', 'owner'));

UPDATE statuses SET review_role = 'owner' WHERE review = 1;

-- In Review is the reviewer's step only where the team also has User review.
UPDATE statuses SET review_role = 'agent'
WHERE review = 1
  AND lower(trim(name)) = 'in review'
  AND EXISTS (
      SELECT 1 FROM statuses o
      WHERE o.team_id = statuses.team_id AND o.review = 1 AND lower(trim(o.name)) = 'user review'
  );

-- Two CHECK constraints gain a value: `waiting_on` gets `reviewer`, and a
-- review's verdict gets `passed`. SQLite cannot alter a CHECK, and a table
-- rebuild of `issues` would cascade deletes through its foreign keys. A
-- wider CHECK does not change the stored format, so this edits the schema
-- text in place, as https://sqlite.org/lang_altertable.html#otheralter
-- describes. The index below bumps the schema cookie, so every open
-- connection reads the new schema.
PRAGMA writable_schema = ON;

UPDATE sqlite_schema
SET sql = replace(sql,
    'CHECK (waiting_on IN (''agent'', ''owner''))',
    'CHECK (waiting_on IN (''agent'', ''owner'', ''reviewer''))')
WHERE type = 'table' AND name = 'issues';

UPDATE sqlite_schema
SET sql = replace(sql,
    'CHECK (verdict IN (''approved'', ''changes_requested''))',
    'CHECK (verdict IN (''approved'', ''changes_requested'', ''passed''))')
WHERE type = 'table' AND name = 'reviews';

PRAGMA writable_schema = RESET;

-- Tickets already in the reviewer agent's step wait on that agent now,
-- unless the owner still has an open question to answer.
UPDATE issues SET waiting_on = 'reviewer'
WHERE waiting_on = 'owner'
  AND status_id IN (SELECT id FROM statuses WHERE review_role = 'agent')
  AND NOT EXISTS (
      SELECT 1 FROM questions q WHERE q.issue_id = issues.id AND q.answer IS NULL
  );

CREATE INDEX review_roles_schema_bump ON statuses (review_role);
DROP INDEX review_roles_schema_bump;
