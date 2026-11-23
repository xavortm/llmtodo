-- The Postgres schema for the shared backend (LLM-453). It matches the
-- SQLite schema after migrations/0020_file_content.sql, table by table
-- and column by column. A test checks that both have the same tables and
-- columns. Every later schema change needs a migration in both folders.
--
-- Types follow SQLite's storage, so one set of queries and row types serves
-- both: IDs and timestamps are TEXT, integers and 0/1 flags are BIGINT,
-- reals are DOUBLE PRECISION and blobs are BYTEA.

CREATE EXTENSION IF NOT EXISTS pg_trgm;

-- SQLite's NOCASE: equal and ordered regardless of case. Queries write
-- `COLLATE NOCASE`, which Postgres reads as this collation.
CREATE COLLATION IF NOT EXISTS nocase (provider = icu, locale = 'und-u-ks-level2', deterministic = false);

-- The SQLite JSON functions the queries use, for text arguments.
-- json_each gives an array's values with a null key, or an object's keys
-- and values, as SQLite's does.
CREATE FUNCTION json_each(j TEXT) RETURNS TABLE (key TEXT, value TEXT)
    LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$
    SELECT NULL::text, e FROM json_array_elements_text(
        CASE WHEN json_typeof(j::json) = 'array' THEN j::json ELSE '[]'::json END) AS e
    UNION ALL
    SELECT t.k, t.v FROM json_each_text(
        CASE WHEN json_typeof(j::json) = 'object' THEN j::json ELSE '{}'::json END) AS t (k, v)
$$;

CREATE FUNCTION json_valid(j TEXT) RETURNS BOOLEAN
    LANGUAGE plpgsql IMMUTABLE PARALLEL SAFE AS $$
BEGIN
    PERFORM j::json;
    RETURN j IS NOT NULL;
EXCEPTION WHEN others THEN
    RETURN FALSE;
END $$;

-- Only the path form the queries use: `$."key"`.
CREATE FUNCTION json_extract(j TEXT, path TEXT) RETURNS TEXT
    LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$
    SELECT j::json ->> substr(path, 4, length(path) - 4)
$$;

-- SQLite's instr: the 1-based position of b in a, or 0.
CREATE FUNCTION instr(a TEXT, b TEXT) RETURNS BIGINT
    LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$ SELECT strpos(a, b)::bigint $$;

CREATE TABLE settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE actors (
    id         TEXT PRIMARY KEY,
    kind       TEXT NOT NULL CHECK (kind IN ('human', 'agent', 'system')),
    name       TEXT NOT NULL,
    harness    TEXT,
    model      TEXT,
    effort     TEXT,
    is_owner   BIGINT NOT NULL DEFAULT 0,
    linear_id  TEXT UNIQUE,
    created_at TEXT NOT NULL
);
CREATE UNIQUE INDEX actors_agent_identity ON actors (coalesce(harness, ''), coalesce(model, ''), coalesce(effort, '')) WHERE kind = 'agent';
CREATE UNIQUE INDEX actors_named ON actors (kind, name) WHERE kind <> 'agent';
CREATE UNIQUE INDEX actors_one_owner ON actors (is_owner) WHERE is_owner = 1;

CREATE TABLE teams (
    id                  TEXT PRIMARY KEY,
    key                 TEXT NOT NULL UNIQUE,
    name                TEXT NOT NULL,
    next_number         BIGINT NOT NULL DEFAULT 1,
    pr_open_status_id   TEXT,
    pr_merged_status_id TEXT,
    linear_id           TEXT UNIQUE,
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL
);

CREATE TABLE statuses (
    id        TEXT PRIMARY KEY,
    team_id   TEXT NOT NULL REFERENCES teams (id) ON DELETE CASCADE,
    name      TEXT NOT NULL,
    type      TEXT NOT NULL CHECK (type IN ('backlog', 'unstarted', 'started', 'completed', 'canceled')),
    color     TEXT NOT NULL,
    position  BIGINT NOT NULL,
    linear_id TEXT UNIQUE,
    review    BIGINT NOT NULL DEFAULT 0
);
CREATE UNIQUE INDEX statuses_team_name ON statuses (team_id, name COLLATE nocase);

