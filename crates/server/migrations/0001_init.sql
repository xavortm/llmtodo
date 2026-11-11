-- llmtodo schema, version 1.
-- IDs are UUIDv7 text. Times are UTC text in a fixed-width RFC 3339 form,
-- so text order is time order.

CREATE TABLE settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
) STRICT;

CREATE TABLE actors (
    id         TEXT PRIMARY KEY,
    kind       TEXT NOT NULL CHECK (kind IN ('human', 'agent', 'system')),
    name       TEXT NOT NULL,
    harness    TEXT,
    model      TEXT,
    effort     TEXT,
    is_owner   INTEGER NOT NULL DEFAULT 0,
    linear_id  TEXT UNIQUE,
    created_at TEXT NOT NULL
) STRICT;
-- One agent actor per mix of harness, model and effort.
CREATE UNIQUE INDEX actors_agent_identity ON actors (coalesce(harness, ''), coalesce(model, ''), coalesce(effort, '')) WHERE kind = 'agent';
CREATE UNIQUE INDEX actors_named ON actors (kind, name) WHERE kind <> 'agent';
CREATE UNIQUE INDEX actors_one_owner ON actors (is_owner) WHERE is_owner = 1;

CREATE TABLE teams (
    id                  TEXT PRIMARY KEY,
    key                 TEXT NOT NULL UNIQUE,
    name                TEXT NOT NULL,
    next_number         INTEGER NOT NULL DEFAULT 1,
    pr_open_status_id   TEXT REFERENCES statuses (id) ON DELETE SET NULL,
    pr_merged_status_id TEXT REFERENCES statuses (id) ON DELETE SET NULL,
    linear_id           TEXT UNIQUE,
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL
) STRICT;

CREATE TABLE statuses (
    id         TEXT PRIMARY KEY,
    team_id    TEXT NOT NULL REFERENCES teams (id) ON DELETE CASCADE,
    name       TEXT NOT NULL,
    type       TEXT NOT NULL CHECK (type IN ('backlog', 'unstarted', 'started', 'completed', 'canceled')),
    color      TEXT NOT NULL,
    position   INTEGER NOT NULL,
    linear_id  TEXT UNIQUE,
    UNIQUE (team_id, name COLLATE NOCASE)
) STRICT;

CREATE TABLE labels (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    group_id    TEXT REFERENCES labels (id) ON DELETE CASCADE,
    is_group    INTEGER NOT NULL DEFAULT 0,
    color       TEXT NOT NULL,
    description TEXT,
    linear_id   TEXT UNIQUE,
    created_at  TEXT NOT NULL
) STRICT;
CREATE UNIQUE INDEX labels_name ON labels (coalesce(group_id, ''), name COLLATE NOCASE);

CREATE TABLE projects (
    id          TEXT PRIMARY KEY,
    slug        TEXT NOT NULL UNIQUE COLLATE NOCASE,
    name        TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    state       TEXT NOT NULL,
    linear_id   TEXT UNIQUE,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    deleted_at  TEXT
) STRICT;

CREATE TABLE project_teams (
    project_id TEXT NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    team_id    TEXT NOT NULL REFERENCES teams (id) ON DELETE CASCADE,
    PRIMARY KEY (project_id, team_id)
) STRICT;

CREATE TABLE project_repos (
    project_id     TEXT NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    repo           TEXT NOT NULL COLLATE NOCASE,
    etag           TEXT,
    last_polled_at TEXT,
    PRIMARY KEY (project_id, repo)
) STRICT;

CREATE TABLE milestones (
    id          TEXT PRIMARY KEY,
    project_id  TEXT NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    target_date TEXT,
    position    INTEGER NOT NULL DEFAULT 0,
    linear_id   TEXT UNIQUE,
    created_at  TEXT NOT NULL,
    UNIQUE (project_id, name COLLATE NOCASE)
) STRICT;

