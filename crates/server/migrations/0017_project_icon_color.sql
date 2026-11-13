-- LLM-196: a project's Lucide icon name and its color as #rrggbb. Agents
-- pick the icon, and the owner picks the color in the app.
ALTER TABLE projects ADD COLUMN icon TEXT;
ALTER TABLE projects ADD COLUMN color TEXT;
