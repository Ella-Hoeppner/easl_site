//! Accounts: the `users` table and the public shapes of a user.

use serde::Serialize;
use sqlx::{FromRow, SqliteConnection, SqliteExecutor, SqlitePool};

use crate::db::{is_unique_violation, now};
use crate::error::AppError;

/// A row of the `users` table.
#[derive(Clone, Debug, FromRow)]
pub struct User {
  pub id: i64,
  pub username: String,
  pub email: String,
  pub password_hash: String,
  /// The display name as the user set it; empty when unset.
  pub display_name: String,
  pub bio: String,
  pub created_at: i64,
  pub email_verified_at: Option<i64>,
  pub is_admin: bool,
  pub banned_at: Option<i64>,
}

impl User {
  /// The name to show: the display name, or the username when it's unset.
  pub fn shown_name(&self) -> &str {
    shown_name(&self.display_name, &self.username)
  }

  pub fn email_verified(&self) -> bool {
    self.email_verified_at.is_some()
  }

  pub fn me(&self) -> Me {
    Me {
      id: self.id,
      username: self.username.clone(),
      display_name: self.shown_name().to_string(),
      avatar_url: avatar_url(&self.username),
      is_admin: self.is_admin,
      email_verified: self.email_verified(),
    }
  }

  pub fn author(&self) -> Author {
    Author::new(&self.username, &self.display_name)
  }

  /// Fails with [`AppError::unverified`] unless the email is verified.
  pub fn require_verified(&self, action: &str) -> Result<(), AppError> {
    if self.email_verified() {
      Ok(())
    } else {
      Err(AppError::unverified(action))
    }
  }
}

/// The display name if set, else the username.
pub fn shown_name<'a>(display_name: &'a str, username: &'a str) -> &'a str {
  if display_name.is_empty() {
    username
  } else {
    display_name
  }
}

/// The URL of a user's generated avatar.
pub fn avatar_url(username: &str) -> String {
  format!("/u/{username}/avatar.svg")
}

/// The signed-in user, as the API describes them.
#[derive(Clone, Debug, Serialize)]
pub struct Me {
  pub id: i64,
  pub username: String,
  pub display_name: String,
  pub avatar_url: String,
  pub is_admin: bool,
  pub email_verified: bool,
}

/// The author of a sketch, comment, or playlist, as the API describes them.
#[derive(Clone, Debug, Serialize)]
pub struct Author {
  pub username: String,
  pub display_name: String,
  pub avatar_url: String,
}

impl Author {
  pub fn new(username: &str, display_name: &str) -> Self {
    Author {
      username: username.to_string(),
      display_name: shown_name(display_name, username).to_string(),
      avatar_url: avatar_url(username),
    }
  }
}

const USER_COLUMNS: &str = "id, username, email, password_hash, display_name, \
  bio, created_at, email_verified_at, is_admin, banned_at";

pub async fn by_id(
  db: impl SqliteExecutor<'_>,
  id: i64,
) -> sqlx::Result<Option<User>> {
  sqlx::query_as(&format!("SELECT {USER_COLUMNS} FROM users WHERE id = ?"))
    .bind(id)
    .fetch_optional(db)
    .await
}

/// The user with a username, case-insensitively.
pub async fn by_username(
  db: impl SqliteExecutor<'_>,
  username: &str,
) -> sqlx::Result<Option<User>> {
  sqlx::query_as(&format!(
    "SELECT {USER_COLUMNS} FROM users WHERE username = ?"
  ))
  .bind(username)
  .fetch_optional(db)
  .await
}

/// The user with an email address, case-insensitively.
pub async fn by_email(
  db: impl SqliteExecutor<'_>,
  email: &str,
) -> sqlx::Result<Option<User>> {
  sqlx::query_as(&format!("SELECT {USER_COLUMNS} FROM users WHERE email = ?"))
    .bind(email)
    .fetch_optional(db)
    .await
}

/// The user a sign-in name refers to: an email address when it contains
/// `@`, else a username.
pub async fn by_login(
  db: &SqlitePool,
  login: &str,
) -> sqlx::Result<Option<User>> {
  let login = login.trim();
  if login.contains('@') {
    by_email(db, login).await
  } else {
    by_username(db, login).await
  }
}

/// Fails with a conflict when the username or email is taken.
pub async fn check_available(
  db: &SqlitePool,
  username: &str,
  email: &str,
) -> Result<(), AppError> {
  if by_username(db, username).await?.is_some() {
    return Err(AppError::conflict("That username is taken."));
  }
  if by_email(db, email).await?.is_some() {
    return Err(AppError::conflict(
      "An account with that email address already exists.",
    ));
  }
  Ok(())
}

