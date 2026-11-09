//! Playlists: ordered collections of sketches.

use anyhow::anyhow;
use serde::Serialize;
use sqlx::{FromRow, SqlitePool};

use crate::auth::{CurrentUser, Viewer};
use crate::db::{ID_ATTEMPTS, is_unique_violation, new_public_id, now};
use crate::error::AppError;
use crate::sketches::{self, SketchSummary};
use crate::users::Author;
use crate::visibility::{Access, Visibility};

/// The most sketches a playlist holds.
pub const MAX_ITEMS: i64 = 500;

/// A playlist joined with its owner and size.
#[derive(Clone, Debug, FromRow)]
pub struct PlaylistRow {
  pub id: String,
  pub owner_id: i64,
  pub title: String,
  pub description: String,
  #[sqlx(try_from = "String")]
  pub visibility: Visibility,
  pub created_at: i64,
  pub updated_at: i64,
  pub owner_username: String,
  pub owner_display_name: String,
  pub owner_banned_at: Option<i64>,
  /// Every item, including ones some viewers can't see.
  pub item_count: i64,
}

const SELECT: &str = "SELECT p.id, p.owner_id, p.title, p.description, \
  p.visibility, p.created_at, p.updated_at, u.username AS owner_username, \
  u.display_name AS owner_display_name, u.banned_at AS owner_banned_at, \
  (SELECT COUNT(*) FROM playlist_items i WHERE i.playlist_id = p.id) \
    AS item_count \
  FROM playlists p JOIN users u ON u.id = p.owner_id";

/// A playlist as the API returns it.
#[derive(Clone, Debug, Serialize)]
pub struct Playlist {
  pub id: String,
  pub title: String,
  pub description: String,
  pub visibility: Visibility,
  pub owner: Author,
  pub items: Vec<SketchSummary>,
  pub created_at: i64,
  pub updated_at: i64,
  pub can_edit: bool,
}

/// One of the signed-in user's playlists, for "add to playlist" menus.
#[derive(Clone, Debug, Serialize, FromRow)]
pub struct OwnPlaylist {
  pub id: String,
  pub title: String,
  #[sqlx(try_from = "String")]
  pub visibility: Visibility,
  pub item_count: i64,
  pub contains: bool,
}

impl PlaylistRow {
  pub fn access(&self) -> Access {
    Access {
      owner_id: self.owner_id,
      visibility: self.visibility,
      hidden: false,
      owner_banned: self.owner_banned_at.is_some(),
    }
  }

  pub fn visible_to(&self, viewer: &Viewer) -> bool {
    self.access().visible_to(viewer)
  }

  pub fn owner(&self) -> Author {
    Author::new(&self.owner_username, &self.owner_display_name)
  }

  /// The API shape, with the items the viewer can see.
  pub async fn detail(
    &self,
    db: &SqlitePool,
    viewer: &Viewer,
  ) -> sqlx::Result<Playlist> {
    let items = sketches::in_playlist(db, viewer, &self.id)
      .await?
      .iter()
      .map(|row| row.summary(viewer))
      .collect();
    Ok(Playlist {
      id: self.id.clone(),
      title: self.title.clone(),
      description: self.description.clone(),
      visibility: self.visibility,
      owner: self.owner(),
      items,
      created_at: self.created_at,
      updated_at: self.updated_at,
      can_edit: viewer.is(self.owner_id),
    })
  }
}

pub async fn find(
  db: &SqlitePool,
  id: &str,
) -> sqlx::Result<Option<PlaylistRow>> {
  sqlx::query_as(&format!("{SELECT} WHERE p.id = ?"))
    .bind(id)
    .fetch_optional(db)
    .await
}

/// A playlist the viewer can see; [`AppError::NotFound`] otherwise.
pub async fn find_visible(
  db: &SqlitePool,
  id: &str,
  viewer: &Viewer,
) -> Result<PlaylistRow, AppError> {
  match find(db, id).await? {
    Some(row) if row.visible_to(viewer) => Ok(row),
    _ => Err(AppError::NotFound),
  }
}

/// A playlist the viewer owns: [`AppError::NotFound`] when they can't see
/// it, [`AppError::Forbidden`] when they can but don't own it.
pub async fn find_owned(
  db: &SqlitePool,
  id: &str,
  viewer: &Viewer,
) -> Result<PlaylistRow, AppError> {
  let row = find_visible(db, id, viewer).await?;
  if viewer.is(row.owner_id) {
    Ok(row)
  } else {
    Err(AppError::forbidden(
      "Only the playlist's owner can change it.",
    ))
  }
}

