//! Sketch revisions: the saved versions of a sketch's program (its files
//! and entry point), numbered from 1 per sketch.
//!
//! A sketch points at two of them: its head (the latest saved revision,
//! which its owner works on) and, while it's unlisted or public, its
//! published revision (what everyone else sees). [`crate::sketches`] moves
//! those pointers; this module stores and reads the revisions themselves.
//!
//! A sketch's history has a size budget
//! ([`crate::config::HistoryBudget`]): each revision records its [`size`],
//! the sketch keeps their total (`sketches.history_bytes`, maintained by
//! triggers), and a save that takes it past the budget [`prune`]s the
//! oldest revisions other than the head and the published one. Numbers are
//! never reused: a new revision is numbered after the highest so far, which
//! is never pruned (it's the head when it's saved, and pruning happens only
//! then).

use serde::Serialize;
use sqlx::{FromRow, SqliteConnection, SqlitePool};

use crate::validate::SketchFile;

/// A sketch's program: what a revision holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Contents {
  pub files: Vec<SketchFile>,
  pub entry_file: Option<String>,
  pub entry_name: Option<String>,
}

/// One revision in a sketch's history, as `GET
/// /api/sketches/{id}/revisions` lists it.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct RevisionInfo {
  pub number: i64,
  pub created_at: i64,
  /// It's the sketch's published revision.
  pub published: bool,
  /// It's the sketch's head.
  pub head: bool,
}

/// What a revision's size counts for the row holding it, beyond its files.
pub const REVISION_OVERHEAD_BYTES: i64 = 256;

/// What a revision's size counts for each file's row, beyond its path and
/// content.
pub const FILE_OVERHEAD_BYTES: i64 = 64;

/// The size a revision of `contents` counts against its history's budget:
/// its files' paths and contents in bytes, plus a fixed overhead per file
/// and per revision for the rows holding them. (Migration
/// `0005_history_budget.sql` measured existing revisions the same way.)
pub fn size(contents: &Contents) -> i64 {
  let files: usize = contents
    .files
    .iter()
    .map(|file| file.path.len() + file.content.len())
    .sum();
  REVISION_OVERHEAD_BYTES
    + FILE_OVERHEAD_BYTES * contents.files.len() as i64
    + files as i64
}

/// Whether a sketch has a revision with this number.
pub async fn exists(
  conn: &mut SqliteConnection,
  sketch_id: &str,
  number: i64,
) -> sqlx::Result<bool> {
  sqlx::query_scalar(
    "SELECT EXISTS (SELECT 1 FROM sketch_revisions \
     WHERE sketch_id = ? AND number = ?)",
  )
  .bind(sketch_id)
  .bind(number)
  .fetch_one(conn)
  .await
}

/// A revision's files in order.
pub async fn files(
  conn: &mut SqliteConnection,
  sketch_id: &str,
  number: i64,
) -> sqlx::Result<Vec<SketchFile>> {
  let rows: Vec<(String, String)> = sqlx::query_as(
    "SELECT path, content FROM revision_files \
     WHERE sketch_id = ? AND number = ? ORDER BY position",
  )
  .bind(sketch_id)
  .bind(number)
  .fetch_all(conn)
  .await?;
  Ok(
    rows
      .into_iter()
      .map(|(path, content)| SketchFile { path, content })
      .collect(),
  )
}

/// A revision's files and entry point.
pub async fn contents(
  conn: &mut SqliteConnection,
  sketch_id: &str,
  number: i64,
) -> sqlx::Result<Contents> {
  let (entry_file, entry_name): (Option<String>, Option<String>) =
    sqlx::query_as(
      "SELECT entry_file, entry_name FROM sketch_revisions \
       WHERE sketch_id = ? AND number = ?",
    )
    .bind(sketch_id)
    .bind(number)
    .fetch_one(&mut *conn)
    .await?;
  Ok(Contents {
    files: files(conn, sketch_id, number).await?,
    entry_file,
    entry_name,
  })
}

