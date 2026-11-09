//! Authentication: password hashing, sessions, email tokens, and the
//! extractors handlers use to learn who is signed in.
//!
//! [`session_layer`] runs in front of every dynamic route. It reads the
//! session cookie, loads the signed-in user (banned users have no
//! sessions), slides the session's expiry forward at most once a day, and
//! stores a [`Session`] in the request's extensions for the extractors:
//! [`MaybeUser`], [`ApiUser`] (401 when signed out), and [`PageUser`]
//! (redirect to sign in when signed out).

use std::ops::Deref;

use anyhow::anyhow;
use argon2::Argon2;
use argon2::password_hash::{
  PasswordHash, PasswordHasher, PasswordVerifier, SaltString,
};
use axum::extract::{FromRequestParts, Request, State};
use axum::http::header::{COOKIE, SET_COOKIE};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;
use rand::RngCore;
use sqlx::{SqliteExecutor, SqlitePool};
use tokio::task;

use crate::config::Config;
use crate::db::now;
use crate::error::{AppError, PageError};
use crate::state::AppState;
use crate::tokens::{hash_token, is_token_shaped, new_token};
use crate::users::{self, User};

/// The session cookie's name.
pub const SESSION_COOKIE: &str = "easl_session";

/// How long a session lasts without use.
const SESSION_SECONDS: i64 = 30 * 24 * 60 * 60;

/// How often a session's expiry is pushed forward.
const SESSION_REFRESH_SECONDS: i64 = 24 * 60 * 60;

/// How long an email verification link works.
pub const VERIFY_TOKEN_SECONDS: i64 = 48 * 60 * 60;

/// How long a password reset link works.
pub const RESET_TOKEN_SECONDS: i64 = 60 * 60;

/// Hashes a password with argon2id's default parameters.
pub fn hash_password_blocking(password: &str) -> anyhow::Result<String> {
  let mut salt_bytes = [0u8; 16];
  rand::rng().fill_bytes(&mut salt_bytes);
  let salt = SaltString::encode_b64(&salt_bytes)
    .map_err(|e| anyhow!("encoding a salt: {e}"))?;
  Argon2::default()
    .hash_password(password.as_bytes(), &salt)
    .map(|hash| hash.to_string())
    .map_err(|e| anyhow!("hashing a password: {e}"))
}

/// Hashes a password on the blocking pool, a few at a time (each hash
/// takes ~19 MB).
pub async fn hash_password(
  state: &AppState,
  password: &str,
) -> Result<String, AppError> {
  let _permit = state
    .hashing
    .acquire()
    .await
    .map_err(|e| AppError::Internal(e.into()))?;
  let password = password.to_string();
  task::spawn_blocking(move || hash_password_blocking(&password))
    .await
    .map_err(|e| AppError::Internal(e.into()))?
    .map_err(AppError::Internal)
}

/// Whether `password` matches `hash`. With no hash, checks against a dummy
/// one and returns false, so that a sign-in naming no account takes as long
/// to reject as a wrong password.
pub async fn verify_password(
  state: &AppState,
  hash: Option<&str>,
  password: &str,
) -> Result<bool, AppError> {
  let _permit = state
    .hashing
    .acquire()
    .await
    .map_err(|e| AppError::Internal(e.into()))?;
  let known = hash.is_some();
  let hash = hash.unwrap_or(&state.dummy_password_hash).to_string();
  let password = password.to_string();
  let matches = task::spawn_blocking(move || {
    PasswordHash::new(&hash).is_ok_and(|parsed| {
      Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
    })
  })
  .await
  .map_err(|e| AppError::Internal(e.into()))?;
  Ok(known && matches)
}

/// Starts a session for a user, returning the cookie token.
pub async fn create_session(
  db: impl SqliteExecutor<'_>,
  user_id: i64,
) -> sqlx::Result<String> {
  let token = new_token();
  let now = now();
  sqlx::query(
    "INSERT INTO sessions (token_hash, user_id, created_at, expires_at, \
     last_seen_at) VALUES (?, ?, ?, ?, ?)",
  )
  .bind(hash_token(&token))
  .bind(user_id)
  .bind(now)
  .bind(now + SESSION_SECONDS)
  .bind(now)
  .execute(db)
  .await?;
  Ok(token)
}

/// Ends one session.
pub async fn delete_session(
  db: &SqlitePool,
  token_hash: &str,
) -> sqlx::Result<()> {
  sqlx::query("DELETE FROM sessions WHERE token_hash = ?")
    .bind(token_hash)
    .execute(db)
    .await?;
  Ok(())
}

/// Ends all of a user's sessions except `keep` (a token hash).
pub async fn delete_other_sessions(
  db: impl SqliteExecutor<'_>,
  user_id: i64,
  keep: Option<&str>,
) -> sqlx::Result<()> {
  sqlx::query("DELETE FROM sessions WHERE user_id = ? AND token_hash IS NOT ?")
    .bind(user_id)
    .bind(keep)
    .execute(db)
    .await?;
  Ok(())
}

