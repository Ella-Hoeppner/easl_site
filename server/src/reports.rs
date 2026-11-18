//! Reports of sketches, comments, and users, for moderators.

use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqliteExecutor, SqlitePool};

use crate::db::now;
use crate::moderation;
use crate::sketches::Page;

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

/// A report, with what the admin page shows about its target.
#[derive(Clone, Debug, Serialize, FromRow)]
pub struct Report {
  pub id: i64,
  pub kind: String,
  pub target: String,
  pub reason: String,
  pub created_at: i64,
  pub reporter_username: Option<String>,
  pub resolved_at: Option<i64>,
  /// The admin who resolved it, while their account exists.
  pub resolver_username: Option<String>,
  /// The reported sketch, or the sketch a reported comment is on.
  pub sketch_id: Option<String>,
  pub sketch_title: Option<String>,
  pub sketch_hidden: Option<bool>,
  pub comment_id: Option<i64>,
  pub comment_body: Option<String>,
  pub comment_deleted: Option<bool>,
  /// The reported user, or the author of the reported sketch or comment.
  pub user_id: Option<i64>,
  pub username: Option<String>,
  pub user_banned: Option<bool>,
}

/// Reports with what [`Report`] shows of their targets.
pub const SELECT: &str = "SELECT r.id, r.kind, r.target, r.reason, \
  r.created_at, reporter.username AS reporter_username, r.resolved_at, \
  resolver.username AS resolver_username, \
  s.id AS sketch_id, s.title AS sketch_title, \
  s.hidden_at IS NOT NULL AS sketch_hidden, \
  c.id AS comment_id, c.body AS comment_body, \
  c.deleted_at IS NOT NULL AS comment_deleted, \
  u.id AS user_id, u.username AS username, \
  u.banned_at IS NOT NULL AS user_banned \
  FROM reports r \
  LEFT JOIN users reporter ON reporter.id = r.reporter_id \
  LEFT JOIN users resolver ON resolver.id = r.resolved_by \
  LEFT JOIN comments c ON r.kind = 'comment' \
    AND c.id = CAST(r.target AS INTEGER) \
  LEFT JOIN sketches s ON s.id = CASE r.kind \
    WHEN 'sketch' THEN r.target WHEN 'comment' THEN c.sketch_id END \
  LEFT JOIN users u ON u.id = CASE r.kind \
    WHEN 'user' THEN (SELECT id FROM users WHERE username = r.target) \
    WHEN 'comment' THEN c.user_id WHEN 'sketch' THEN s.owner_id END";

/// Every unresolved report, oldest first.
pub async fn open(db: &SqlitePool) -> sqlx::Result<Vec<Report>> {
  sqlx::query_as(&format!(
    "{SELECT} WHERE r.resolved_at IS NULL ORDER BY r.created_at, r.id"
  ))
  .fetch_all(db)
  .await
}

/// A page of resolved reports, the most recently resolved first.
pub async fn resolved(
  db: &SqlitePool,
  page: i64,
) -> sqlx::Result<Page<Report>> {
  let rows = sqlx::query_as(&format!(
    "{SELECT} WHERE r.resolved_at IS NOT NULL \
     ORDER BY r.resolved_at DESC, r.id DESC LIMIT ? OFFSET ?"
  ))
  .bind(moderation::PAGE_SIZE + 1)
  .bind(moderation::page_offset(page))
  .fetch_all(db)
  .await?;
  Ok(Page::split(rows, moderation::PAGE_SIZE))
}

/// Reports a user submitted, newest first: `size` of them from `offset`.
pub async fn submitted_by(
  db: &SqlitePool,
  user_id: i64,
  size: i64,
  offset: i64,
) -> sqlx::Result<Page<Report>> {
  let rows = sqlx::query_as(&format!(
    "{SELECT} WHERE r.reporter_id = ? \
     ORDER BY r.created_at DESC, r.id DESC LIMIT ? OFFSET ?"
  ))
  .bind(user_id)
  .bind(size + 1)
  .bind(offset)
  .fetch_all(db)
  .await?;
  Ok(Page::split(rows, size))
}

/// Reports against a user (of them, their sketches, or their comments),
/// newest first: `size` of them from `offset`.
pub async fn against(
  db: &SqlitePool,
  user_id: i64,
  size: i64,
  offset: i64,
) -> sqlx::Result<Page<Report>> {
  let rows = sqlx::query_as(&format!(
    "{SELECT} WHERE u.id = ? \
     ORDER BY r.created_at DESC, r.id DESC LIMIT ? OFFSET ?"
  ))
  .bind(user_id)
  .bind(size + 1)
  .bind(offset)
  .fetch_all(db)
  .await?;
  Ok(Page::split(rows, size))
}

/// A report by id.
pub async fn find(
  db: impl SqliteExecutor<'_>,
  id: i64,
) -> sqlx::Result<Option<Report>> {
  sqlx::query_as(&format!("{SELECT} WHERE r.id = ?"))
    .bind(id)
    .fetch_optional(db)
    .await
}

/// Marks a report resolved by an admin (as of `at`), returning whether it
/// was open.
pub async fn resolve(
  db: impl SqliteExecutor<'_>,
  id: i64,
  admin_id: i64,
  at: i64,
) -> sqlx::Result<bool> {
  let result = sqlx::query(
    "UPDATE reports SET resolved_at = ?, resolved_by = ? \
     WHERE id = ? AND resolved_at IS NULL",
  )
  .bind(at)
  .bind(admin_id)
  .bind(id)
  .execute(db)
  .await?;
  Ok(result.rows_affected() > 0)
}
