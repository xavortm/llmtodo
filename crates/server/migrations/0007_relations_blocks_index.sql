-- Walks up a chain of blockers: finds the blockers of an issue. The unique
-- index on (type, from_id, to_id) already covers the walk down. Without this
-- index, SQLite reads that unique index for the walk up too, and scans every
-- `blocks` row once per issue.
CREATE INDEX relations_blocked_by ON relations (type, to_id, from_id);
