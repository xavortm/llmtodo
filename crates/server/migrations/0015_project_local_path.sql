-- LLM-213: the local folder of a project's repo, so the app can show it
-- when the project has no linked repo. The CLI saves it from the git root.
ALTER TABLE projects ADD COLUMN local_path TEXT;
