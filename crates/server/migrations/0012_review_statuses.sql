-- A status with the review flag holds tickets that wait for the owner's
-- review. A move there needs a review section and ends the agent's claim.
ALTER TABLE statuses ADD COLUMN review INTEGER NOT NULL DEFAULT 0;

UPDATE statuses SET review = 1
WHERE lower(trim(name)) IN ('in review', 'user review');

-- The server ends the live claims on issues in these statuses at its next
-- start, through the normal write path, then deletes this mark.
INSERT INTO settings (key, value) VALUES ('review_claims_sweep', 'pending');