CREATE TABLE issues (
    id                  TEXT PRIMARY KEY,
    team_id             TEXT NOT NULL REFERENCES teams (id),
    number              INTEGER NOT NULL,
    key                 TEXT NOT NULL UNIQUE,
    title               TEXT NOT NULL,
    description         TEXT NOT NULL DEFAULT '',
    description_version INTEGER NOT NULL DEFAULT 1,
    status_id           TEXT NOT NULL REFERENCES statuses (id),
    priority            INTEGER NOT NULL DEFAULT 0,
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
    UNIQUE (team_id, number)
) STRICT;
CREATE INDEX issues_parent ON issues (parent_id);
CREATE INDEX issues_project ON issues (project_id);
CREATE INDEX issues_status ON issues (status_id);
CREATE INDEX issues_assignee ON issues (assignee_id);

-- Every key an issue ever had. Old keys stay after a team move, so old
-- links still resolve. Keys are never reused.
CREATE TABLE issue_keys (
    key      TEXT PRIMARY KEY,
    issue_id TEXT NOT NULL REFERENCES issues (id) ON DELETE CASCADE
) STRICT;

CREATE TABLE issue_labels (
    issue_id TEXT NOT NULL REFERENCES issues (id) ON DELETE CASCADE,
    label_id TEXT NOT NULL REFERENCES labels (id) ON DELETE CASCADE,
    PRIMARY KEY (issue_id, label_id)
) STRICT;

CREATE TABLE relations (
    id         TEXT PRIMARY KEY,
    type       TEXT NOT NULL CHECK (type IN ('blocks', 'related', 'duplicate')),
    from_id    TEXT NOT NULL REFERENCES issues (id) ON DELETE CASCADE,
    to_id      TEXT NOT NULL REFERENCES issues (id) ON DELETE CASCADE,
    linear_id  TEXT UNIQUE,
    created_by TEXT REFERENCES actors (id),
    created_at TEXT NOT NULL,
    UNIQUE (type, from_id, to_id),
    CHECK (from_id <> to_id)
) STRICT;
CREATE INDEX relations_to ON relations (to_id);

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
) STRICT;

CREATE TABLE comments (
    id         TEXT PRIMARY KEY,
    issue_id   TEXT NOT NULL REFERENCES issues (id) ON DELETE CASCADE,
    parent_id  TEXT REFERENCES comments (id) ON DELETE SET NULL,
    author_id  TEXT REFERENCES actors (id),
    body       TEXT NOT NULL,
    version    INTEGER NOT NULL DEFAULT 1,
    linear_id  TEXT UNIQUE,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    deleted_at TEXT
) STRICT;
CREATE INDEX comments_issue ON comments (issue_id);

CREATE TABLE documents (
    id         TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    slug       TEXT NOT NULL COLLATE NOCASE,
    title      TEXT NOT NULL,
    body       TEXT NOT NULL DEFAULT '',
    version    INTEGER NOT NULL DEFAULT 1,
    linear_id  TEXT UNIQUE,
    created_by TEXT REFERENCES actors (id),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    deleted_at TEXT,
    UNIQUE (project_id, slug)
) STRICT;

-- A full copy of a description, document or comment body at one version.
CREATE TABLE revisions (
    id          TEXT PRIMARY KEY,
    entity_type TEXT NOT NULL CHECK (entity_type IN ('issue', 'document', 'comment')),
    entity_id   TEXT NOT NULL,
    version     INTEGER NOT NULL,
    body        TEXT NOT NULL,
    actor_id    TEXT REFERENCES actors (id),
    session_id  TEXT,
    created_at  TEXT NOT NULL,
    UNIQUE (entity_type, entity_id, version)
) STRICT;

CREATE TABLE activity (
    seq         INTEGER PRIMARY KEY AUTOINCREMENT,
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
) STRICT;
CREATE INDEX activity_issue ON activity (issue_id, seq);

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
) STRICT;
-- At most one live claim per issue.
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
    UNIQUE (session_id, issue_id)
) STRICT;