/// Creates an account, failing with a conflict when the username or email
/// is taken.
pub async fn create(
  db: &SqlitePool,
  username: &str,
  email: &str,
  password_hash: &str,
) -> Result<User, AppError> {
  check_available(db, username, email).await?;
  let result = sqlx::query_as(&format!(
    "INSERT INTO users (username, email, password_hash, created_at) \
     VALUES (?, ?, ?, ?) RETURNING {USER_COLUMNS}"
  ))
  .bind(username)
  .bind(email)
  .bind(password_hash)
  .bind(now())
  .fetch_one(db)
  .await;
  match result {
    Ok(user) => Ok(user),
    Err(error) if is_unique_violation(&error) => Err(AppError::conflict(
      "That username or email address is taken.",
    )),
    Err(error) => Err(error.into()),
  }
}

pub async fn update_profile(
  db: &SqlitePool,
  id: i64,
  display_name: &str,
  bio: &str,
) -> sqlx::Result<()> {
  sqlx::query("UPDATE users SET display_name = ?, bio = ? WHERE id = ?")
    .bind(display_name)
    .bind(bio)
    .bind(id)
    .execute(db)
    .await?;
  Ok(())
}

/// Changes a user's email address, marking it unverified, and invalidates
/// their outstanding email links (which went to the old address).
pub async fn change_email(
  db: &SqlitePool,
  id: i64,
  email: &str,
) -> Result<(), AppError> {
  if let Some(other) = by_email(db, email).await?
    && other.id != id
  {
    return Err(AppError::conflict(
      "An account with that email address already exists.",
    ));
  }
  let mut tx = db.begin().await?;
  let result = sqlx::query(
    "UPDATE users SET email = ?, email_verified_at = NULL WHERE id = ?",
  )
  .bind(email)
  .bind(id)
  .execute(&mut *tx)
  .await;
  if let Err(error) = result {
    return Err(if is_unique_violation(&error) {
      AppError::conflict("An account with that email address already exists.")
    } else {
      error.into()
    });
  }
  sqlx::query("DELETE FROM email_tokens WHERE user_id = ?")
    .bind(id)
    .execute(&mut *tx)
    .await?;
  tx.commit().await?;
  Ok(())
}

pub async fn set_password_hash(
  db: impl SqliteExecutor<'_>,
  id: i64,
  password_hash: &str,
) -> sqlx::Result<()> {
  sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
    .bind(password_hash)
    .bind(id)
    .execute(db)
    .await?;
  Ok(())
}

pub async fn mark_verified(
  db: impl SqliteExecutor<'_>,
  id: i64,
) -> sqlx::Result<()> {
  sqlx::query(
    "UPDATE users SET email_verified_at = ? \
     WHERE id = ? AND email_verified_at IS NULL",
  )
  .bind(now())
  .bind(id)
  .execute(db)
  .await?;
  Ok(())
}

/// Deletes an account and, through cascades, everything it owns.
pub async fn delete(db: &SqlitePool, id: i64) -> sqlx::Result<()> {
  sqlx::query("DELETE FROM users WHERE id = ?")
    .bind(id)
    .execute(db)
    .await?;
  Ok(())
}

/// Bans a user (as of `at`) or unbans them, returning whether that changed
/// anything: banning a banned user keeps the time they were banned.
/// Banning also ends their sessions.
pub async fn set_banned(
  conn: &mut SqliteConnection,
  id: i64,
  banned: bool,
  at: i64,
) -> sqlx::Result<bool> {
  let result = sqlx::query(
    "UPDATE users SET banned_at = ? WHERE id = ? AND (banned_at IS NULL) = ?",
  )
  .bind(banned.then_some(at))
  .bind(id)
  .bind(banned)
  .execute(&mut *conn)
  .await?;
  if banned {
    sqlx::query("DELETE FROM sessions WHERE user_id = ?")
      .bind(id)
      .execute(&mut *conn)
      .await?;
  }
  Ok(result.rows_affected() > 0)
}

/// Grants or revokes admin rights, returning whether the user exists.
pub async fn set_admin(
  db: impl SqliteExecutor<'_>,
  username: &str,
  admin: bool,
) -> sqlx::Result<bool> {
  let result = sqlx::query("UPDATE users SET is_admin = ? WHERE username = ?")
    .bind(admin)
    .bind(username)
    .execute(db)
    .await?;
  Ok(result.rows_affected() > 0)
}

pub async fn count(db: &SqlitePool) -> sqlx::Result<i64> {
  sqlx::query_scalar("SELECT COUNT(*) FROM users")
    .fetch_one(db)
    .await
}
