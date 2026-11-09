//! Reports of sketches, comments, and users, for moderators.

use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool};

use crate::db::now;

/// What a report is about; its target is a sketch id, a comment id, or a
/// username.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReportKind {
  Sketch,
  Comment,
  User,
}

impl ReportKind {
  pub fn as_str(self) -> &'static str {
    match self {
      ReportKind::Sketch => "sketch",
      ReportKind::Comment => "comment",
      ReportKind::User => "user",
    }
  }
}

pub async fn create(
  db: &SqlitePool,
  reporter_id: i64,
  kind: ReportKind,
  target: &str,
  reason: &str,
) -> sqlx::Result<i64> {
  sqlx::query_scalar(
    "INSERT INTO reports (reporter_id, kind, target, reason, created_at) \
     VALUES (?, ?, ?, ?, ?) RETURNING id",
  )
  .bind(reporter_id)
  .bind(kind.as_str())
  .bind(target)
  .bind(reason)
  .bind(now())
  .fetch_one(db)
  .await
}

/// An unresolved report, with what the admin page shows about its target.
#[derive(Clone, Debug, Serialize, FromRow)]
pub struct OpenReport {
  pub id: i64,
  pub kind: String,
  pub target: String,
  pub reason: String,
  pub created_at: i64,
  pub reporter_username: Option<String>,
  /// The reported sketch, or the sketch a reported comment is on.
  pub sketch_id: Option<String>,
  pub sketch_title: Option<String>,
  pub sketch_hidden: Option<bool>,
  pub comment_id: Option<i64>,
  pub comment_body: Option<String>,
  pub comment_deleted: Option<bool>,
  /// The reported user, or the author of the reported sketch or comment.
  pub username: Option<String>,
  pub user_banned: Option<bool>,
}

/// Every unresolved report, oldest first.
pub async fn open(db: &SqlitePool) -> sqlx::Result<Vec<OpenReport>> {
  sqlx::query_as(
    "SELECT r.id, r.kind, r.target, r.reason, r.created_at, \
     reporter.username AS reporter_username, \
     s.id AS sketch_id, s.title AS sketch_title, \
     s.hidden_at IS NOT NULL AS sketch_hidden, \
     c.id AS comment_id, c.body AS comment_body, \
     c.deleted_at IS NOT NULL AS comment_deleted, \
     u.username AS username, u.banned_at IS NOT NULL AS user_banned \
     FROM reports r \
     LEFT JOIN users reporter ON reporter.id = r.reporter_id \
     LEFT JOIN comments c ON r.kind = 'comment' \
       AND c.id = CAST(r.target AS INTEGER) \
     LEFT JOIN sketches s ON s.id = CASE r.kind \
       WHEN 'sketch' THEN r.target WHEN 'comment' THEN c.sketch_id END \
     LEFT JOIN users u ON u.id = CASE r.kind \
       WHEN 'user' THEN (SELECT id FROM users WHERE username = r.target) \
       WHEN 'comment' THEN c.user_id WHEN 'sketch' THEN s.owner_id END \
     WHERE r.resolved_at IS NULL ORDER BY r.created_at, r.id",
  )
  .fetch_all(db)
  .await
}

/// Marks a report resolved, returning whether it exists.
pub async fn resolve(
  db: &SqlitePool,
  id: i64,
  admin_id: i64,
) -> sqlx::Result<bool> {
  let result = sqlx::query(
    "UPDATE reports SET resolved_at = COALESCE(resolved_at, ?), \
     resolved_by = COALESCE(resolved_by, ?) WHERE id = ?",
  )
  .bind(now())
  .bind(admin_id)
  .bind(id)
  .execute(db)
  .await?;
  Ok(result.rows_affected() > 0)
}
