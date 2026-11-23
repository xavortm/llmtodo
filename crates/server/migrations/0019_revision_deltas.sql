-- Revisions stop keeping a full copy of every version (LLM-346).
--
-- A row with base_version NULL holds the full text in body. A row with
-- base_version set holds a delta in body: the text of revision base_version
-- of the same entity, edited into this one (see `delta.rs`). The base is
-- always the next newer row, so the newest row of each entity is full, and a
-- trim of old rows never breaks a newer one.
ALTER TABLE revisions ADD COLUMN base_version INTEGER;

-- SQL cannot compute a delta, so the server turns the existing full copies
-- into deltas when it next opens the database, and removes this row in the
-- same transaction.
INSERT INTO settings (key, value) VALUES ('revisions_compact', 'pending');
