-- LLM-458: a file's bytes can live in the database. A shared Postgres
-- database keeps them there, so every laptop serves the same files, and a
-- backup or a move to SQLite carries them along. SQLite keeps new files in
-- the data folder's `files/` and leaves this column empty.
ALTER TABLE files ADD COLUMN content BLOB;
