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