/// Deletes expired sessions and email tokens.
pub async fn delete_expired(db: &SqlitePool) -> sqlx::Result<()> {
  let now = now();
  sqlx::query("DELETE FROM sessions WHERE expires_at <= ?")
    .bind(now)
    .execute(db)
    .await?;
  sqlx::query("DELETE FROM email_tokens WHERE expires_at <= ?")
    .bind(now)
    .execute(db)
    .await?;
  Ok(())
}

/// The `Set-Cookie` value that stores a session token.
pub fn session_cookie(config: &Config, token: &str) -> HeaderValue {
  cookie_header(config, token, SESSION_SECONDS)
}

/// The `Set-Cookie` value that removes the session cookie.
pub fn clear_session_cookie(config: &Config) -> HeaderValue {
  cookie_header(config, "", 0)
}

fn cookie_header(config: &Config, value: &str, max_age: i64) -> HeaderValue {
  let secure = if config.secure_cookies() {
    "; Secure"
  } else {
    ""
  };
  let cookie = format!(
    "{SESSION_COOKIE}={value}; Path=/; Max-Age={max_age}; HttpOnly; \
     SameSite=Lax{secure}"
  );
  // The value is a base64url token or empty, so the header is always valid.
  HeaderValue::from_str(&cookie).unwrap_or(HeaderValue::from_static(""))
}

/// The session token from a request's cookies, if there is one.
fn session_token(headers: &HeaderMap) -> Option<&str> {
  headers
    .get_all(COOKIE)
    .iter()
    .filter_map(|value| value.to_str().ok())
    .flat_map(|cookies| cookies.split(';'))
    .filter_map(|cookie| cookie.trim().split_once('='))
    .find(|(name, _)| *name == SESSION_COOKIE)
    .map(|(_, value)| value)
}

/// A signed-in user and the session they're signed in with.
#[derive(Clone, Debug)]
pub struct CurrentUser {
  pub user: User,
  pub session_hash: String,
}

impl Deref for CurrentUser {
  type Target = User;

  fn deref(&self) -> &User {
    &self.user
  }
}

/// Who is signed in for this request, as found by [`session_layer`].
#[derive(Clone, Debug, Default)]
pub struct Session(pub Option<CurrentUser>);

/// The outcome of looking up a session cookie.
enum Lookup {
  /// A live session, and whether its expiry was just pushed forward.
  Valid {
    user: Box<CurrentUser>,
    refreshed: bool,
  },
  Invalid,
}

async fn lookup_session(db: &SqlitePool, token: &str) -> sqlx::Result<Lookup> {
  if !is_token_shaped(token) {
    return Ok(Lookup::Invalid);
  }
  let token_hash = hash_token(token);
  let now = now();
  let row: Option<(i64, i64)> = sqlx::query_as(
    "SELECT s.user_id, s.last_seen_at FROM sessions s \
     JOIN users u ON u.id = s.user_id \
     WHERE s.token_hash = ? AND s.expires_at > ? AND u.banned_at IS NULL",
  )
  .bind(&token_hash)
  .bind(now)
  .fetch_optional(db)
  .await?;
  let Some((user_id, last_seen_at)) = row else {
    return Ok(Lookup::Invalid);
  };
  let Some(user) = users::by_id(db, user_id).await? else {
    return Ok(Lookup::Invalid);
  };
  let refreshed = now - last_seen_at >= SESSION_REFRESH_SECONDS;
  if refreshed {
    sqlx::query(
      "UPDATE sessions SET last_seen_at = ?, expires_at = ? \
       WHERE token_hash = ?",
    )
    .bind(now)
    .bind(now + SESSION_SECONDS)
    .bind(&token_hash)
    .execute(db)
    .await?;
  }
  Ok(Lookup::Valid {
    user: Box::new(CurrentUser {
      user,
      session_hash: token_hash,
    }),
    refreshed,
  })
}

/// Whether a response already sets the session cookie (signing in or out).
fn sets_session_cookie(response: &Response) -> bool {
  response.headers().get_all(SET_COOKIE).iter().any(|value| {
    value
      .to_str()
      .is_ok_and(|v| v.starts_with(&format!("{SESSION_COOKIE}=")))
  })
}

/// Middleware that resolves the session; see the module documentation. A
/// cookie naming no live session is cleared.
pub async fn session_layer(
  State(state): State<AppState>,
  mut request: Request,
  next: Next,
) -> Response {
  let token = session_token(request.headers()).map(str::to_string);
  let mut cookie = None;
  let mut session = Session(None);
  if let Some(token) = token {
    match lookup_session(&state.db, &token).await {
      Ok(Lookup::Valid { user, refreshed }) => {
        if refreshed {
          cookie = Some(session_cookie(&state.config, &token));
        }
        session = Session(Some(*user));
      }
      Ok(Lookup::Invalid) => {
        cookie = Some(clear_session_cookie(&state.config));
      }
      Err(error) => tracing::error!("looking up a session: {error}"),
    }
  }
  request.extensions_mut().insert(session);
  let mut response = next.run(request).await;
  if let Some(cookie) = cookie
    && !sets_session_cookie(&response)
  {
    response.headers_mut().append(SET_COOKIE, cookie);
  }
  response
}