-- SQLite declares these two references inline. Postgres needs statuses to
-- exist first.
ALTER TABLE teams
    ADD FOREIGN KEY (pr_open_status_id) REFERENCES statuses (id) ON DELETE SET NULL,
    ADD FOREIGN KEY (pr_merged_status_id) REFERENCES statuses (id) ON DELETE SET NULL;

CREATE TABLE labels (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    group_id    TEXT REFERENCES labels (id) ON DELETE CASCADE,
    is_group    BIGINT NOT NULL DEFAULT 0,
    color       TEXT NOT NULL,
    description TEXT,
    linear_id   TEXT UNIQUE,
    created_at  TEXT NOT NULL
);
CREATE UNIQUE INDEX labels_name ON labels (coalesce(group_id, ''), name COLLATE nocase);

CREATE TABLE projects (
    id          TEXT PRIMARY KEY,
    slug        TEXT NOT NULL COLLATE nocase UNIQUE,
    name        TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    state       TEXT NOT NULL,
    linear_id   TEXT UNIQUE,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    deleted_at  TEXT,
    local_path  TEXT,
    icon        TEXT,
    color       TEXT
);

CREATE TABLE project_teams (
    project_id TEXT NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    team_id    TEXT NOT NULL REFERENCES teams (id) ON DELETE CASCADE,
    PRIMARY KEY (project_id, team_id)
);

CREATE TABLE project_repos (
    project_id     TEXT NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    repo           TEXT NOT NULL COLLATE nocase,
    etag           TEXT,
    last_polled_at TEXT,
    PRIMARY KEY (project_id, repo)
);

CREATE TABLE milestones (
    id          TEXT PRIMARY KEY,
    project_id  TEXT NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    target_date TEXT,
    position    BIGINT NOT NULL DEFAULT 0,
    linear_id   TEXT UNIQUE,
    created_at  TEXT NOT NULL
);
CREATE UNIQUE INDEX milestones_project_name ON milestones (project_id, name COLLATE nocase);

CREATE TABLE issues (
    id                  TEXT PRIMARY KEY,
    team_id             TEXT NOT NULL REFERENCES teams (id),
    number              BIGINT NOT NULL,
    key                 TEXT NOT NULL UNIQUE,
    title               TEXT NOT NULL,
    description         TEXT NOT NULL DEFAULT '',
    description_version BIGINT NOT NULL DEFAULT 1,
    status_id           TEXT NOT NULL REFERENCES statuses (id),
    priority            BIGINT NOT NULL DEFAULT 0,
    assignee_id         TEXT REFERENCES actors (id),
    project_id          TEXT REFERENCES projects (id) ON DELETE SET NULL,
    milestone_id        TEXT REFERENCES milestones (id) ON DELETE SET NULL,
    parent_id           TEXT REFERENCES issues (id) ON DELETE SET NULL,
    wanted_executor     TEXT NOT NULL DEFAULT '{}',
    linear_id           TEXT UNIQUE,
    created_by          TEXT REFERENCES actors (id),
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL,
    completed_at        TEXT,
    deleted_at          TEXT,
    waiting_on          TEXT CHECK (waiting_on IN ('agent', 'owner')),
    git_branch          TEXT,
    git_worktree        TEXT,
    git_host            TEXT,
    git_pr              TEXT,
    estimate_minutes    BIGINT,
    estimate_cost_usd   DOUBLE PRECISION,
    order_step          BIGINT CHECK (order_step IS NULL OR order_step > 0),
    UNIQUE (team_id, number)
);
CREATE INDEX issues_parent ON issues (parent_id);
CREATE INDEX issues_project ON issues (project_id);
CREATE INDEX issues_status ON issues (status_id);
CREATE INDEX issues_assignee ON issues (assignee_id);
CREATE INDEX issues_default_sort ON issues (
    (CASE WHEN priority = 0 THEN 5 ELSE priority END),
    updated_at DESC,
    id
) WHERE deleted_at IS NULL;
CREATE INDEX issues_parent_order ON issues (parent_id, order_step);

CREATE TABLE issue_keys (
    key      TEXT PRIMARY KEY,
    issue_id TEXT NOT NULL REFERENCES issues (id) ON DELETE CASCADE
);

CREATE TABLE issue_labels (
    issue_id TEXT NOT NULL REFERENCES issues (id) ON DELETE CASCADE,
    label_id TEXT NOT NULL REFERENCES labels (id) ON DELETE CASCADE,
    PRIMARY KEY (issue_id, label_id)
);

