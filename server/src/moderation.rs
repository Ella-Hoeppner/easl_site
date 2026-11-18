//! Moderation: the actions admins take, each recorded in the moderation
//! log (`moderation_actions`) in the same transaction as the action, and
//! the lists the admin page shows.
//!
//! A log entry keeps what it shows of its target as it was then (the
//! user's username, the sketch's title, an excerpt of a comment), so it
//! stays readable once the target is gone. People acting on their own
//! content (an author deleting their comment, an owner deleting their
//! sketch or a comment on it) aren't moderating and aren't logged, admins
//! included; an action that changes nothing (hiding a hidden sketch) isn't
//! logged either.

use serde::Serialize;
use sqlx::{FromRow, SqliteConnection, SqlitePool};

use crate::comments;
use crate::db::now;
use crate::reports::{self, Report};
use crate::sketches::{self, MAX_PAGE, Page};
use crate::text::excerpt;
use crate::users::{self, User};

/// How many rows a page of the admin page's lists shows.
pub const PAGE_SIZE: i64 = 50;

/// How much of a comment the log keeps, in characters.
const EXCERPT_CHARS: usize = 200;

/// The note on actions taken with the `admin` command.
pub const COMMAND_LINE: &str = "command line";

/// The kinds of moderation action, as `moderation_actions.action` stores
/// them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
  HideSketch,
  UnhideSketch,
  /// Deleting someone else's sketch.
  DeleteSketch,
  /// Deleting a comment on someone else's sketch, by someone else.
  DeleteComment,
  RestoreComment,
  BanUser,
  UnbanUser,
  ResolveReport,
  GrantAdmin,
  RevokeAdmin,
}

impl Action {
  pub fn as_str(self) -> &'static str {
    match self {
      Action::HideSketch => "hide_sketch",
      Action::UnhideSketch => "unhide_sketch",
      Action::DeleteSketch => "delete_sketch",
      Action::DeleteComment => "delete_comment",
      Action::RestoreComment => "restore_comment",
      Action::BanUser => "ban_user",
      Action::UnbanUser => "unban_user",
      Action::ResolveReport => "resolve_report",
      Action::GrantAdmin => "grant_admin",
      Action::RevokeAdmin => "revoke_admin",
    }
  }
}

/// What an action was taken on, as the log records it: its kind
/// (`sketch`, `comment`, `user`, or `report`) and id (a sketch id, comment
/// id, username, or report id), the user it is or belongs to, the sketch it
/// is or is on, and for a comment, an excerpt.
#[derive(Default)]
struct Target {
  kind: &'static str,
  id: String,
  user_id: Option<i64>,
  username: Option<String>,
  sketch_id: Option<String>,
  sketch_title: Option<String>,
  detail: Option<String>,
}

impl Target {
  fn user(user: &User) -> Self {
    Target {
      kind: "user",
      id: user.username.clone(),
      user_id: Some(user.id),
      username: Some(user.username.clone()),
      ..Target::default()
    }
  }

  fn report(report: &Report) -> Self {
    Target {
      kind: "report",
      id: report.id.to_string(),
      user_id: report.user_id,
      username: report.username.clone(),
      sketch_id: report.sketch_id.clone(),
      sketch_title: report.sketch_title.clone(),
      detail: report
        .comment_body
        .as_deref()
        .map(|body| excerpt(body, EXCERPT_CHARS)),
    }
  }
}

/// Writes a log entry.
async fn record(
  conn: &mut SqliteConnection,
  admin_id: Option<i64>,
  action: Action,
  target: &Target,
  note: Option<&str>,
  at: i64,
) -> sqlx::Result<()> {
  sqlx::query(
    "INSERT INTO moderation_actions (admin_id, action, target_kind, target, \
     target_user_id, target_username, sketch_id, sketch_title, detail, \
     note, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
  )
  .bind(admin_id)
  .bind(action.as_str())
  .bind(target.kind)
  .bind(&target.id)
  .bind(target.user_id)
  .bind(&target.username)
  .bind(&target.sketch_id)
  .bind(&target.sketch_title)
  .bind(&target.detail)
  .bind(note)
  .bind(at)
  .execute(conn)
  .await?;
  Ok(())
}

