//! Sketches: storage, visibility, listings, likes, thumbnails, and their
//! API shapes.

use anyhow::anyhow;
use serde::Serialize;
use sqlx::{FromRow, Sqlite, SqlitePool, Transaction};

use crate::auth::Viewer;
use crate::db::{ID_ATTEMPTS, is_unique_violation, new_public_id, now};
use crate::error::AppError;
use crate::users::Author;
use crate::validate::SketchFile;
use crate::visibility::{
  Access, LISTED_SKETCH_SQL, VISIBLE_SKETCH_SQL, Visibility,
};

/// How many sketches a listing page shows.
pub const PAGE_SIZE: i64 = 24;

/// The highest page number a listing serves.
pub const MAX_PAGE: i64 = 500;

/// A sketch joined with what its API shape and visibility need. Built by
/// queries starting with [`SELECT`], whose one parameter is the viewer's id
/// (for `liked`).
#[derive(Clone, Debug, FromRow)]
pub struct SketchRow {
  pub id: String,
  pub owner_id: i64,
  pub title: String,
  pub description: String,
  #[sqlx(try_from = "String")]
  pub visibility: Visibility,
  pub entry_file: Option<String>,
  pub entry_name: Option<String>,
  pub forked_from: Option<String>,
  pub created_at: i64,
  pub updated_at: i64,
  pub published_at: Option<i64>,
  pub like_count: i64,
  pub comment_count: i64,
  pub view_count: i64,
  pub hidden_at: Option<i64>,
  pub owner_username: String,
  pub owner_display_name: String,
  pub owner_banned_at: Option<i64>,
  pub thumbnail_updated_at: Option<i64>,
  pub liked: bool,
  pub fork_title: Option<String>,
  pub fork_visibility: Option<String>,
  pub fork_hidden_at: Option<i64>,
  pub fork_owner_id: Option<i64>,
  pub fork_owner_username: Option<String>,
  pub fork_owner_banned_at: Option<i64>,
}

/// The start of every [`SketchRow`] query; binds the viewer's id.
const SELECT: &str = "SELECT s.id, s.owner_id, s.title, s.description, \
  s.visibility, s.entry_file, s.entry_name, s.forked_from, s.created_at, \
  s.updated_at, s.published_at, s.like_count, s.comment_count, \
  s.view_count, s.hidden_at, u.username AS owner_username, \
  u.display_name AS owner_display_name, u.banned_at AS owner_banned_at, \
  t.updated_at AS thumbnail_updated_at, \
  EXISTS (SELECT 1 FROM likes l WHERE l.sketch_id = s.id \
    AND l.user_id IS ?) AS liked, \
  f.title AS fork_title, f.visibility AS fork_visibility, \
  f.hidden_at AS fork_hidden_at, f.owner_id AS fork_owner_id, \
  fu.username AS fork_owner_username, fu.banned_at AS fork_owner_banned_at \
  FROM sketches s \
  JOIN users u ON u.id = s.owner_id \
  LEFT JOIN thumbnails t ON t.sketch_id = s.id \
  LEFT JOIN sketches f ON f.id = s.forked_from \
  LEFT JOIN users fu ON fu.id = f.owner_id";

/// Where a sketch was forked from, when the viewer can see it.
#[derive(Clone, Debug, Serialize)]
pub struct ForkSource {
  pub id: String,
  pub title: String,
  pub owner_username: String,
}

/// A sketch without its files and description, for listings.
#[derive(Clone, Debug, Serialize)]
pub struct SketchSummary {
  pub id: String,
  pub title: String,
  pub visibility: Visibility,
  pub owner: Author,
  pub entry_file: Option<String>,
  pub entry_name: Option<String>,
  pub forked_from: Option<ForkSource>,
  pub created_at: i64,
  pub updated_at: i64,
  pub published_at: Option<i64>,
  pub like_count: i64,
  pub comment_count: i64,
  pub view_count: i64,
  pub liked: bool,
  pub can_edit: bool,
  /// Hidden by a moderator (only its owner and admins see it).
  pub hidden: bool,
  pub thumbnail_url: String,
  pub url: String,
}

/// A sketch with everything, as the API returns it.
#[derive(Clone, Debug, Serialize)]
pub struct Sketch {
  #[serde(flatten)]
  pub summary: SketchSummary,
  pub description: String,
  pub files: Vec<SketchFile>,
}

impl SketchRow {
  pub fn access(&self) -> Access {
    Access {
      owner_id: self.owner_id,
      visibility: self.visibility,
      hidden: self.hidden_at.is_some(),
      owner_banned: self.owner_banned_at.is_some(),
    }
  }

