-- The dependency graph depends on whether each issue is open, not on its
-- status. A move between two open statuses, such as a claim from Todo to
-- In Progress, no longer makes every server rebuild the graph (LLM-469).
-- A team move changes the team and number the graph sorts by, so those
-- count too.
DROP TRIGGER dep_graph_issue_update;
CREATE TRIGGER dep_graph_issue_update AFTER UPDATE OF status_id, deleted_at, team_id, number ON issues
WHEN OLD.deleted_at IS NOT NEW.deleted_at
  OR OLD.team_id IS NOT NEW.team_id
  OR OLD.number IS NOT NEW.number
  OR (OLD.status_id IS NOT NEW.status_id
      AND (SELECT type IN ('completed', 'canceled') FROM statuses WHERE id = OLD.status_id)
          IS NOT (SELECT type IN ('completed', 'canceled') FROM statuses WHERE id = NEW.status_id))
BEGIN UPDATE dep_graph_token SET token = randomblob(16); END;