/// What the log records about a sketch.
#[derive(FromRow)]
struct SketchInfo {
  id: String,
  title: String,
  owner_id: i64,
  owner_username: String,
}

impl SketchInfo {
  async fn find(
    conn: &mut SqliteConnection,
    id: &str,
  ) -> sqlx::Result<Option<Self>> {
    sqlx::query_as(
      "SELECT s.id, s.title, s.owner_id, u.username AS owner_username \
       FROM sketches s JOIN users u ON u.id = s.owner_id WHERE s.id = ?",
    )
    .bind(id)
    .fetch_optional(conn)
    .await
  }

  fn target(self) -> Target {
    Target {
      kind: "sketch",
      id: self.id.clone(),
      user_id: Some(self.owner_id),
      username: Some(self.owner_username),
      sketch_id: Some(self.id),
      sketch_title: Some(self.title),
      detail: None,
    }
  }
}

/// What the log records about a comment.
#[derive(FromRow)]
struct CommentInfo {
  id: i64,
  body: String,
  user_id: i64,
  author_username: String,
  sketch_id: String,
  sketch_title: String,
}

impl CommentInfo {
  async fn find(
    conn: &mut SqliteConnection,
    id: i64,
  ) -> sqlx::Result<Option<Self>> {
    sqlx::query_as(
      "SELECT c.id, c.body, c.user_id, u.username AS author_username, \
       c.sketch_id, s.title AS sketch_title FROM comments c \
       JOIN users u ON u.id = c.user_id \
       JOIN sketches s ON s.id = c.sketch_id WHERE c.id = ?",
    )
    .bind(id)
    .fetch_optional(conn)
    .await
  }

  fn target(self) -> Target {
    Target {
      kind: "comment",
      id: self.id.to_string(),
      user_id: Some(self.user_id),
      username: Some(self.author_username),
      sketch_id: Some(self.sketch_id),
      sketch_title: Some(self.sketch_title),
      detail: Some(excerpt(&self.body, EXCERPT_CHARS)),
    }
  }
}

/// Hides or unhides a sketch for an admin, returning whether the sketch
/// exists.
pub async fn set_hidden(
  db: &SqlitePool,
  admin_id: i64,
  sketch_id: &str,
  hidden: bool,
) -> sqlx::Result<bool> {
  let mut tx = db.begin_with("BEGIN IMMEDIATE").await?;
  let Some(sketch) = SketchInfo::find(&mut tx, sketch_id).await? else {
    return Ok(false);
  };
  let at = now();
  if sketches::set_hidden(&mut *tx, sketch_id, hidden, at).await? {
    let action = if hidden {
      Action::HideSketch
    } else {
      Action::UnhideSketch
    };
    record(&mut tx, Some(admin_id), action, &sketch.target(), None, at).await?;
  }
  tx.commit().await?;
  Ok(true)
}

/// Deletes someone else's sketch for an admin.
pub async fn delete_sketch(
  db: &SqlitePool,
  admin_id: i64,
  sketch_id: &str,
) -> sqlx::Result<()> {
  let mut tx = db.begin_with("BEGIN IMMEDIATE").await?;
  let Some(sketch) = SketchInfo::find(&mut tx, sketch_id).await? else {
    return Ok(());
  };
  let target = sketch.target();
  sketches::delete(&mut *tx, sketch_id).await?;
  record(
    &mut tx,
    Some(admin_id),
    Action::DeleteSketch,
    &target,
    None,
    now(),
  )
  .await?;
  tx.commit().await
}

