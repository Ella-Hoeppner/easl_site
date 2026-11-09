-- Revisions: every saved version of a sketch's program (its files and
-- entry point), numbered from 1 per sketch and kept forever.
-- `sketches.head_revision` is the latest saved one (the owner's draft);
-- `sketches.published_revision` is the one everyone else sees, set while
-- the sketch is unlisted or public. `published_updated_at` is when the
-- published revision last changed.

CREATE TABLE sketch_revisions (
  sketch_id TEXT NOT NULL REFERENCES sketches (id) ON DELETE CASCADE,
  number INTEGER NOT NULL,
  entry_file TEXT,
  entry_name TEXT,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (sketch_id, number)
);

CREATE TABLE revision_files (
  sketch_id TEXT NOT NULL,
  number INTEGER NOT NULL,
  path TEXT NOT NULL,
  content TEXT NOT NULL,
  position INTEGER NOT NULL,
  PRIMARY KEY (sketch_id, number, path),
  FOREIGN KEY (sketch_id, number)
    REFERENCES sketch_revisions (sketch_id, number) ON DELETE CASCADE
);

ALTER TABLE sketches ADD COLUMN head_revision INTEGER NOT NULL DEFAULT 1;
ALTER TABLE sketches ADD COLUMN published_revision INTEGER;
ALTER TABLE sketches ADD COLUMN published_updated_at INTEGER;

-- Each existing sketch's program becomes its revision 1, published unless
-- the sketch is private.

INSERT INTO sketch_revisions (sketch_id, number, entry_file, entry_name,
  created_at)
SELECT id, 1, entry_file, entry_name, updated_at FROM sketches;

INSERT INTO revision_files (sketch_id, number, path, content, position)
SELECT sketch_id, 1, path, content, position FROM sketch_files;

UPDATE sketches SET published_revision = 1, published_updated_at = updated_at
WHERE visibility != 'private';

DROP TABLE sketch_files;
ALTER TABLE sketches DROP COLUMN entry_file;
ALTER TABLE sketches DROP COLUMN entry_name;

CREATE INDEX sketches_by_owner_published
  ON sketches (owner_id, published_updated_at);