/// Stores a revision with a given number.
pub async fn insert(
  conn: &mut SqliteConnection,
  sketch_id: &str,
  number: i64,
  contents: &Contents,
  created_at: i64,
) -> sqlx::Result<()> {
  sqlx::query(
    "INSERT INTO sketch_revisions (sketch_id, number, entry_file, \
     entry_name, created_at, size) VALUES (?, ?, ?, ?, ?, ?)",
  )
  .bind(sketch_id)
  .bind(number)
  .bind(&contents.entry_file)
  .bind(&contents.entry_name)
  .bind(created_at)
  .bind(size(contents))
  .execute(&mut *conn)
  .await?;
  for (position, file) in contents.files.iter().enumerate() {
    sqlx::query(
      "INSERT INTO revision_files (sketch_id, number, path, content, \
       position) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(sketch_id)
    .bind(number)
    .bind(&file.path)
    .bind(&file.content)
    .bind(position as i64)
    .execute(&mut *conn)
    .await?;
  }
  Ok(())
}

/// Stores `contents` as a new revision numbered after the sketch's highest
/// so far, unless they're the same as revision `head`'s. Returns the new
/// revision's number, or `None` when nothing was stored. Moving the
/// sketch's head is up to the caller.
pub async fn save(
  conn: &mut SqliteConnection,
  sketch_id: &str,
  head: i64,
  contents: &Contents,
  created_at: i64,
) -> sqlx::Result<Option<i64>> {
  if self::contents(conn, sketch_id, head).await? == *contents {
    return Ok(None);
  }
  let number: i64 = sqlx::query_scalar(
    "SELECT COALESCE(MAX(number), 0) + 1 FROM sketch_revisions \
     WHERE sketch_id = ?",
  )
  .bind(sketch_id)
  .fetch_one(&mut *conn)
  .await?;
  insert(conn, sketch_id, number, contents, created_at).await?;
  Ok(Some(number))
}

/// The total size of a sketch's revisions.
pub async fn history_bytes(
  conn: &mut SqliteConnection,
  sketch_id: &str,
) -> sqlx::Result<i64> {
  sqlx::query_scalar("SELECT history_bytes FROM sketches WHERE id = ?")
    .bind(sketch_id)
    .fetch_one(conn)
    .await
}

/// Deletes a sketch's oldest revisions, never those numbered in `keep`
/// (its head and published revision), until its history fits in `budget`
/// bytes, or until only those are left. Returns how many it deleted.
pub async fn prune(
  conn: &mut SqliteConnection,
  sketch_id: &str,
  budget: i64,
  keep: [Option<i64>; 2],
) -> sqlx::Result<u64> {
  let excess = history_bytes(conn, sketch_id).await? - budget;
  if excess <= 0 {
    return Ok(0);
  }
  // The oldest revision whose deletion, with every prunable one before it,
  // frees enough; none when even deleting them all doesn't.
  let last: Option<i64> = sqlx::query_scalar(
    "SELECT MIN(number) FROM (SELECT number, \
     SUM(size) OVER (ORDER BY number) AS freed FROM sketch_revisions \
     WHERE sketch_id = ? AND number IS NOT ? AND number IS NOT ?) \
     WHERE freed >= ?",
  )
  .bind(sketch_id)
  .bind(keep[0])
  .bind(keep[1])
  .bind(excess)
  .fetch_one(&mut *conn)
  .await?;
  let deleted = sqlx::query(
    "DELETE FROM sketch_revisions WHERE sketch_id = ? AND number <= ? \
     AND number IS NOT ? AND number IS NOT ?",
  )
  .bind(sketch_id)
  .bind(last.unwrap_or(i64::MAX))
  .bind(keep[0])
  .bind(keep[1])
  .execute(conn)
  .await?;
  Ok(deleted.rows_affected())
}

#[derive(FromRow)]
struct RevisionRow {
  number: i64,
  created_at: i64,
}

/// A sketch's revisions, newest first, marked against its head and
/// published revision.
pub async fn list(
  db: &SqlitePool,
  sketch_id: &str,
  head: i64,
  published: Option<i64>,
) -> sqlx::Result<Vec<RevisionInfo>> {
  let rows: Vec<RevisionRow> = sqlx::query_as(
    "SELECT number, created_at FROM sketch_revisions WHERE sketch_id = ? \
     ORDER BY number DESC",
  )
  .bind(sketch_id)
  .fetch_all(db)
  .await?;
  Ok(
    rows
      .into_iter()
      .map(|row| RevisionInfo {
        number: row.number,
        created_at: row.created_at,
        published: published == Some(row.number),
        head: row.number == head,
      })
      .collect(),
  )
}