/// Deletes a comment for an admin who is neither its author nor the
/// sketch's owner.
pub async fn delete_comment(
  db: &SqlitePool,
  admin_id: i64,
  comment_id: i64,
) -> sqlx::Result<()> {
  let mut tx = db.begin_with("BEGIN IMMEDIATE").await?;
  let Some(comment) = CommentInfo::find(&mut tx, comment_id).await? else {
    return Ok(());
  };
  let at = now();
  if comments::soft_delete(&mut *tx, comment_id, admin_id, at).await? {
    let target = comment.target();
    record(
      &mut tx,
      Some(admin_id),
      Action::DeleteComment,
      &target,
      None,
      at,
    )
    .await?;
  }
  tx.commit().await
}

/// Restores a deleted comment for an admin, returning whether it was
/// deleted.
pub async fn restore_comment(
  db: &SqlitePool,
  admin_id: i64,
  comment_id: i64,
) -> sqlx::Result<bool> {
  let mut tx = db.begin_with("BEGIN IMMEDIATE").await?;
  let Some(comment) = CommentInfo::find(&mut tx, comment_id).await? else {
    return Ok(false);
  };
  if !comments::restore(&mut *tx, comment_id).await? {
    return Ok(false);
  }
  let target = comment.target();
  record(
    &mut tx,
    Some(admin_id),
    Action::RestoreComment,
    &target,
    None,
    now(),
  )
  .await?;
  tx.commit().await?;
  Ok(true)
}

/// Bans or unbans a user for an admin.
pub async fn set_banned(
  db: &SqlitePool,
  admin_id: i64,
  user: &User,
  banned: bool,
) -> sqlx::Result<()> {
  let mut tx = db.begin_with("BEGIN IMMEDIATE").await?;
  let at = now();
  if users::set_banned(&mut tx, user.id, banned, at).await? {
    let action = if banned {
      Action::BanUser
    } else {
      Action::UnbanUser
    };
    record(
      &mut tx,
      Some(admin_id),
      action,
      &Target::user(user),
      None,
      at,
    )
    .await?;
  }
  tx.commit().await
}

/// Resolves a report for an admin, returning whether it exists.
pub async fn resolve_report(
  db: &SqlitePool,
  admin_id: i64,
  report_id: i64,
) -> sqlx::Result<bool> {
  let mut tx = db.begin_with("BEGIN IMMEDIATE").await?;
  let Some(report) = reports::find(&mut *tx, report_id).await? else {
    return Ok(false);
  };
  let at = now();
  if reports::resolve(&mut *tx, report_id, admin_id, at).await? {
    let target = Target::report(&report);
    record(
      &mut tx,
      Some(admin_id),
      Action::ResolveReport,
      &target,
      None,
      at,
    )
    .await?;
  }
  tx.commit().await?;
  Ok(true)
}

/// Grants or revokes a user's admin rights from the command line (logged
/// with no admin and the note "command line"), returning whether that
/// changed anything, or `None` when there's no such user.
pub async fn set_admin(
  db: &SqlitePool,
  username: &str,
  admin: bool,
) -> sqlx::Result<Option<bool>> {
  let mut tx = db.begin_with("BEGIN IMMEDIATE").await?;
  let Some(user) = users::by_username(&mut *tx, username).await? else {
    return Ok(None);
  };
  if user.is_admin == admin {
    return Ok(Some(false));
  }
  users::set_admin(&mut *tx, &user.username, admin).await?;
  let action = if admin {
    Action::GrantAdmin
  } else {
    Action::RevokeAdmin
  };
  let target = Target::user(&user);
  record(&mut tx, None, action, &target, Some(COMMAND_LINE), now()).await?;
  tx.commit().await?;
  Ok(Some(true))
}

/// The offset of a page of the admin page's lists.
pub fn page_offset(page: i64) -> i64 {
  (page.clamp(1, MAX_PAGE) - 1) * PAGE_SIZE
}

/// How many things each of the admin page's lists holds.
#[derive(Clone, Copy, Debug, Serialize, FromRow)]
pub struct Counts {
  pub open_reports: i64,
  pub resolved_reports: i64,
  pub banned_users: i64,
  pub hidden_sketches: i64,
  pub deleted_comments: i64,
}