  pub fn visible_to(&self, viewer: &Viewer) -> bool {
    self.access().visible_to(viewer)
  }

  /// Where the sketch was forked from, if the viewer can see that sketch.
  fn fork_source(&self, viewer: &Viewer) -> Option<ForkSource> {
    let access = Access {
      owner_id: self.fork_owner_id?,
      visibility: Visibility::parse(self.fork_visibility.as_deref()?)?,
      hidden: self.fork_hidden_at.is_some(),
      owner_banned: self.fork_owner_banned_at.is_some(),
    };
    access.visible_to(viewer).then(|| ForkSource {
      id: self.forked_from.clone().unwrap_or_default(),
      title: self.fork_title.clone().unwrap_or_default(),
      owner_username: self.fork_owner_username.clone().unwrap_or_default(),
    })
  }

  /// The thumbnail's URL, versioned by when it (or, for the generated
  /// placeholder, the sketch's title) last changed.
  pub fn thumbnail_url(&self) -> String {
    thumbnail_url(
      &self.id,
      self.thumbnail_updated_at.unwrap_or(self.updated_at),
    )
  }

  pub fn summary(&self, viewer: &Viewer) -> SketchSummary {
    SketchSummary {
      id: self.id.clone(),
      title: self.title.clone(),
      visibility: self.visibility,
      owner: Author::new(&self.owner_username, &self.owner_display_name),
      entry_file: self.entry_file.clone(),
      entry_name: self.entry_name.clone(),
      forked_from: self.fork_source(viewer),
      created_at: self.created_at,
      updated_at: self.updated_at,
      published_at: self.published_at,
      like_count: self.like_count,
      comment_count: self.comment_count,
      view_count: self.view_count,
      liked: self.liked,
      can_edit: viewer.is(self.owner_id),
      hidden: self.hidden_at.is_some(),
      thumbnail_url: self.thumbnail_url(),
      url: format!("/s/{}", self.id),
    }
  }

  /// The full API shape, loading the files.
  pub async fn detail(
    &self,
    db: &SqlitePool,
    viewer: &Viewer,
  ) -> sqlx::Result<Sketch> {
    Ok(Sketch {
      summary: self.summary(viewer),
      description: self.description.clone(),
      files: files(db, &self.id).await?,
    })
  }
}

pub fn thumbnail_url(id: &str, version: i64) -> String {
  format!("/s/{id}/thumb?v={version}")
}

/// A sketch by id, whatever its visibility.
pub async fn find(
  db: &SqlitePool,
  id: &str,
  viewer: &Viewer,
) -> sqlx::Result<Option<SketchRow>> {
  sqlx::query_as(&format!("{SELECT} WHERE s.id = ?"))
    .bind(viewer.id)
    .bind(id)
    .fetch_optional(db)
    .await
}

/// A sketch the viewer can see; [`AppError::NotFound`] otherwise, so a
/// sketch the viewer can't see is indistinguishable from a missing one.
pub async fn find_visible(
  db: &SqlitePool,
  id: &str,
  viewer: &Viewer,
) -> Result<SketchRow, AppError> {
  match find(db, id, viewer).await? {
    Some(row) if row.visible_to(viewer) => Ok(row),
    _ => Err(AppError::NotFound),
  }
}

/// A sketch's files in order.
pub async fn files(db: &SqlitePool, id: &str) -> sqlx::Result<Vec<SketchFile>> {
  let rows: Vec<(String, String)> = sqlx::query_as(
    "SELECT path, content FROM sketch_files WHERE sketch_id = ? \
     ORDER BY position",
  )
  .bind(id)
  .fetch_all(db)
  .await?;
  Ok(
    rows
      .into_iter()
      .map(|(path, content)| SketchFile { path, content })
      .collect(),
  )
}

/// A validated new sketch.
#[derive(Clone, Debug)]
pub struct NewSketch {
  pub title: String,
  pub description: String,
  pub files: Vec<SketchFile>,
  pub entry_file: Option<String>,
  pub entry_name: Option<String>,
  pub visibility: Visibility,
  pub forked_from: Option<String>,
}

/// Stores a new sketch for `owner_id`, returning its id.
pub async fn create(
  db: &SqlitePool,
  owner_id: i64,
  sketch: &NewSketch,
) -> Result<String, AppError> {
  for _ in 0..ID_ATTEMPTS {
    let id = new_public_id();
    match insert(db, &id, owner_id, sketch).await {
      Ok(()) => return Ok(id),
      Err(error) if is_unique_violation(&error) => continue,
      Err(error) => return Err(error.into()),
    }
  }
  Err(AppError::Internal(anyhow!(
    "couldn't find an unused sketch id"
  )))
}