/// A user's playlists, most recently updated first: all of them for
/// `include_unlisted` (the owner's own view), else the public ones.
pub async fn by_owner(
  db: &SqlitePool,
  owner_id: i64,
  include_unlisted: bool,
) -> sqlx::Result<Vec<PlaylistRow>> {
  let filter = if include_unlisted {
    "1"
  } else {
    "p.visibility = 'public' AND u.banned_at IS NULL"
  };
  sqlx::query_as(&format!(
    "{SELECT} WHERE p.owner_id = ? AND {filter} \
     ORDER BY p.updated_at DESC, p.id"
  ))
  .bind(owner_id)
  .fetch_all(db)
  .await
}

/// The user's playlists, each saying whether it holds `sketch_id`.
pub async fn own_with_membership(
  db: &SqlitePool,
  owner_id: i64,
  sketch_id: Option<&str>,
) -> sqlx::Result<Vec<OwnPlaylist>> {
  sqlx::query_as(
    "SELECT p.id, p.title, p.visibility, \
     (SELECT COUNT(*) FROM playlist_items i WHERE i.playlist_id = p.id) \
       AS item_count, \
     EXISTS (SELECT 1 FROM playlist_items i WHERE i.playlist_id = p.id \
       AND i.sketch_id IS ?) AS contains \
     FROM playlists p WHERE p.owner_id = ? ORDER BY p.updated_at DESC, p.id",
  )
  .bind(sketch_id)
  .bind(owner_id)
  .fetch_all(db)
  .await
}

/// Whether `user` may give a playlist `visibility`: publishing needs a
/// verified email.
pub fn check_visibility(
  user: &CurrentUser,
  visibility: Visibility,
) -> Result<(), AppError> {
  if visibility == Visibility::Public {
    user.require_verified("publish playlists")?;
  }
  Ok(())
}