pub async fn counts(db: &SqlitePool) -> sqlx::Result<Counts> {
  sqlx::query_as(
    "SELECT \
     (SELECT COUNT(*) FROM reports WHERE resolved_at IS NULL) \
       AS open_reports, \
     (SELECT COUNT(*) FROM reports WHERE resolved_at IS NOT NULL) \
       AS resolved_reports, \
     (SELECT COUNT(*) FROM users WHERE banned_at IS NOT NULL) \
       AS banned_users, \
     (SELECT COUNT(*) FROM sketches WHERE hidden_at IS NOT NULL) \
       AS hidden_sketches, \
     (SELECT COUNT(*) FROM comments WHERE deleted_at IS NOT NULL) \
       AS deleted_comments",
  )
  .fetch_one(db)
  .await
}

/// A banned user, with who banned them (when the log recorded it).
#[derive(Clone, Debug, Serialize, FromRow)]
pub struct BannedUser {
  pub username: String,
  pub display_name: String,
  pub banned_at: i64,
  pub banned_by: Option<String>,
  pub sketch_count: i64,
  pub comment_count: i64,
}

/// A page of banned users, the most recently banned first.
pub async fn banned_users(
  db: &SqlitePool,
  page: i64,
) -> sqlx::Result<Page<BannedUser>> {
  let rows = sqlx::query_as(
    "SELECT u.username, u.display_name, u.banned_at, \
     admin.username AS banned_by, \
     (SELECT COUNT(*) FROM sketches WHERE owner_id = u.id) AS sketch_count, \
     (SELECT COUNT(*) FROM comments \
       WHERE user_id = u.id AND deleted_at IS NULL) AS comment_count \
     FROM users u \
     LEFT JOIN moderation_actions m ON m.id = (SELECT id \
       FROM moderation_actions WHERE target_user_id = u.id \
       AND action = 'ban_user' AND created_at >= u.banned_at \
       ORDER BY created_at DESC, id DESC LIMIT 1) \
     LEFT JOIN users admin ON admin.id = m.admin_id \
     WHERE u.banned_at IS NOT NULL \
     ORDER BY u.banned_at DESC, u.id DESC LIMIT ? OFFSET ?",
  )
  .bind(PAGE_SIZE + 1)
  .bind(page_offset(page))
  .fetch_all(db)
  .await?;
  Ok(Page::split(rows, PAGE_SIZE))
}

/// A hidden sketch, with who hid it (when the log recorded it).
#[derive(Clone, Debug, Serialize, FromRow)]
pub struct HiddenSketch {
  pub id: String,
  pub title: String,
  pub visibility: String,
  pub hidden_at: i64,
  pub hidden_by: Option<String>,
  pub owner_username: String,
  pub owner_banned: bool,
}

/// A page of hidden sketches, the most recently hidden first.
pub async fn hidden_sketches(
  db: &SqlitePool,
  page: i64,
) -> sqlx::Result<Page<HiddenSketch>> {
  let rows = sqlx::query_as(
    "SELECT s.id, s.title, s.visibility, s.hidden_at, \
     admin.username AS hidden_by, u.username AS owner_username, \
     u.banned_at IS NOT NULL AS owner_banned \
     FROM sketches s JOIN users u ON u.id = s.owner_id \
     LEFT JOIN moderation_actions m ON m.id = (SELECT id \
       FROM moderation_actions WHERE target_kind = 'sketch' \
       AND target = s.id AND action = 'hide_sketch' \
       AND created_at >= s.hidden_at \
       ORDER BY created_at DESC, id DESC LIMIT 1) \
     LEFT JOIN users admin ON admin.id = m.admin_id \
     WHERE s.hidden_at IS NOT NULL \
     ORDER BY s.hidden_at DESC, s.rowid DESC LIMIT ? OFFSET ?",
  )
  .bind(PAGE_SIZE + 1)
  .bind(page_offset(page))
  .fetch_all(db)
  .await?;
  Ok(Page::split(rows, PAGE_SIZE))
}

