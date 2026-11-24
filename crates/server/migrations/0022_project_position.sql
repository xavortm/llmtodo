-- LLM-524: the owner orders projects in the app's sidebar. Projects list by
-- position, then name, so with every position 0 the order stays by name.
ALTER TABLE projects ADD COLUMN position INTEGER NOT NULL DEFAULT 0;
