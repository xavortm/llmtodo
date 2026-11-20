-- LLM-4: the search triggers find their FTS rows by rowid.
--
-- The triggers from 0001 deleted with `WHERE entity_type = ? AND entity_id = ?`
-- and `WHERE issue_id = ?`. Those are UNINDEXED FTS5 columns, so every
-- title, description, comment or document write scanned the whole FTS table
-- under the write lock.
--
-- `search_rows` gives each indexed entity a stable integer key. That key is
-- the entity's rowid in `search_fts`, and for issues also in `issues_trgm`.
-- It is an INTEGER PRIMARY KEY, so VACUUM and backups keep it. The FTS
-- tables are rebuilt from the base tables with the same rows the old
-- triggers wrote, so search results stay the same.

DROP TRIGGER issues_ai;
DROP TRIGGER issues_au;
DROP TRIGGER issues_ad;
DROP TRIGGER comments_ai;
DROP TRIGGER comments_au;
DROP TRIGGER comments_ad;
DROP TRIGGER documents_ai;
DROP TRIGGER documents_au;
DROP TRIGGER documents_ad;

DROP TABLE search_fts;
DROP TABLE issues_trgm;

CREATE TABLE search_rows (
    id          INTEGER PRIMARY KEY,
    entity_type TEXT NOT NULL,
    entity_id   TEXT NOT NULL,
    issue_id    TEXT,
    UNIQUE (entity_type, entity_id)
) STRICT;
CREATE INDEX search_rows_issue ON search_rows (issue_id) WHERE issue_id IS NOT NULL;

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

INSERT INTO search_rows (entity_type, entity_id, issue_id) SELECT 'issue', id, id FROM issues;
INSERT INTO search_rows (entity_type, entity_id, issue_id) SELECT 'comment', id, issue_id FROM comments;
INSERT INTO search_rows (entity_type, entity_id, issue_id) SELECT 'document', id, NULL FROM documents;

INSERT INTO search_fts (rowid, entity_type, entity_id, issue_id, title, body)
    SELECT r.id, 'issue', i.id, i.id, i.key || ' ' || i.title, i.description
    FROM issues i JOIN search_rows r ON r.entity_type = 'issue' AND r.entity_id = i.id;
INSERT INTO issues_trgm (rowid, issue_id, title, description)
    SELECT r.id, i.id, i.title, i.description
    FROM issues i JOIN search_rows r ON r.entity_type = 'issue' AND r.entity_id = i.id;
INSERT INTO search_fts (rowid, entity_type, entity_id, issue_id, title, body)
    SELECT r.id, 'comment', c.id, c.issue_id, '', c.body
    FROM comments c JOIN search_rows r ON r.entity_type = 'comment' AND r.entity_id = c.id
    WHERE c.deleted_at IS NULL;
INSERT INTO search_fts (rowid, entity_type, entity_id, issue_id, title, body)
    SELECT r.id, 'document', d.id, NULL, d.title, d.body
    FROM documents d JOIN search_rows r ON r.entity_type = 'document' AND r.entity_id = d.id;

-- Each trigger looks its key up through the UNIQUE index, then touches the
-- FTS tables by rowid only. The update triggers add a missing key first, so
-- they still work on a row the map lost.

CREATE TRIGGER issues_ai AFTER INSERT ON issues BEGIN
    INSERT INTO search_rows (entity_type, entity_id, issue_id) VALUES ('issue', new.id, new.id);
    INSERT INTO search_fts (rowid, entity_type, entity_id, issue_id, title, body) VALUES (
        (SELECT id FROM search_rows WHERE entity_type = 'issue' AND entity_id = new.id),
        'issue', new.id, new.id, new.key || ' ' || new.title, new.description);
    INSERT INTO issues_trgm (rowid, issue_id, title, description) VALUES (
        (SELECT id FROM search_rows WHERE entity_type = 'issue' AND entity_id = new.id),
        new.id, new.title, new.description);
