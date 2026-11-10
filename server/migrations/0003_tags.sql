-- Tags: short labels a sketch's owner gives it (at most 8, distinct, in
-- the order the owner gave them), for browsing explore by topic. Like the
-- title and description, they belong to the sketch, not its revisions.

CREATE TABLE sketch_tags (
  sketch_id TEXT NOT NULL REFERENCES sketches (id) ON DELETE CASCADE,
  tag TEXT NOT NULL,
  position INTEGER NOT NULL,
  PRIMARY KEY (sketch_id, tag)
);

CREATE INDEX sketch_tags_by_tag ON sketch_tags (tag, sketch_id);