/// A deleted comment. `deleted_role` is who deleted it: `author`, `owner`
/// (the sketch's), `moderator` (`deleted_by` names them), or `unknown`
/// (deleted before deleters were recorded, or by an account since
/// deleted).
#[derive(Clone, Debug, Serialize, FromRow)]
pub struct DeletedComment {
  pub id: i64,
  pub body: String,
  pub created_at: i64,
  pub deleted_at: i64,
  pub deleted_role: String,
  pub deleted_by: Option<String>,
  pub sketch_id: String,
  pub sketch_title: String,
  pub author_username: String,
  pub author_banned: bool,
}

const DELETED_COMMENTS: &str = "SELECT c.id, c.body, c.created_at, \
  c.deleted_at, CASE WHEN deleter.id IS NULL THEN 'unknown' \
    WHEN c.deleted_by = c.user_id THEN 'author' \
    WHEN c.deleted_by = s.owner_id THEN 'owner' \
    ELSE 'moderator' END AS deleted_role, \
  deleter.username AS deleted_by, c.sketch_id, s.title AS sketch_title, \
  author.username AS author_username, \
  author.banned_at IS NOT NULL AS author_banned \
  FROM comments c JOIN users author ON author.id = c.user_id \
  JOIN sketches s ON s.id = c.sketch_id \
  LEFT JOIN users deleter ON deleter.id = c.deleted_by \
  WHERE c.deleted_at IS NOT NULL";

const DELETED_COMMENTS_ORDER: &str =
  "ORDER BY c.deleted_at DESC, c.id DESC LIMIT ? OFFSET ?";

/// A page of deleted comments, the most recently deleted first.
pub async fn deleted_comments(
  db: &SqlitePool,
  page: i64,
) -> sqlx::Result<Page<DeletedComment>> {
  let rows =
    sqlx::query_as(&format!("{DELETED_COMMENTS} {DELETED_COMMENTS_ORDER}"))
      .bind(PAGE_SIZE + 1)
      .bind(page_offset(page))
      .fetch_all(db)
      .await?;
  Ok(Page::split(rows, PAGE_SIZE))
}

/// A user's deleted comments, the most recently deleted first: `size` of
/// them from `offset`.
pub async fn deleted_comments_by(
  db: &SqlitePool,
  user_id: i64,
  size: i64,
  offset: i64,
) -> sqlx::Result<Page<DeletedComment>> {
  let rows = sqlx::query_as(&format!(
    "{DELETED_COMMENTS} AND c.user_id = ? {DELETED_COMMENTS_ORDER}"
  ))
  .bind(user_id)
  .bind(size + 1)
  .bind(offset)
  .fetch_all(db)
  .await?;
  Ok(Page::split(rows, size))
}

/// A log entry, with whether its target user and sketch still exist (for
/// linking to them) and, for a resolved report, the report.
#[derive(Clone, Debug, Serialize, FromRow)]
pub struct LogEntry {
  pub id: i64,
  pub action: String,
  pub target_kind: String,
  pub target: String,
  /// The admin, while their account exists.
  pub admin_username: Option<String>,
  pub target_username: Option<String>,
  pub target_user_exists: bool,
  pub sketch_id: Option<String>,
  pub sketch_title: Option<String>,
  pub sketch_exists: bool,
  pub detail: Option<String>,
  pub note: Option<String>,
  pub report_kind: Option<String>,
  pub report_reason: Option<String>,
  pub created_at: i64,
}

