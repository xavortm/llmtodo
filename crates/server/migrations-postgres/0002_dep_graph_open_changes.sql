-- The dependency graph depends on whether each issue is open, not on its
-- status. A move between two open statuses, such as a claim from Todo to
-- In Progress, no longer makes every server rebuild the graph (LLM-469).
-- A team move changes the team and number the graph sorts by, so those
-- count too. A trigger's WHEN cannot read other tables, so the function
-- compares the two statuses.
CREATE FUNCTION dep_graph_issue_bump() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.deleted_at IS DISTINCT FROM NEW.deleted_at
       OR OLD.team_id IS DISTINCT FROM NEW.team_id
       OR OLD.number IS DISTINCT FROM NEW.number
       OR (SELECT type IN ('completed', 'canceled') FROM statuses WHERE id = OLD.status_id)
          IS DISTINCT FROM (SELECT type IN ('completed', 'canceled') FROM statuses WHERE id = NEW.status_id)
    THEN
        UPDATE dep_graph_token SET token = decode(md5(random()::text || clock_timestamp()::text), 'hex');
    END IF;
    RETURN NULL;
END $$;

DROP TRIGGER dep_graph_issue_update ON issues;
CREATE TRIGGER dep_graph_issue_update AFTER UPDATE OF status_id, deleted_at, team_id, number ON issues
    FOR EACH ROW WHEN (OLD.status_id IS DISTINCT FROM NEW.status_id
        OR OLD.deleted_at IS DISTINCT FROM NEW.deleted_at
        OR OLD.team_id IS DISTINCT FROM NEW.team_id
        OR OLD.number IS DISTINCT FROM NEW.number)
    EXECUTE FUNCTION dep_graph_issue_bump();