CREATE TABLE relations (
    id         TEXT PRIMARY KEY,
    type       TEXT NOT NULL CHECK (type IN ('blocks', 'related', 'duplicate')),
    from_id    TEXT NOT NULL REFERENCES issues (id) ON DELETE CASCADE,
    to_id      TEXT NOT NULL REFERENCES issues (id) ON DELETE CASCADE,
    linear_id  TEXT UNIQUE,
    created_by TEXT REFERENCES actors (id),
    created_at TEXT NOT NULL,
    source     TEXT NOT NULL DEFAULT 'manual' CHECK (source IN ('manual', 'order')),
    UNIQUE (type, from_id, to_id),
    CHECK (from_id <> to_id)
);
CREATE INDEX relations_to ON relations (to_id);
CREATE INDEX relations_blocked_by ON relations (type, to_id, from_id);

CREATE TABLE attachments (
    id         TEXT PRIMARY KEY,
    issue_id   TEXT NOT NULL REFERENCES issues (id) ON DELETE CASCADE,
    kind       TEXT NOT NULL,
    url        TEXT NOT NULL,
    title      TEXT,
    subtitle   TEXT,
    source     TEXT NOT NULL DEFAULT 'manual',
    metadata   TEXT NOT NULL DEFAULT '{}',
    linear_id  TEXT UNIQUE,
    created_by TEXT REFERENCES actors (id),
    created_at TEXT NOT NULL,
    UNIQUE (issue_id, url)
);
CREATE INDEX attachments_url ON attachments (url);

CREATE TABLE comments (
    id         TEXT PRIMARY KEY,
    issue_id   TEXT NOT NULL REFERENCES issues (id) ON DELETE CASCADE,
    parent_id  TEXT REFERENCES comments (id) ON DELETE SET NULL,
    author_id  TEXT REFERENCES actors (id),
    body       TEXT NOT NULL,
    version    BIGINT NOT NULL DEFAULT 1,
    linear_id  TEXT UNIQUE,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    deleted_at TEXT
);
CREATE INDEX comments_issue ON comments (issue_id);

CREATE TABLE documents (
    id         TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    slug       TEXT NOT NULL COLLATE nocase,
    title      TEXT NOT NULL,
    body       TEXT NOT NULL DEFAULT '',
    version    BIGINT NOT NULL DEFAULT 1,
    linear_id  TEXT UNIQUE,
    created_by TEXT REFERENCES actors (id),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    deleted_at TEXT,
    UNIQUE (project_id, slug)
);

CREATE TABLE revisions (
    id           TEXT PRIMARY KEY,
    entity_type  TEXT NOT NULL CHECK (entity_type IN ('issue', 'document', 'comment')),
    entity_id    TEXT NOT NULL,
    version      BIGINT NOT NULL,
    body         TEXT NOT NULL,
    actor_id     TEXT REFERENCES actors (id),
    session_id   TEXT,
    created_at   TEXT NOT NULL,
    base_version BIGINT,
    UNIQUE (entity_type, entity_id, version)
);

-- SQLite's AUTOINCREMENT: seqs only grow, and the write lock keeps them in
-- commit order.
CREATE TABLE activity (
    seq         BIGINT GENERATED BY DEFAULT AS IDENTITY PRIMARY KEY,
    id          TEXT NOT NULL UNIQUE,
    entity_type TEXT NOT NULL,
    entity_id   TEXT NOT NULL,
    issue_id    TEXT,
    field       TEXT NOT NULL,
    old         TEXT,
    new         TEXT,
    actor_id    TEXT REFERENCES actors (id),
    session_id  TEXT,
    created_at  TEXT NOT NULL
);
CREATE INDEX activity_issue ON activity (issue_id, seq);
CREATE INDEX activity_entity ON activity (entity_id, seq);

CREATE TABLE claims (
    id          TEXT PRIMARY KEY,
    issue_id    TEXT NOT NULL REFERENCES issues (id) ON DELETE CASCADE,
    actor_id    TEXT NOT NULL REFERENCES actors (id),
    session_id  TEXT NOT NULL,
    host        TEXT,
    worktree    TEXT,
    branch      TEXT,
    herdr_pane  TEXT,
    stolen_from TEXT REFERENCES claims (id),
    created_at  TEXT NOT NULL,
    ended_at    TEXT,
    end_reason  TEXT
);
CREATE UNIQUE INDEX claims_live ON claims (issue_id) WHERE ended_at IS NULL;
CREATE INDEX claims_branch ON claims (branch);

