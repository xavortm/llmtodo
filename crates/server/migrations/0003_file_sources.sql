-- Every URL a stored file came from. Two Linear URLs can hold the same
-- bytes, so one file can have several sources.
CREATE TABLE file_sources (
    url  TEXT PRIMARY KEY,
    hash TEXT NOT NULL REFERENCES files (hash) ON DELETE CASCADE
) STRICT;
INSERT INTO file_sources (url, hash) SELECT original_url, hash FROM files WHERE original_url IS NOT NULL;