fn session_of(parts: &Parts) -> Option<CurrentUser> {
  parts
    .extensions
    .get::<Session>()
    .and_then(|session| session.0.clone())
}

/// The signed-in user, if any.
pub struct MaybeUser(pub Option<CurrentUser>);

impl<S: Send + Sync> FromRequestParts<S> for MaybeUser {
  type Rejection = AppError;

  async fn from_request_parts(
    parts: &mut Parts,
    _state: &S,
  ) -> Result<Self, Self::Rejection> {
    Ok(MaybeUser(session_of(parts)))
  }
}

/// The signed-in user; API requests without one fail with 401.
pub struct ApiUser(pub CurrentUser);

impl<S: Send + Sync> FromRequestParts<S> for ApiUser {
  type Rejection = AppError;

  async fn from_request_parts(
    parts: &mut Parts,
    _state: &S,
  ) -> Result<Self, Self::Rejection> {
    session_of(parts).map(ApiUser).ok_or(AppError::Unauthorized)
  }
}

/// The signed-in user; page requests without one are sent to sign in.
pub struct PageUser(pub CurrentUser);

impl<S: Send + Sync> FromRequestParts<S> for PageUser {
  type Rejection = PageError;

  async fn from_request_parts(
    parts: &mut Parts,
    _state: &S,
  ) -> Result<Self, Self::Rejection> {
    session_of(parts)
      .map(PageUser)
      .ok_or(PageError(AppError::Unauthorized))
  }
}

/// Who is looking at something, for visibility decisions.
#[derive(Clone, Copy, Debug, Default)]
pub struct Viewer {
  pub id: Option<i64>,
  pub is_admin: bool,
}

impl Viewer {
  pub fn of(user: Option<&CurrentUser>) -> Self {
    Viewer {
      id: user.map(|u| u.id),
      is_admin: user.is_some_and(|u| u.is_admin),
    }
  }

  pub fn is(&self, user_id: i64) -> bool {
    self.id == Some(user_id)
  }
}

/// What an email link is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenPurpose {
  Verify,
  Reset,
}

impl TokenPurpose {
  fn as_str(self) -> &'static str {
    match self {
      TokenPurpose::Verify => "verify",
      TokenPurpose::Reset => "reset",
    }
  }

  fn lifetime(self) -> i64 {
    match self {
      TokenPurpose::Verify => VERIFY_TOKEN_SECONDS,
      TokenPurpose::Reset => RESET_TOKEN_SECONDS,
    }
  }
}

/// Creates an email link token for a user, returning the token.
pub async fn create_email_token(
  db: &SqlitePool,
  user_id: i64,
  purpose: TokenPurpose,
) -> sqlx::Result<String> {
  let token = new_token();
  sqlx::query(
    "INSERT INTO email_tokens (token_hash, user_id, purpose, expires_at) \
     VALUES (?, ?, ?, ?)",
  )
  .bind(hash_token(&token))
  .bind(user_id)
  .bind(purpose.as_str())
  .bind(now() + purpose.lifetime())
  .execute(db)
  .await?;
  Ok(token)
}

/// Deletes a user's email tokens for a purpose.
pub async fn delete_email_tokens(
  db: impl SqliteExecutor<'_>,
  user_id: i64,
  purpose: TokenPurpose,
) -> sqlx::Result<()> {
  sqlx::query("DELETE FROM email_tokens WHERE user_id = ? AND purpose = ?")
    .bind(user_id)
    .bind(purpose.as_str())
    .execute(db)
    .await?;
  Ok(())
}

/// The user an unused, unexpired email token belongs to, without using it.
pub async fn peek_email_token(
  db: &SqlitePool,
  token: &str,
  purpose: TokenPurpose,
) -> sqlx::Result<Option<i64>> {
  if !is_token_shaped(token) {
    return Ok(None);
  }
  sqlx::query_scalar(
    "SELECT user_id FROM email_tokens WHERE token_hash = ? AND purpose = ? \
     AND used_at IS NULL AND expires_at > ?",
  )
  .bind(hash_token(token))
  .bind(purpose.as_str())
  .bind(now())
  .fetch_optional(db)
  .await
}

/// Marks an unused, unexpired email token used, returning its user. Each
/// token works once, even under concurrent requests.
pub async fn consume_email_token(
  db: impl SqliteExecutor<'_>,
  token: &str,
  purpose: TokenPurpose,
) -> sqlx::Result<Option<i64>> {
  if !is_token_shaped(token) {
    return Ok(None);
  }
  let now = now();
  sqlx::query_scalar(
    "UPDATE email_tokens SET used_at = ? WHERE token_hash = ? AND \
     purpose = ? AND used_at IS NULL AND expires_at > ? RETURNING user_id",
  )
  .bind(now)
  .bind(hash_token(token))
  .bind(purpose.as_str())
  .bind(now)
  .fetch_optional(db)
  .await
}