/// Stores a new sketch with a chosen id; a unique violation when the id is
/// taken.
pub async fn insert(
  db: &SqlitePool,
  id: &str,
  owner_id: i64,
  sketch: &NewSketch,
) -> sqlx::Result<()> {
  let mut tx = db.begin().await?;
  let now = now();
  sqlx::query(
    "INSERT INTO sketches (id, owner_id, title, description, visibility, \
     entry_file, entry_name, forked_from, created_at, updated_at, \
     published_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
  )
  .bind(id)
  .bind(owner_id)
  .bind(&sketch.title)
  .bind(&sketch.description)
  .bind(sketch.visibility.as_str())
  .bind(&sketch.entry_file)
  .bind(&sketch.entry_name)
  .bind(&sketch.forked_from)
  .bind(now)
  .bind(now)
  .bind((sketch.visibility == Visibility::Public).then_some(now))
  .execute(&mut *tx)
  .await?;
  write_files(&mut tx, id, &sketch.files).await?;
  tx.commit().await
}

async fn write_files(
  tx: &mut Transaction<'_, Sqlite>,
  id: &str,
  files: &[SketchFile],
) -> sqlx::Result<()> {
  sqlx::query("DELETE FROM sketch_files WHERE sketch_id = ?")
    .bind(id)
    .execute(&mut **tx)
    .await?;
  for (position, file) in files.iter().enumerate() {
    sqlx::query(
      "INSERT INTO sketch_files (sketch_id, path, content, position) \
       VALUES (?, ?, ?, ?)",
    )
    .bind(id)
    .bind(&file.path)
    .bind(&file.content)
    .bind(position as i64)
    .execute(&mut **tx)
    .await?;
  }
  Ok(())
}

/// A validated full new state of a sketch's editable fields; `files` is
/// `None` when they don't change.
#[derive(Clone, Debug)]
pub struct SketchUpdate {
  pub title: String,
  pub description: String,
  pub files: Option<Vec<SketchFile>>,
  pub entry_file: Option<String>,
  pub entry_name: Option<String>,
  pub visibility: Visibility,
}

/// Writes a sketch's new state, setting `published_at` the first time it
/// becomes public.
pub async fn update(
  db: &SqlitePool,
  id: &str,
  update: &SketchUpdate,
) -> sqlx::Result<()> {
  let mut tx = db.begin().await?;
  let now = now();
  sqlx::query(
    "UPDATE sketches SET title = ?, description = ?, entry_file = ?, \
     entry_name = ?, visibility = ?, updated_at = ?, \
     published_at = COALESCE(published_at, ?) WHERE id = ?",
  )
  .bind(&update.title)
  .bind(&update.description)
  .bind(&update.entry_file)
  .bind(&update.entry_name)
  .bind(update.visibility.as_str())
  .bind(now)
  .bind((update.visibility == Visibility::Public).then_some(now))
  .bind(id)
  .execute(&mut *tx)
  .await?;
  if let Some(files) = &update.files {
    write_files(&mut tx, id, files).await?;
  }
  tx.commit().await
}

pub async fn delete(db: &SqlitePool, id: &str) -> sqlx::Result<()> {
  sqlx::query("DELETE FROM sketches WHERE id = ?")
    .bind(id)
    .execute(db)
    .await?;
  Ok(())
}

/// Hides a sketch from everyone but its owner and admins, or un-hides it.
pub async fn set_hidden(
  db: &SqlitePool,
  id: &str,
  hidden: bool,
) -> sqlx::Result<bool> {
  let result = sqlx::query("UPDATE sketches SET hidden_at = ? WHERE id = ?")
    .bind(hidden.then(now))
    .bind(id)
    .execute(db)
    .await?;
  Ok(result.rows_affected() > 0)
}

pub async fn add_view(db: &SqlitePool, id: &str) -> sqlx::Result<()> {
  sqlx::query("UPDATE sketches SET view_count = view_count + 1 WHERE id = ?")
    .bind(id)
    .execute(db)
    .await?;
  Ok(())
}

