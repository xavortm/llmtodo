-- Who an issue waits on: an agent can pick it up, or the owner must answer.
ALTER TABLE issues ADD COLUMN waiting_on TEXT CHECK (waiting_on IN ('agent', 'owner'));