END;
CREATE TRIGGER issues_au AFTER UPDATE OF title, description, key ON issues BEGIN
    INSERT OR IGNORE INTO search_rows (entity_type, entity_id, issue_id) VALUES ('issue', new.id, new.id);
    DELETE FROM search_fts WHERE rowid = (SELECT id FROM search_rows WHERE entity_type = 'issue' AND entity_id = old.id);
    INSERT INTO search_fts (rowid, entity_type, entity_id, issue_id, title, body) VALUES (
        (SELECT id FROM search_rows WHERE entity_type = 'issue' AND entity_id = new.id),
        'issue', new.id, new.id, new.key || ' ' || new.title, new.description);
    DELETE FROM issues_trgm WHERE rowid = (SELECT id FROM search_rows WHERE entity_type = 'issue' AND entity_id = old.id);
    INSERT INTO issues_trgm (rowid, issue_id, title, description) VALUES (
        (SELECT id FROM search_rows WHERE entity_type = 'issue' AND entity_id = new.id),
        new.id, new.title, new.description);
END;
-- The issue's comments go with it, by the foreign key cascade and by
-- `issue_id` here, as the old trigger removed every row of the issue.
CREATE TRIGGER issues_ad AFTER DELETE ON issues BEGIN
    DELETE FROM search_fts WHERE rowid IN (SELECT id FROM search_rows WHERE issue_id = old.id);
    DELETE FROM issues_trgm WHERE rowid = (SELECT id FROM search_rows WHERE entity_type = 'issue' AND entity_id = old.id);
    DELETE FROM search_rows WHERE issue_id = old.id;
END;

CREATE TRIGGER comments_ai AFTER INSERT ON comments BEGIN
    INSERT INTO search_rows (entity_type, entity_id, issue_id) VALUES ('comment', new.id, new.issue_id);
    INSERT INTO search_fts (rowid, entity_type, entity_id, issue_id, title, body) VALUES (
        (SELECT id FROM search_rows WHERE entity_type = 'comment' AND entity_id = new.id),
        'comment', new.id, new.issue_id, '', new.body);
END;
CREATE TRIGGER comments_au AFTER UPDATE OF body, deleted_at ON comments BEGIN
    INSERT OR IGNORE INTO search_rows (entity_type, entity_id, issue_id) VALUES ('comment', new.id, new.issue_id);
    DELETE FROM search_fts WHERE rowid = (SELECT id FROM search_rows WHERE entity_type = 'comment' AND entity_id = old.id);
    INSERT INTO search_fts (rowid, entity_type, entity_id, issue_id, title, body)
        SELECT r.id, 'comment', new.id, new.issue_id, '', new.body
        FROM search_rows r WHERE r.entity_type = 'comment' AND r.entity_id = new.id AND new.deleted_at IS NULL;
END;
CREATE TRIGGER comments_ad AFTER DELETE ON comments BEGIN
    DELETE FROM search_fts WHERE rowid = (SELECT id FROM search_rows WHERE entity_type = 'comment' AND entity_id = old.id);
    DELETE FROM search_rows WHERE entity_type = 'comment' AND entity_id = old.id;
END;

CREATE TRIGGER documents_ai AFTER INSERT ON documents BEGIN
    INSERT INTO search_rows (entity_type, entity_id, issue_id) VALUES ('document', new.id, NULL);
    INSERT INTO search_fts (rowid, entity_type, entity_id, issue_id, title, body) VALUES (
        (SELECT id FROM search_rows WHERE entity_type = 'document' AND entity_id = new.id),
        'document', new.id, NULL, new.title, new.body);
END;
CREATE TRIGGER documents_au AFTER UPDATE OF title, body ON documents BEGIN
    INSERT OR IGNORE INTO search_rows (entity_type, entity_id, issue_id) VALUES ('document', new.id, NULL);
    DELETE FROM search_fts WHERE rowid = (SELECT id FROM search_rows WHERE entity_type = 'document' AND entity_id = old.id);
    INSERT INTO search_fts (rowid, entity_type, entity_id, issue_id, title, body) VALUES (
        (SELECT id FROM search_rows WHERE entity_type = 'document' AND entity_id = new.id),
        'document', new.id, NULL, new.title, new.body);
END;
CREATE TRIGGER documents_ad AFTER DELETE ON documents BEGIN
    DELETE FROM search_fts WHERE rowid = (SELECT id FROM search_rows WHERE entity_type = 'document' AND entity_id = old.id);
    DELETE FROM search_rows WHERE entity_type = 'document' AND entity_id = old.id;
END;