CREATE TABLE work_sessions (
    id              TEXT PRIMARY KEY,
    session_id      TEXT NOT NULL,
    issue_id        TEXT NOT NULL REFERENCES issues (id) ON DELETE CASCADE,
    actor_id        TEXT NOT NULL REFERENCES actors (id),
    harness_version TEXT,
    transcript_path TEXT,
    herdr_pane      TEXT,
    host            TEXT,
    worktree        TEXT,
    branch          TEXT,
    first_head      TEXT,
    last_head       TEXT,
    started_at      TEXT NOT NULL,
    last_seen_at    TEXT NOT NULL,
    active_seconds  BIGINT NOT NULL DEFAULT 0,
    UNIQUE (session_id, issue_id)
);
CREATE INDEX work_sessions_issue ON work_sessions (issue_id);

CREATE TABLE session_notes (
    id         TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    issue_id   TEXT REFERENCES issues (id) ON DELETE CASCADE,
    body       TEXT NOT NULL,
    actor_id   TEXT REFERENCES actors (id),
    created_at TEXT NOT NULL
);
CREATE INDEX session_notes_session ON session_notes (session_id);

CREATE TABLE saved_views (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL COLLATE nocase UNIQUE,
    query      TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- `content` holds the bytes on Postgres, so every laptop serves the same
-- files (LLM-458). SQLite keeps them in the data folder's `files/` and
-- leaves the column empty.
CREATE TABLE files (
    hash         TEXT PRIMARY KEY,
    mime         TEXT,
    size         BIGINT NOT NULL,
    original_url TEXT UNIQUE,
    created_at   TEXT NOT NULL,
    content      BYTEA
);

CREATE TABLE file_sources (
    url  TEXT PRIMARY KEY,
    hash TEXT NOT NULL REFERENCES files (hash) ON DELETE CASCADE
);

CREATE TABLE work_session_usage (
    work_session_id TEXT NOT NULL REFERENCES work_sessions (id) ON DELETE CASCADE,
    model           TEXT NOT NULL,
    requests        BIGINT NOT NULL DEFAULT 0,
    input           BIGINT NOT NULL DEFAULT 0,
    output          BIGINT NOT NULL DEFAULT 0,
    cache_write_5m  BIGINT NOT NULL DEFAULT 0,
    cache_write_1h  BIGINT NOT NULL DEFAULT 0,
    cache_read      BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (work_session_id, model)
);

CREATE TABLE transcript_usage_seen (
    transcript     TEXT NOT NULL,
    model          TEXT NOT NULL,
    requests       BIGINT NOT NULL DEFAULT 0,
    input          BIGINT NOT NULL DEFAULT 0,
    output         BIGINT NOT NULL DEFAULT 0,
    cache_write_5m BIGINT NOT NULL DEFAULT 0,
    cache_write_1h BIGINT NOT NULL DEFAULT 0,
    cache_read     BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (transcript, model)
);

CREATE TABLE transcript_time_seen (
    transcript     TEXT PRIMARY KEY,
    active_seconds BIGINT NOT NULL
);

CREATE TABLE analyzing (
    issue_id   TEXT PRIMARY KEY REFERENCES issues (id) ON DELETE CASCADE,
    actor_id   TEXT NOT NULL REFERENCES actors (id),
    session_id TEXT NOT NULL,
    started_at TEXT NOT NULL,
    seen_at    TEXT NOT NULL
);

CREATE TABLE questions (
    issue_id          TEXT NOT NULL REFERENCES issues (id) ON DELETE CASCADE,
    position          BIGINT NOT NULL,
    number            BIGINT NOT NULL,
    text              TEXT NOT NULL,
    context           TEXT NOT NULL,
    options           TEXT NOT NULL,
    answer            TEXT,
    answered_version  BIGINT,
    answered_by       TEXT REFERENCES actors (id),
    answered_question TEXT,
    PRIMARY KEY (issue_id, position)
);
CREATE INDEX questions_open ON questions (issue_id) WHERE answer IS NULL;

CREATE TABLE reviews (
    id         TEXT PRIMARY KEY,
    issue_id   TEXT NOT NULL REFERENCES issues (id) ON DELETE CASCADE,
    round      BIGINT NOT NULL,
    verdict    TEXT NOT NULL CHECK (verdict IN ('approved', 'changes_requested')),
    feedback   TEXT NOT NULL DEFAULT '',
    author_id  TEXT REFERENCES actors (id),
    created_at TEXT NOT NULL,
    UNIQUE (issue_id, round)
);

-- The dependency graph cache key (LLM-216): any change to a blocks edge, an
-- issue's status or trash state, or a status type gives it a new value.
CREATE TABLE dep_graph_token (token BYTEA NOT NULL);
INSERT INTO dep_graph_token (token) VALUES (decode(md5(random()::text || clock_timestamp()::text), 'hex'));

CREATE FUNCTION dep_graph_bump() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    UPDATE dep_graph_token SET token = decode(md5(random()::text || clock_timestamp()::text), 'hex');
    RETURN NULL;
END $$;

CREATE TRIGGER dep_graph_relation_insert AFTER INSERT ON relations
    FOR EACH ROW WHEN (NEW.type = 'blocks') EXECUTE FUNCTION dep_graph_bump();
CREATE TRIGGER dep_graph_relation_delete AFTER DELETE ON relations
    FOR EACH ROW WHEN (OLD.type = 'blocks') EXECUTE FUNCTION dep_graph_bump();
CREATE TRIGGER dep_graph_relation_update AFTER UPDATE ON relations
    FOR EACH ROW WHEN (OLD.type = 'blocks' OR NEW.type = 'blocks') EXECUTE FUNCTION dep_graph_bump();
CREATE TRIGGER dep_graph_issue_update AFTER UPDATE OF status_id, deleted_at ON issues
    FOR EACH ROW WHEN (OLD.status_id IS DISTINCT FROM NEW.status_id OR OLD.deleted_at IS DISTINCT FROM NEW.deleted_at)
    EXECUTE FUNCTION dep_graph_bump();
CREATE TRIGGER dep_graph_issue_delete AFTER DELETE ON issues
    FOR EACH ROW EXECUTE FUNCTION dep_graph_bump();
CREATE TRIGGER dep_graph_status_update AFTER UPDATE OF type ON statuses
    FOR EACH ROW WHEN (OLD.type IS DISTINCT FROM NEW.type) EXECUTE FUNCTION dep_graph_bump();

-- Search (LLM-455). search_rows gives each searchable record a stable
-- number, as on SQLite. search_fts holds the text with a tsvector for full
-- text, and issues_trgm the text for duplicate search with pg_trgm.
CREATE TABLE search_rows (
    id          BIGINT GENERATED BY DEFAULT AS IDENTITY PRIMARY KEY,
    entity_type TEXT NOT NULL,
    entity_id   TEXT NOT NULL,
    issue_id    TEXT,
    UNIQUE (entity_type, entity_id)
);
CREATE INDEX search_rows_issue ON search_rows (issue_id) WHERE issue_id IS NOT NULL;

CREATE TABLE search_fts (
    rowid       BIGINT PRIMARY KEY,
    entity_type TEXT NOT NULL,
    entity_id   TEXT NOT NULL,
    issue_id    TEXT,
    title       TEXT NOT NULL,
    body        TEXT NOT NULL,
    tsv         tsvector GENERATED ALWAYS AS (
        setweight(to_tsvector('simple', title), 'A') || setweight(to_tsvector('simple', body), 'B')
    ) STORED
);
CREATE INDEX search_fts_tsv ON search_fts USING gin (tsv);
CREATE INDEX search_fts_issue ON search_fts (issue_id);

CREATE TABLE issues_trgm (
    rowid       BIGINT PRIMARY KEY,
    issue_id    TEXT NOT NULL,
    title       TEXT NOT NULL,
    description TEXT NOT NULL,
    doc         TEXT GENERATED ALWAYS AS (title || ' ' || description) STORED
);
CREATE INDEX issues_trgm_doc ON issues_trgm USING gin (doc gin_trgm_ops);

CREATE FUNCTION search_row(t TEXT, eid TEXT, iid TEXT) RETURNS BIGINT LANGUAGE plpgsql AS $$
DECLARE r BIGINT;
BEGIN
    INSERT INTO search_rows (entity_type, entity_id, issue_id) VALUES (t, eid, iid)
        ON CONFLICT (entity_type, entity_id) DO NOTHING;
    SELECT id INTO r FROM search_rows WHERE entity_type = t AND entity_id = eid;
    RETURN r;
END $$;

CREATE FUNCTION issues_search() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE r BIGINT;
BEGIN
    IF TG_OP = 'DELETE' THEN
        DELETE FROM search_fts WHERE rowid IN (SELECT id FROM search_rows WHERE issue_id = OLD.id);
        DELETE FROM issues_trgm WHERE issue_id = OLD.id;
        DELETE FROM search_rows WHERE issue_id = OLD.id;
        RETURN NULL;
    END IF;
    r := search_row('issue', NEW.id, NEW.id);
    INSERT INTO search_fts (rowid, entity_type, entity_id, issue_id, title, body)
        VALUES (r, 'issue', NEW.id, NEW.id, NEW.key || ' ' || NEW.title, NEW.description)
        ON CONFLICT (rowid) DO UPDATE SET title = excluded.title, body = excluded.body;
    INSERT INTO issues_trgm (rowid, issue_id, title, description)
        VALUES (r, NEW.id, NEW.title, NEW.description)
        ON CONFLICT (rowid) DO UPDATE SET title = excluded.title, description = excluded.description;
    RETURN NULL;
END $$;

CREATE TRIGGER issues_ai AFTER INSERT ON issues FOR EACH ROW EXECUTE FUNCTION issues_search();
CREATE TRIGGER issues_au AFTER UPDATE OF title, description, key ON issues FOR EACH ROW EXECUTE FUNCTION issues_search();
CREATE TRIGGER issues_ad AFTER DELETE ON issues FOR EACH ROW EXECUTE FUNCTION issues_search();

CREATE FUNCTION comments_search() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE r BIGINT;
BEGIN
    IF TG_OP = 'DELETE' THEN
        DELETE FROM search_fts WHERE rowid = (SELECT id FROM search_rows WHERE entity_type = 'comment' AND entity_id = OLD.id);
        DELETE FROM search_rows WHERE entity_type = 'comment' AND entity_id = OLD.id;
        RETURN NULL;
    END IF;
    r := search_row('comment', NEW.id, NEW.issue_id);
    DELETE FROM search_fts WHERE rowid = r;
    IF NEW.deleted_at IS NULL THEN
        INSERT INTO search_fts (rowid, entity_type, entity_id, issue_id, title, body)
            VALUES (r, 'comment', NEW.id, NEW.issue_id, '', NEW.body);
    END IF;
    RETURN NULL;
END $$;

CREATE TRIGGER comments_ai AFTER INSERT ON comments FOR EACH ROW EXECUTE FUNCTION comments_search();
CREATE TRIGGER comments_au AFTER UPDATE OF body, deleted_at ON comments FOR EACH ROW EXECUTE FUNCTION comments_search();
CREATE TRIGGER comments_ad AFTER DELETE ON comments FOR EACH ROW EXECUTE FUNCTION comments_search();

CREATE FUNCTION documents_search() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE r BIGINT;
BEGIN
    IF TG_OP = 'DELETE' THEN
        DELETE FROM search_fts WHERE rowid = (SELECT id FROM search_rows WHERE entity_type = 'document' AND entity_id = OLD.id);
        DELETE FROM search_rows WHERE entity_type = 'document' AND entity_id = OLD.id;
        RETURN NULL;
    END IF;
    r := search_row('document', NEW.id, NULL);
    INSERT INTO search_fts (rowid, entity_type, entity_id, issue_id, title, body)
        VALUES (r, 'document', NEW.id, NULL, NEW.title, NEW.body)
        ON CONFLICT (rowid) DO UPDATE SET title = excluded.title, body = excluded.body;
    RETURN NULL;
END $$;

CREATE TRIGGER documents_ai AFTER INSERT ON documents FOR EACH ROW EXECUTE FUNCTION documents_search();
CREATE TRIGGER documents_au AFTER UPDATE OF title, body ON documents FOR EACH ROW EXECUTE FUNCTION documents_search();
CREATE TRIGGER documents_ad AFTER DELETE ON documents FOR EACH ROW EXECUTE FUNCTION documents_search();
