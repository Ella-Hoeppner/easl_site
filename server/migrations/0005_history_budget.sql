-- Budgets: how much revision history sketches keep, and how many emails
-- the site sends a day.
--
-- Each revision records its size (`revisions::size`: its files' paths and
-- contents in bytes, plus 64 bytes per file and 256 per revision for the
-- rows holding them), and each sketch the total size of its revisions in
-- `history_bytes`, kept by triggers (as the like and comment counts are),
-- so a save can check its sketch's and its owner's totals without reading
-- any files. A save past the sketch's budget prunes its oldest revisions;
-- one past the account's is refused.

ALTER TABLE sketch_revisions ADD COLUMN size INTEGER NOT NULL DEFAULT 0;

UPDATE sketch_revisions SET size = 256 + COALESCE((
  SELECT SUM(64 + length(CAST(f.path AS BLOB))
    + length(CAST(f.content AS BLOB)))
  FROM revision_files f
  WHERE f.sketch_id = sketch_revisions.sketch_id
    AND f.number = sketch_revisions.number
), 0);

ALTER TABLE sketches ADD COLUMN history_bytes INTEGER NOT NULL DEFAULT 0;

UPDATE sketches SET history_bytes = (
  SELECT COALESCE(SUM(r.size), 0) FROM sketch_revisions r
  WHERE r.sketch_id = sketches.id
);

CREATE TRIGGER revisions_history_insert AFTER INSERT ON sketch_revisions
BEGIN
  UPDATE sketches SET history_bytes = history_bytes + NEW.size
  WHERE id = NEW.sketch_id;
END;

CREATE TRIGGER revisions_history_delete AFTER DELETE ON sketch_revisions
BEGIN
  UPDATE sketches SET history_bytes = history_bytes - OLD.size
  WHERE id = OLD.sketch_id;
END;

-- How many emails the site sent each day (UTC days since the unix epoch),
-- checked against its daily budget before each email (`outbox`).

CREATE TABLE email_days (
  day INTEGER PRIMARY KEY,
  sent INTEGER NOT NULL
);
