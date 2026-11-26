-- LLM-590: a ready-to-run preview build of the ticket's branch. See the
-- SQLite migration 0024.
ALTER TABLE issues ADD COLUMN preview_build_path TEXT;
ALTER TABLE issues ADD COLUMN preview_build_commit TEXT;
ALTER TABLE issues ADD COLUMN preview_build_host TEXT;
ALTER TABLE issues ADD COLUMN preview_build_at TEXT;
