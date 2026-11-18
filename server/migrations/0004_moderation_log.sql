-- The moderation log: one row per admin action (hiding or unhiding a
-- sketch, deleting someone else's sketch or comment, restoring a comment,
-- banning or unbanning a user, resolving a report, granting or revoking
-- admin rights), written in the same transaction as the action. Besides
-- the target's id (a sketch id, comment id, username, or report id) it
-- keeps what the log shows of the target as it was then: the user it is
-- or belongs to, the sketch it is or is on and that sketch's title, and an
-- excerpt of a comment (`detail`), so the log stays readable after the
-- target is deleted. `admin_id` is NULL for actions taken from the command
-- line (`note` says so) and once the admin's account is deleted. The
-- kinds of action and target are listed in `src/moderation.rs`, not
-- checked here, so a new kind needs no table rebuild.

CREATE TABLE moderation_actions (
  id INTEGER PRIMARY KEY,
  admin_id INTEGER REFERENCES users (id) ON DELETE SET NULL,
  action TEXT NOT NULL,
  target_kind TEXT NOT NULL,
  target TEXT NOT NULL,
  target_user_id INTEGER REFERENCES users (id) ON DELETE SET NULL,
  target_username TEXT,
  sketch_id TEXT,
  sketch_title TEXT,
  detail TEXT,
  note TEXT,
  created_at INTEGER NOT NULL
);

CREATE INDEX moderation_actions_by_time
  ON moderation_actions (created_at, id);
CREATE INDEX moderation_actions_by_target
  ON moderation_actions (target_kind, target);
CREATE INDEX moderation_actions_by_admin ON moderation_actions (admin_id)
  WHERE admin_id IS NOT NULL;
CREATE INDEX moderation_actions_by_target_user
  ON moderation_actions (target_user_id) WHERE target_user_id IS NOT NULL;

-- Who deleted a comment: its author, the sketch's owner, or an admin.
-- NULL for comments deleted before this was recorded, and once the
-- deleter's account is deleted.
ALTER TABLE comments ADD COLUMN deleted_by INTEGER
  REFERENCES users (id) ON DELETE SET NULL;

CREATE INDEX comments_deleted ON comments (deleted_at)
  WHERE deleted_at IS NOT NULL;
CREATE INDEX comments_by_deleter ON comments (deleted_by)
  WHERE deleted_by IS NOT NULL;

CREATE INDEX users_banned ON users (banned_at) WHERE banned_at IS NOT NULL;
CREATE INDEX sketches_hidden ON sketches (hidden_at)
  WHERE hidden_at IS NOT NULL;

-- Restoring a soft-deleted comment counts it again.
CREATE TRIGGER comments_count_restore AFTER UPDATE OF deleted_at ON comments
WHEN OLD.deleted_at IS NOT NULL AND NEW.deleted_at IS NULL
BEGIN
  UPDATE sketches SET comment_count = comment_count + 1
  WHERE id = NEW.sketch_id;
END;

-- Reports resolved so far go into the log as they would have: who resolved
-- each and when, with its target as it is now. Earlier bans, hides, and
-- deletions weren't recorded, so the log can't say who did them.
INSERT INTO moderation_actions (admin_id, action, target_kind, target,
  target_user_id, target_username, sketch_id, sketch_title, detail,
  created_at)
SELECT r.resolved_by, 'resolve_report', 'report', CAST(r.id AS TEXT),
  u.id, u.username, s.id, s.title,
  CASE WHEN length(c.body) > 200 THEN substr(c.body, 1, 200) || '…'
    ELSE c.body END,
  r.resolved_at
FROM reports r
LEFT JOIN comments c ON r.kind = 'comment'
  AND c.id = CAST(r.target AS INTEGER)
LEFT JOIN sketches s ON s.id = CASE r.kind
  WHEN 'sketch' THEN r.target WHEN 'comment' THEN c.sketch_id END
LEFT JOIN users u ON u.id = CASE r.kind
  WHEN 'user' THEN (SELECT id FROM users WHERE username = r.target)
  WHEN 'comment' THEN c.user_id WHEN 'sketch' THEN s.owner_id END
WHERE r.resolved_at IS NOT NULL
ORDER BY r.resolved_at, r.id;
