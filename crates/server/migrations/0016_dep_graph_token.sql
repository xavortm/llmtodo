-- LLM-216: the server keeps the dependency graph in memory, with each
-- issue's depth and has_open_dependents. This one-row table holds a random
-- token. Every write that can change the graph sets a new token, so the
-- server knows when its copy is stale. A random value, not a counter, keeps
-- a rolled-back transaction from leaving a copy that looks current.
CREATE TABLE dep_graph_token (token BLOB NOT NULL);
INSERT INTO dep_graph_token (token) VALUES (randomblob(16));

CREATE TRIGGER dep_graph_relation_insert AFTER INSERT ON relations
WHEN NEW.type = 'blocks'
BEGIN UPDATE dep_graph_token SET token = randomblob(16); END;

CREATE TRIGGER dep_graph_relation_delete AFTER DELETE ON relations
WHEN OLD.type = 'blocks'
BEGIN UPDATE dep_graph_token SET token = randomblob(16); END;

CREATE TRIGGER dep_graph_relation_update AFTER UPDATE ON relations
WHEN OLD.type = 'blocks' OR NEW.type = 'blocks'
BEGIN UPDATE dep_graph_token SET token = randomblob(16); END;

CREATE TRIGGER dep_graph_issue_update AFTER UPDATE OF status_id, deleted_at ON issues
WHEN OLD.status_id IS NOT NEW.status_id OR OLD.deleted_at IS NOT NEW.deleted_at
BEGIN UPDATE dep_graph_token SET token = randomblob(16); END;

CREATE TRIGGER dep_graph_issue_delete AFTER DELETE ON issues
BEGIN UPDATE dep_graph_token SET token = randomblob(16); END;

CREATE TRIGGER dep_graph_status_update AFTER UPDATE OF type ON statuses
WHEN OLD.type IS NOT NEW.type
BEGIN UPDATE dep_graph_token SET token = randomblob(16); END;