/// Likes or unlikes a sketch for a user, returning its new like count.
/// Repeating either is harmless.
pub async fn set_liked(
  db: &SqlitePool,
  user_id: i64,
  sketch_id: &str,
  liked: bool,
) -> sqlx::Result<i64> {
  let mut tx = db.begin().await?;
  if liked {
    sqlx::query(
      "INSERT OR IGNORE INTO likes (user_id, sketch_id, created_at) \
       VALUES (?, ?, ?)",
    )
    .bind(user_id)
    .bind(sketch_id)
    .bind(now())
    .execute(&mut *tx)
    .await?;
  } else {
    sqlx::query("DELETE FROM likes WHERE user_id = ? AND sketch_id = ?")
      .bind(user_id)
      .bind(sketch_id)
      .execute(&mut *tx)
      .await?;
  }
  let count =
    sqlx::query_scalar("SELECT like_count FROM sketches WHERE id = ?")
      .bind(sketch_id)
      .fetch_one(&mut *tx)
      .await?;
  tx.commit().await?;
  Ok(count)
}

/// A stored thumbnail.
pub struct Thumbnail {
  pub mime: String,
  pub data: Vec<u8>,
}

pub async fn thumbnail(
  db: &SqlitePool,
  id: &str,
) -> sqlx::Result<Option<Thumbnail>> {
  let row: Option<(String, Vec<u8>)> =
    sqlx::query_as("SELECT mime, data FROM thumbnails WHERE sketch_id = ?")
      .bind(id)
      .fetch_optional(db)
      .await?;
  Ok(row.map(|(mime, data)| Thumbnail { mime, data }))
}

pub async fn set_thumbnail(
  db: &SqlitePool,
  id: &str,
  mime: &str,
  data: &[u8],
) -> sqlx::Result<()> {
  sqlx::query(
    "INSERT INTO thumbnails (sketch_id, mime, data, updated_at) \
     VALUES (?, ?, ?, ?) ON CONFLICT (sketch_id) DO UPDATE SET \
     mime = excluded.mime, data = excluded.data, \
     updated_at = excluded.updated_at",
  )
  .bind(id)
  .bind(mime)
  .bind(data)
  .bind(now())
  .execute(db)
  .await?;
  Ok(())
}

/// How explore orders sketches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sort {
  /// Most recently published first.
  New,
  /// Most liked first.
  Popular,
  /// Most liked in the last week first.
  Trending,
}

impl Sort {
  pub fn parse(text: Option<&str>) -> Self {
    match text {
      Some("popular") => Sort::Popular,
      Some("trending") => Sort::Trending,
      _ => Sort::New,
    }
  }

  pub fn as_str(self) -> &'static str {
    match self {
      Sort::New => "new",
      Sort::Popular => "popular",
      Sort::Trending => "trending",
    }
  }
}

/// How far back trending counts likes.
const TRENDING_SECONDS: i64 = 7 * 24 * 60 * 60;

/// One page of a listing, and whether another follows it.
pub struct Page<T> {
  pub items: Vec<T>,
  pub has_more: bool,
}

impl<T> Page<T> {
  /// Splits a fetch of `PAGE_SIZE + 1` rows into a page.
  fn from_overfetch(mut items: Vec<T>) -> Self {
    let has_more = items.len() as i64 > PAGE_SIZE;
    items.truncate(PAGE_SIZE as usize);
    Page { items, has_more }
  }
}

fn offset(page: i64) -> i64 {
  (page.clamp(1, MAX_PAGE) - 1) * PAGE_SIZE
}

/// Escapes `%`, `_`, and `\` for a `LIKE … ESCAPE '\'` pattern.
fn like_pattern(text: &str) -> String {
  let mut pattern = String::from("%");
  for c in text.chars() {
    if matches!(c, '%' | '_' | '\\') {
      pattern.push('\\');
    }
    pattern.push(c);
  }
  pattern.push('%');
  pattern
}

/// A page of publicly listed sketches, optionally matching a search in
/// their title, description, or author.
pub async fn explore(
  db: &SqlitePool,
  viewer: &Viewer,
  sort: Sort,
  search: Option<&str>,
  page: i64,
) -> sqlx::Result<Page<SketchRow>> {
  let search = search.map(str::trim).filter(|q| !q.is_empty());
  let search_sql = if search.is_some() {
    " AND (s.title LIKE ? ESCAPE '\\' OR s.description LIKE ? ESCAPE '\\' \
     OR u.username LIKE ? ESCAPE '\\' OR u.display_name LIKE ? ESCAPE '\\')"
  } else {
    ""
  };
  let order_sql = match sort {
    Sort::New => "s.published_at DESC, s.id",
    Sort::Popular => "s.like_count DESC, s.published_at DESC, s.id",
    Sort::Trending => {
      "(SELECT COUNT(*) FROM likes l WHERE l.sketch_id = s.id \
       AND l.created_at > ?) DESC, s.like_count DESC, s.published_at DESC, \
       s.id"
    }
  };
  let sql = format!(
    "{SELECT} WHERE {LISTED_SKETCH_SQL}{search_sql} ORDER BY {order_sql} \
     LIMIT ? OFFSET ?"
  );
  let mut query = sqlx::query_as(&sql).bind(viewer.id);
  if let Some(search) = search {
    let pattern = like_pattern(search);
    for _ in 0..4 {
      query = query.bind(pattern.clone());
    }
  }
  if sort == Sort::Trending {
    query = query.bind(now() - TRENDING_SECONDS);
  }
  let rows = query
    .bind(PAGE_SIZE + 1)
    .bind(offset(page))
    .fetch_all(db)
    .await?;
  Ok(Page::from_overfetch(rows))
}

