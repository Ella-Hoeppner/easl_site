-- What the docs said about each example sketch when it was last synced
-- (`src/examples.rs`): its title, description, and files (a JSON array of
-- `{path, content}`). A sync changes a sketch's title, description, or
-- files only where the docs' version changed since, so edits made on the
-- site survive until the docs change the same thing.

CREATE TABLE example_syncs (
  sketch_id TEXT PRIMARY KEY REFERENCES sketches (id) ON DELETE CASCADE,
  title TEXT NOT NULL,
  description TEXT NOT NULL,
  files TEXT NOT NULL
);
