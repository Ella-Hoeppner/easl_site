//! Comments on sketches.

use serde::Serialize;
use sqlx::{FromRow, SqlitePool};

use crate::auth::Viewer;
use crate::db::now;
use crate::users::Author;

/// A comment joined with its author and its sketch's owner.
#[derive(Clone, Debug, FromRow)]
pub struct CommentRow {
  pub id: i64,
  pub sketch_id: String,
  pub user_id: i64,
  pub body: String,
  pub created_at: i64,
  pub deleted_at: Option<i64>,
  pub author_username: String,
  pub author_display_name: String,
  pub author_banned_at: Option<i64>,
  pub sketch_owner_id: i64,
}

const SELECT: &str = "SELECT c.id, c.sketch_id, c.user_id, c.body, \
  c.created_at, c.deleted_at, u.username AS author_username, \
  u.display_name AS author_display_name, u.banned_at AS author_banned_at, \
  s.owner_id AS sketch_owner_id \
  FROM comments c JOIN users u ON u.id = c.user_id \
  JOIN sketches s ON s.id = c.sketch_id";

/// A comment as the API returns it.
#[derive(Clone, Debug, Serialize)]
pub struct Comment {
  pub id: i64,
  pub body: String,
  pub created_at: i64,
  pub author: Author,
  pub can_delete: bool,
}

impl CommentRow {
  /// Whether the viewer may delete the comment: its author, the sketch's
  /// owner, or an admin.
  pub fn deletable_by(&self, viewer: &Viewer) -> bool {
    viewer.is(self.user_id)
      || viewer.is(self.sketch_owner_id)
      || viewer.is_admin
  }

  pub fn to_api(&self, viewer: &Viewer) -> Comment {
    Comment {
      id: self.id,
      body: self.body.clone(),
      created_at: self.created_at,
      author: Author::new(&self.author_username, &self.author_display_name),
      can_delete: self.deletable_by(viewer),
    }
  }
}

/// A sketch's comments, oldest first, without deleted ones or (except for
/// admins) ones by banned users.
pub async fn list(
  db: &SqlitePool,
  sketch_id: &str,
  viewer: &Viewer,
) -> sqlx::Result<Vec<Comment>> {
  let rows: Vec<CommentRow> = sqlx::query_as(&format!(
    "{SELECT} WHERE c.sketch_id = ? AND c.deleted_at IS NULL \
     AND (u.banned_at IS NULL OR ?) ORDER BY c.created_at, c.id"
  ))
  .bind(sketch_id)
  .bind(viewer.is_admin)
  .fetch_all(db)
  .await?;
  Ok(rows.iter().map(|row| row.to_api(viewer)).collect())
}

/// A comment by id, deleted or not.
pub async fn find(
  db: &SqlitePool,
  id: i64,
) -> sqlx::Result<Option<CommentRow>> {
  sqlx::query_as(&format!("{SELECT} WHERE c.id = ?"))
    .bind(id)
    .fetch_optional(db)
    .await
}

/// Adds a comment, returning it.
pub async fn create(
  db: &SqlitePool,
  sketch_id: &str,
  user_id: i64,
  body: &str,
) -> sqlx::Result<CommentRow> {
  let id: i64 = sqlx::query_scalar(
    "INSERT INTO comments (sketch_id, user_id, body, created_at) \
     VALUES (?, ?, ?, ?) RETURNING id",
  )
  .bind(sketch_id)
  .bind(user_id)
  .bind(body)
  .bind(now())
  .fetch_one(db)
  .await?;
  find(db, id).await?.ok_or(sqlx::Error::RowNotFound)
}

/// Marks a comment deleted; it stays in the database for moderation.
pub async fn soft_delete(db: &SqlitePool, id: i64) -> sqlx::Result<()> {
  sqlx::query(
    "UPDATE comments SET deleted_at = ? WHERE id = ? AND deleted_at IS NULL",
  )
  .bind(now())
  .bind(id)
  .execute(db)
  .await?;
  Ok(())
}