/// A page of a user's sketches, most recently updated first: all of them
/// when `include_unlisted` (the owner's own view), else the publicly listed
/// ones.
pub async fn by_owner(
  db: &SqlitePool,
  viewer: &Viewer,
  owner_id: i64,
  include_unlisted: bool,
  page: i64,
) -> sqlx::Result<Page<SketchRow>> {
  let filter = if include_unlisted {
    "1"
  } else {
    LISTED_SKETCH_SQL
  };
  let rows = sqlx::query_as(&format!(
    "{SELECT} WHERE s.owner_id = ? AND {filter} \
     ORDER BY s.updated_at DESC, s.rowid DESC LIMIT ? OFFSET ?"
  ))
  .bind(viewer.id)
  .bind(owner_id)
  .bind(PAGE_SIZE + 1)
  .bind(offset(page))
  .fetch_all(db)
  .await?;
  Ok(Page::from_overfetch(rows))
}

/// A page of the sketches a user liked, most recent like first: the
/// publicly listed ones, plus, when `own_view` (the user looking at their
/// own likes), any others they can still see except other people's private
/// ones.
pub async fn liked_by(
  db: &SqlitePool,
  viewer: &Viewer,
  user_id: i64,
  own_view: bool,
  page: i64,
) -> sqlx::Result<Page<SketchRow>> {
  let filter = if own_view {
    "(s.owner_id = lk.user_id OR (s.visibility != 'private' AND \
     s.hidden_at IS NULL AND u.banned_at IS NULL))"
  } else {
    LISTED_SKETCH_SQL
  };
  let rows = sqlx::query_as(&format!(
    "{SELECT} JOIN likes lk ON lk.sketch_id = s.id \
     WHERE lk.user_id = ? AND {filter} \
     ORDER BY lk.created_at DESC, s.id LIMIT ? OFFSET ?"
  ))
  .bind(viewer.id)
  .bind(user_id)
  .bind(PAGE_SIZE + 1)
  .bind(offset(page))
  .fetch_all(db)
  .await?;
  Ok(Page::from_overfetch(rows))
}

/// A user's most recently updated sketches, all visibilities.
pub async fn recent_by_owner(
  db: &SqlitePool,
  viewer: &Viewer,
  owner_id: i64,
  limit: i64,
) -> sqlx::Result<Vec<SketchRow>> {
  sqlx::query_as(&format!(
    "{SELECT} WHERE s.owner_id = ? ORDER BY s.updated_at DESC, s.rowid DESC LIMIT ?"
  ))
  .bind(viewer.id)
  .bind(owner_id)
  .bind(limit)
  .fetch_all(db)
  .await
}

/// The sketches of a playlist the viewer can see, in playlist order.
pub async fn in_playlist(
  db: &SqlitePool,
  viewer: &Viewer,
  playlist_id: &str,
) -> sqlx::Result<Vec<SketchRow>> {
  sqlx::query_as(&format!(
    "{SELECT} JOIN playlist_items pi ON pi.sketch_id = s.id \
     WHERE pi.playlist_id = ? AND {VISIBLE_SKETCH_SQL} \
     ORDER BY pi.position, pi.added_at"
  ))
  .bind(viewer.id)
  .bind(playlist_id)
  .bind(viewer.id)
  .bind(viewer.is_admin)
  .fetch_all(db)
  .await
}

/// Counts of a user's publicly listed sketches and the likes they've
/// received on them, for their profile.
pub async fn listed_counts(
  db: &SqlitePool,
  owner_id: i64,
) -> sqlx::Result<(i64, i64)> {
  sqlx::query_as(&format!(
    "SELECT COUNT(*), COALESCE(SUM(s.like_count), 0) FROM sketches s \
     JOIN users u ON u.id = s.owner_id \
     WHERE s.owner_id = ? AND {LISTED_SKETCH_SQL}"
  ))
  .bind(owner_id)
  .fetch_one(db)
  .await
}