CREATE TABLE session_notes (
    id         TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    issue_id   TEXT REFERENCES issues (id) ON DELETE CASCADE,
    body       TEXT NOT NULL,
    actor_id   TEXT REFERENCES actors (id),
    created_at TEXT NOT NULL
) STRICT;
CREATE INDEX session_notes_session ON session_notes (session_id);

CREATE TABLE saved_views (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL UNIQUE COLLATE NOCASE,
    query      TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
) STRICT;

-- Files copied from Linear, stored in files/ by content hash.
CREATE TABLE files (
    hash         TEXT PRIMARY KEY,
    mime         TEXT,
    size         INTEGER NOT NULL,
    original_url TEXT UNIQUE,
    created_at   TEXT NOT NULL
) STRICT;

-- Full-text search over issues, comments and documents.
CREATE VIRTUAL TABLE search_fts USING fts5 (
    entity_type UNINDEXED,
    entity_id UNINDEXED,
    issue_id UNINDEXED,
    title,
    body,
    tokenize = 'unicode61 remove_diacritics 2'
);

-- Duplicate search over issue titles and descriptions.
CREATE VIRTUAL TABLE issues_trgm USING fts5 (
    issue_id UNINDEXED,
    title,
    description,
    tokenize = 'trigram'
);

CREATE TRIGGER issues_ai AFTER INSERT ON issues BEGIN
    INSERT INTO search_fts (entity_type, entity_id, issue_id, title, body) VALUES ('issue', new.id, new.id, new.key || ' ' || new.title, new.description);
    INSERT INTO issues_trgm (issue_id, title, description) VALUES (new.id, new.title, new.description);
END;
CREATE TRIGGER issues_au AFTER UPDATE OF title, description, key ON issues BEGIN
    DELETE FROM search_fts WHERE entity_type = 'issue' AND entity_id = old.id;
    INSERT INTO search_fts (entity_type, entity_id, issue_id, title, body) VALUES ('issue', new.id, new.id, new.key || ' ' || new.title, new.description);
    DELETE FROM issues_trgm WHERE issue_id = old.id;
    INSERT INTO issues_trgm (issue_id, title, description) VALUES (new.id, new.title, new.description);
END;
CREATE TRIGGER issues_ad AFTER DELETE ON issues BEGIN
    DELETE FROM search_fts WHERE issue_id = old.id;
    DELETE FROM issues_trgm WHERE issue_id = old.id;
END;

CREATE TRIGGER comments_ai AFTER INSERT ON comments BEGIN
    INSERT INTO search_fts (entity_type, entity_id, issue_id, title, body) VALUES ('comment', new.id, new.issue_id, '', new.body);
END;
CREATE TRIGGER comments_au AFTER UPDATE OF body, deleted_at ON comments BEGIN
    DELETE FROM search_fts WHERE entity_type = 'comment' AND entity_id = old.id;
    INSERT INTO search_fts (entity_type, entity_id, issue_id, title, body)
        SELECT 'comment', new.id, new.issue_id, '', new.body WHERE new.deleted_at IS NULL;
END;
CREATE TRIGGER comments_ad AFTER DELETE ON comments BEGIN
    DELETE FROM search_fts WHERE entity_type = 'comment' AND entity_id = old.id;
END;

CREATE TRIGGER documents_ai AFTER INSERT ON documents BEGIN
    INSERT INTO search_fts (entity_type, entity_id, issue_id, title, body) VALUES ('document', new.id, NULL, new.title, new.body);
END;
CREATE TRIGGER documents_au AFTER UPDATE OF title, body ON documents BEGIN
    DELETE FROM search_fts WHERE entity_type = 'document' AND entity_id = old.id;
    INSERT INTO search_fts (entity_type, entity_id, issue_id, title, body) VALUES ('document', new.id, NULL, new.title, new.body);
END;
CREATE TRIGGER documents_ad AFTER DELETE ON documents BEGIN
    DELETE FROM search_fts WHERE entity_type = 'document' AND entity_id = old.id;
END;
