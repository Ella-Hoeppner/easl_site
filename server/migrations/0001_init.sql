-- The initial schema. Timestamps are unix seconds.

CREATE TABLE users (
  id INTEGER PRIMARY KEY,
  username TEXT NOT NULL UNIQUE COLLATE NOCASE,
  email TEXT NOT NULL UNIQUE COLLATE NOCASE,
  password_hash TEXT NOT NULL,
  display_name TEXT NOT NULL DEFAULT '',
  bio TEXT NOT NULL DEFAULT '',
  created_at INTEGER NOT NULL,
  email_verified_at INTEGER,
  is_admin INTEGER NOT NULL DEFAULT 0,
  banned_at INTEGER
);

CREATE TABLE sessions (
  token_hash TEXT PRIMARY KEY,
  user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
  created_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL,
  last_seen_at INTEGER NOT NULL
);

CREATE INDEX sessions_by_user ON sessions (user_id);
CREATE INDEX sessions_by_expiry ON sessions (expires_at);

CREATE TABLE email_tokens (
  token_hash TEXT PRIMARY KEY,
  user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
  purpose TEXT NOT NULL CHECK (purpose IN ('verify', 'reset')),
  expires_at INTEGER NOT NULL,
  used_at INTEGER
);

CREATE INDEX email_tokens_by_user ON email_tokens (user_id);

CREATE TABLE sketches (
  id TEXT PRIMARY KEY,
  owner_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
  title TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  visibility TEXT NOT NULL
    CHECK (visibility IN ('private', 'unlisted', 'public')),
  entry_file TEXT,
  entry_name TEXT,
  forked_from TEXT REFERENCES sketches (id) ON DELETE SET NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  published_at INTEGER,
  like_count INTEGER NOT NULL DEFAULT 0,
  comment_count INTEGER NOT NULL DEFAULT 0,
  view_count INTEGER NOT NULL DEFAULT 0,
  hidden_at INTEGER
);

CREATE INDEX sketches_by_owner ON sketches (owner_id, updated_at);
CREATE INDEX sketches_by_fork_source ON sketches (forked_from);
CREATE INDEX sketches_public_by_published ON sketches (published_at)
  WHERE visibility = 'public' AND hidden_at IS NULL;
CREATE INDEX sketches_public_by_likes ON sketches (like_count, published_at)
  WHERE visibility = 'public' AND hidden_at IS NULL;

CREATE TABLE sketch_files (
  sketch_id TEXT NOT NULL REFERENCES sketches (id) ON DELETE CASCADE,
  path TEXT NOT NULL,
  content TEXT NOT NULL,
  position INTEGER NOT NULL,
  PRIMARY KEY (sketch_id, path)
);

CREATE TABLE thumbnails (
  sketch_id TEXT PRIMARY KEY REFERENCES sketches (id) ON DELETE CASCADE,
  mime TEXT NOT NULL,
  data BLOB NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE likes (
  user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
  sketch_id TEXT NOT NULL REFERENCES sketches (id) ON DELETE CASCADE,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (user_id, sketch_id)
);

CREATE INDEX likes_by_sketch ON likes (sketch_id, created_at);

CREATE TABLE comments (
  id INTEGER PRIMARY KEY,
  sketch_id TEXT NOT NULL REFERENCES sketches (id) ON DELETE CASCADE,
  user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
  body TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  deleted_at INTEGER
);

CREATE INDEX comments_by_sketch ON comments (sketch_id, created_at);
CREATE INDEX comments_by_user ON comments (user_id);

CREATE TABLE playlists (
  id TEXT PRIMARY KEY,
  owner_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
  title TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  visibility TEXT NOT NULL
    CHECK (visibility IN ('private', 'unlisted', 'public')),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE INDEX playlists_by_owner ON playlists (owner_id, updated_at);

CREATE TABLE playlist_items (
  playlist_id TEXT NOT NULL REFERENCES playlists (id) ON DELETE CASCADE,
  sketch_id TEXT NOT NULL REFERENCES sketches (id) ON DELETE CASCADE,
  position INTEGER NOT NULL,
  added_at INTEGER NOT NULL,
  PRIMARY KEY (playlist_id, sketch_id)
);

CREATE INDEX playlist_items_by_sketch ON playlist_items (sketch_id);

CREATE TABLE reports (
  id INTEGER PRIMARY KEY,
  reporter_id INTEGER REFERENCES users (id) ON DELETE SET NULL,
  kind TEXT NOT NULL CHECK (kind IN ('sketch', 'comment', 'user')),
  target TEXT NOT NULL,
  reason TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  resolved_at INTEGER,
  resolved_by INTEGER REFERENCES users (id) ON DELETE SET NULL
);

CREATE INDEX reports_open ON reports (created_at) WHERE resolved_at IS NULL;

-- The denormalized counters follow the rows they count in the same
-- transaction, including rows removed by cascading deletes (a deleted
-- account's likes and comments).

CREATE TRIGGER likes_count_insert AFTER INSERT ON likes
BEGIN
  UPDATE sketches SET like_count = like_count + 1 WHERE id = NEW.sketch_id;
END;

CREATE TRIGGER likes_count_delete AFTER DELETE ON likes
BEGIN
  UPDATE sketches SET like_count = like_count - 1 WHERE id = OLD.sketch_id;
END;

CREATE TRIGGER comments_count_insert AFTER INSERT ON comments
WHEN NEW.deleted_at IS NULL
BEGIN
  UPDATE sketches SET comment_count = comment_count + 1
  WHERE id = NEW.sketch_id;
END;

CREATE TRIGGER comments_count_soft_delete AFTER UPDATE OF deleted_at ON comments
WHEN OLD.deleted_at IS NULL AND NEW.deleted_at IS NOT NULL
BEGIN
  UPDATE sketches SET comment_count = comment_count - 1
  WHERE id = NEW.sketch_id;
END;

CREATE TRIGGER comments_count_delete AFTER DELETE ON comments
WHEN OLD.deleted_at IS NULL
BEGIN
  UPDATE sketches SET comment_count = comment_count - 1
  WHERE id = OLD.sketch_id;
END;
