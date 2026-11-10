//! Sketch tags: short labels a sketch's owner gives it, for browsing
//! explore by topic.
//!
//! A sketch has at most [`MAX_TAGS`](crate::validate::MAX_TAGS) tags,
//! distinct and kept in the order its owner gave them
//! ([`crate::validate::tags`] checks and normalizes them). Like the title
//! and description they belong to the sketch, not its revisions.
//! [`crate::sketches`] reads a sketch's tags with the sketch and filters
//! explore by them.

use serde::Serialize;
use sqlx::{FromRow, SqliteConnection, SqlitePool};

use crate::visibility::LISTED_SKETCH_SQL;

/// A tag and how many publicly listed sketches have it.
#[derive(Clone, Debug, FromRow, Serialize, PartialEq, Eq)]
pub struct TagCount {
  pub tag: String,
  pub count: i64,
}

/// A sketch's tags in order.
pub async fn of(
  conn: &mut SqliteConnection,
  sketch_id: &str,
) -> sqlx::Result<Vec<String>> {
  sqlx::query_scalar(
    "SELECT tag FROM sketch_tags WHERE sketch_id = ? ORDER BY position",
  )
  .bind(sketch_id)
  .fetch_all(conn)
  .await
}

/// Replaces a sketch's tags with `tags` (already validated), in order.
pub async fn set(
  conn: &mut SqliteConnection,
  sketch_id: &str,
  tags: &[String],
) -> sqlx::Result<()> {
  sqlx::query("DELETE FROM sketch_tags WHERE sketch_id = ?")
    .bind(sketch_id)
    .execute(&mut *conn)
    .await?;
  for (position, tag) in tags.iter().enumerate() {
    sqlx::query(
      "INSERT INTO sketch_tags (sketch_id, tag, position) VALUES (?, ?, ?)",
    )
    .bind(sketch_id)
    .bind(tag)
    .bind(position as i64)
    .execute(&mut *conn)
    .await?;
  }
  Ok(())
}

/// The `limit` tags on the most publicly listed sketches, most used first
/// (ties alphabetically), counting only listed sketches.
pub async fn popular(
  db: &SqlitePool,
  limit: i64,
) -> sqlx::Result<Vec<TagCount>> {
  sqlx::query_as(&format!(
    "SELECT st.tag, COUNT(*) AS count FROM sketch_tags st \
     JOIN sketches s ON s.id = st.sketch_id \
     JOIN users u ON u.id = s.owner_id \
     WHERE {LISTED_SKETCH_SQL} \
     GROUP BY st.tag ORDER BY count DESC, st.tag LIMIT ?"
  ))
  .bind(limit)
  .fetch_all(db)
  .await
}