const LOG: &str = "SELECT m.id, m.action, m.target_kind, m.target, \
  admin.username AS admin_username, m.target_username, \
  target_user.id IS NOT NULL AS target_user_exists, m.sketch_id, \
  m.sketch_title, s.id IS NOT NULL AS sketch_exists, m.detail, m.note, \
  r.kind AS report_kind, r.reason AS report_reason, m.created_at \
  FROM moderation_actions m \
  LEFT JOIN users admin ON admin.id = m.admin_id \
  LEFT JOIN users target_user ON target_user.id = m.target_user_id \
  LEFT JOIN sketches s ON s.id = m.sketch_id \
  LEFT JOIN reports r ON m.target_kind = 'report' \
    AND r.id = CAST(m.target AS INTEGER)";

const LOG_ORDER: &str =
  "ORDER BY m.created_at DESC, m.id DESC LIMIT ? OFFSET ?";

/// A page of the log, newest first.
pub async fn log(db: &SqlitePool, page: i64) -> sqlx::Result<Page<LogEntry>> {
  let rows = sqlx::query_as(&format!("{LOG} {LOG_ORDER}"))
    .bind(PAGE_SIZE + 1)
    .bind(page_offset(page))
    .fetch_all(db)
    .await?;
  Ok(Page::split(rows, PAGE_SIZE))
}

/// The log entries about a user or their sketches, comments, or reports
/// against them, newest first: `size` of them from `offset`.
pub async fn log_involving(
  db: &SqlitePool,
  user_id: i64,
  size: i64,
  offset: i64,
) -> sqlx::Result<Page<LogEntry>> {
  let rows =
    sqlx::query_as(&format!("{LOG} WHERE m.target_user_id = ? {LOG_ORDER}"))
      .bind(user_id)
      .bind(size + 1)
      .bind(offset)
      .fetch_all(db)
      .await?;
  Ok(Page::split(rows, size))
}

/// What the moderation page shows about a user beyond their account.
/// The sketch count includes private and hidden ones; the comment count
/// leaves out deleted ones.
#[derive(Clone, Debug, Serialize, FromRow)]
pub struct UserRecord {
  pub sketch_count: i64,
  pub hidden_sketch_count: i64,
  pub private_sketch_count: i64,
  pub comment_count: i64,
  pub deleted_comment_count: i64,
  pub reports_submitted: i64,
  pub reports_against: i64,
  pub log_entries: i64,
  /// Who banned them, when they're banned and the log recorded it.
  pub banned_by: Option<String>,
}

pub async fn user_record(
  db: &SqlitePool,
  user: &User,
) -> sqlx::Result<UserRecord> {
  sqlx::query_as(&format!(
    "SELECT \
     (SELECT COUNT(*) FROM sketches WHERE owner_id = ?1) AS sketch_count, \
     (SELECT COUNT(*) FROM sketches WHERE owner_id = ?1 \
       AND hidden_at IS NOT NULL) AS hidden_sketch_count, \
     (SELECT COUNT(*) FROM sketches WHERE owner_id = ?1 \
       AND visibility = 'private') AS private_sketch_count, \
     (SELECT COUNT(*) FROM comments WHERE user_id = ?1 \
       AND deleted_at IS NULL) AS comment_count, \
     (SELECT COUNT(*) FROM comments WHERE user_id = ?1 \
       AND deleted_at IS NOT NULL) AS deleted_comment_count, \
     (SELECT COUNT(*) FROM reports WHERE reporter_id = ?1) \
       AS reports_submitted, \
     (SELECT COUNT(*) FROM ({}) WHERE user_id = ?1) AS reports_against, \
     (SELECT COUNT(*) FROM moderation_actions WHERE target_user_id = ?1) \
       AS log_entries, \
     (SELECT admin.username FROM moderation_actions m \
       JOIN users admin ON admin.id = m.admin_id \
       WHERE m.target_user_id = ?1 AND m.action = 'ban_user' \
       AND m.created_at >= ?2 \
       ORDER BY m.created_at DESC, m.id DESC LIMIT 1) AS banned_by",
    reports::SELECT
  ))
  .bind(user.id)
  .bind(user.banned_at)
  .fetch_one(db)
  .await
}
