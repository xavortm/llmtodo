-- LLM-590: the coding agent leaves a ready-to-run preview build of the
-- ticket's branch, so the owner's Try it never waits for a compile. The
-- path is on `preview_build_host`, built from `preview_build_commit`.
ALTER TABLE issues ADD COLUMN preview_build_path TEXT;
ALTER TABLE issues ADD COLUMN preview_build_commit TEXT;
ALTER TABLE issues ADD COLUMN preview_build_host TEXT;
ALTER TABLE issues ADD COLUMN preview_build_at TEXT;