/// Creates a playlist, returning its id.
pub async fn create(
  db: &SqlitePool,
  owner_id: i64,
  title: &str,
  description: &str,
  visibility: Visibility,
) -> Result<String, AppError> {
  for _ in 0..ID_ATTEMPTS {
    let id = new_public_id();
    let now = now();
    let inserted = sqlx::query(
      "INSERT INTO playlists (id, owner_id, title, description, visibility, \
       created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(owner_id)
    .bind(title)
    .bind(description)
    .bind(visibility.as_str())
    .bind(now)
    .bind(now)
    .execute(db)
    .await;
    match inserted {
      Ok(_) => return Ok(id),
      Err(error) if is_unique_violation(&error) => continue,
      Err(error) => return Err(error.into()),
    }
  }
  Err(AppError::Internal(anyhow!(
    "couldn't find an unused playlist id"
  )))
}

pub async fn update(
  db: &SqlitePool,
  id: &str,
  title: &str,
  description: &str,
  visibility: Visibility,
) -> sqlx::Result<()> {
  sqlx::query(
    "UPDATE playlists SET title = ?, description = ?, visibility = ?, \
     updated_at = ? WHERE id = ?",
  )
  .bind(title)
  .bind(description)
  .bind(visibility.as_str())
  .bind(now())
  .bind(id)
  .execute(db)
  .await?;
  Ok(())
}

pub async fn delete(db: &SqlitePool, id: &str) -> sqlx::Result<()> {
  sqlx::query("DELETE FROM playlists WHERE id = ?")
    .bind(id)
    .execute(db)
    .await?;
  Ok(())
}

async fn touch(db: &SqlitePool, id: &str) -> sqlx::Result<()> {
  sqlx::query("UPDATE playlists SET updated_at = ? WHERE id = ?")
    .bind(now())
    .bind(id)
    .execute(db)
    .await?;
  Ok(())
}

/// Appends a sketch to a playlist; adding one it already holds changes
/// nothing.
pub async fn add_item(
  db: &SqlitePool,
  playlist_id: &str,
  sketch_id: &str,
) -> Result<(), AppError> {
  let count: i64 = sqlx::query_scalar(
    "SELECT COUNT(*) FROM playlist_items WHERE playlist_id = ?",
  )
  .bind(playlist_id)
  .fetch_one(db)
  .await?;
  if count >= MAX_ITEMS {
    return Err(AppError::bad_request(format!(
      "A playlist can hold at most {MAX_ITEMS} sketches."
    )));
  }
  sqlx::query(
    "INSERT OR IGNORE INTO playlist_items \
     (playlist_id, sketch_id, position, added_at) \
     VALUES (?, ?, (SELECT COALESCE(MAX(position) + 1, 0) \
       FROM playlist_items WHERE playlist_id = ?), ?)",
  )
  .bind(playlist_id)
  .bind(sketch_id)
  .bind(playlist_id)
  .bind(now())
  .execute(db)
  .await?;
  touch(db, playlist_id).await?;
  Ok(())
}

/// Removes a sketch from a playlist, returning whether it was there.
pub async fn remove_item(
  db: &SqlitePool,
  playlist_id: &str,
  sketch_id: &str,
) -> sqlx::Result<bool> {
  let result = sqlx::query(
    "DELETE FROM playlist_items WHERE playlist_id = ? AND sketch_id = ?",
  )
  .bind(playlist_id)
  .bind(sketch_id)
  .execute(db)
  .await?;
  if result.rows_affected() > 0 {
    touch(db, playlist_id).await?;
  }
  Ok(result.rows_affected() > 0)
}

/// Reorders a playlist: the sketches in `order` come first, in that order,
/// followed by any others (ones the owner can no longer see) in their
/// current order. Every id in `order` must be in the playlist, once.
pub async fn reorder(
  db: &SqlitePool,
  playlist_id: &str,
  order: &[String],
) -> Result<(), AppError> {
  let mut tx = db.begin().await?;
  let current: Vec<String> = sqlx::query_scalar(
    "SELECT sketch_id FROM playlist_items WHERE playlist_id = ? \
     ORDER BY position, added_at",
  )
  .bind(playlist_id)
  .fetch_all(&mut *tx)
  .await?;
  for (i, id) in order.iter().enumerate() {
    if !current.contains(id) {
      return Err(AppError::bad_request(format!(
        "The sketch {id:?} isn't in this playlist."
      )));
    }
    if order[..i].contains(id) {
      return Err(AppError::bad_request(format!(
        "The sketch {id:?} appears twice."
      )));
    }
  }
  let rest = current.iter().filter(|id| !order.contains(id));
  for (position, id) in order.iter().chain(rest).enumerate() {
    sqlx::query(
      "UPDATE playlist_items SET position = ? \
       WHERE playlist_id = ? AND sketch_id = ?",
    )
    .bind(position as i64)
    .bind(playlist_id)
    .bind(id)
    .execute(&mut *tx)
    .await?;
  }
  sqlx::query("UPDATE playlists SET updated_at = ? WHERE id = ?")
    .bind(now())
    .bind(playlist_id)
    .execute(&mut *tx)
    .await?;
  tx.commit().await?;
  Ok(())
}

/// Where a sketch sits in a playlist the viewer is playing through.
#[derive(Clone, Debug, Serialize)]
pub struct PlaylistPosition {
  pub id: String,
  pub title: String,
  /// The sketch's index among the items the viewer can see.
  pub index: usize,
  pub count: usize,
  /// The previous sketch's id, if any.
  pub prev: Option<String>,
  pub next: Option<String>,
}

/// The sketch's position in a playlist, if the viewer can see the playlist
/// and it holds the sketch.
pub async fn position_of(
  db: &SqlitePool,
  viewer: &Viewer,
  playlist_id: &str,
  sketch_id: &str,
) -> sqlx::Result<Option<PlaylistPosition>> {
  let Some(playlist) = find(db, playlist_id).await? else {
    return Ok(None);
  };
  if !playlist.visible_to(viewer) {
    return Ok(None);
  }
  let items = sketches::in_playlist(db, viewer, playlist_id).await?;
  let Some(index) = items.iter().position(|s| s.id == sketch_id) else {
    return Ok(None);
  };
  Ok(Some(PlaylistPosition {
    id: playlist.id,
    title: playlist.title,
    index,
    count: items.len(),
    prev: index.checked_sub(1).map(|i| items[i].id.clone()),
    next: items.get(index + 1).map(|s| s.id.clone()),
  }))
}
