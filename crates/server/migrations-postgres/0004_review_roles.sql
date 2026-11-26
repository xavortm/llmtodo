-- LLM-588: a reviewer agent reviews before the owner. See the SQLite
-- migration 0023 for the rules.
ALTER TABLE statuses ADD COLUMN review_role TEXT CHECK (review_role IN ('agent', 'owner'));

UPDATE statuses SET review_role = 'owner' WHERE review = 1;

UPDATE statuses SET review_role = 'agent'
WHERE review = 1
  AND lower(trim(name)) = 'in review'
  AND EXISTS (
      SELECT 1 FROM statuses o
      WHERE o.team_id = statuses.team_id AND o.review = 1 AND lower(trim(o.name)) = 'user review'
  );

ALTER TABLE issues DROP CONSTRAINT issues_waiting_on_check;
ALTER TABLE issues ADD CONSTRAINT issues_waiting_on_check
    CHECK (waiting_on IN ('agent', 'owner', 'reviewer'));

ALTER TABLE reviews DROP CONSTRAINT reviews_verdict_check;
ALTER TABLE reviews ADD CONSTRAINT reviews_verdict_check
    CHECK (verdict IN ('approved', 'changes_requested', 'passed'));

-- Tickets already in the reviewer agent's step wait on that agent now,
-- unless the owner still has an open question to answer.
UPDATE issues SET waiting_on = 'reviewer'
WHERE waiting_on = 'owner'
  AND status_id IN (SELECT id FROM statuses WHERE review_role = 'agent')
  AND NOT EXISTS (
      SELECT 1 FROM questions q WHERE q.issue_id = issues.id AND q.answer IS NULL
  );
