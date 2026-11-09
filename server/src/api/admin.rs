//! `/api/admin`: moderation actions, for admins only.

use axum::extract::{Path, State};
use axum::http::StatusCode;

use crate::auth::{ApiUser, CurrentUser};
use crate::error::{ApiResult, AppError};
use crate::reports;
use crate::sketches;
use crate::state::AppState;
use crate::users;

fn require_admin(user: &CurrentUser) -> Result<(), AppError> {
  if user.is_admin {
    Ok(())
  } else {
    Err(AppError::forbidden("Only admins can do that."))
  }
}

async fn set_hidden(
  state: &AppState,
  user: &CurrentUser,
  id: &str,
  hidden: bool,
) -> ApiResult<StatusCode> {
  require_admin(user)?;
  if !sketches::set_hidden(&state.db, id, hidden).await? {
    return Err(AppError::NotFound);
  }
  Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/admin/sketches/{id}/hide`.
pub async fn hide_sketch(
  State(state): State<AppState>,
  ApiUser(user): ApiUser,
  Path(id): Path<String>,
) -> ApiResult<StatusCode> {
  set_hidden(&state, &user, &id, true).await
}

/// `POST /api/admin/sketches/{id}/unhide`.
pub async fn unhide_sketch(
  State(state): State<AppState>,
  ApiUser(user): ApiUser,
  Path(id): Path<String>,
) -> ApiResult<StatusCode> {
  set_hidden(&state, &user, &id, false).await
}

async fn set_banned(
  state: &AppState,
  user: &CurrentUser,
  username: &str,
  banned: bool,
) -> ApiResult<StatusCode> {
  require_admin(user)?;
  let target = users::by_username(&state.db, username)
    .await?
    .ok_or(AppError::NotFound)?;
  if banned && (target.is_admin || target.id == user.id) {
    return Err(AppError::bad_request(
      "Admins can't be banned; revoke their admin rights first.",
    ));
  }
  users::set_banned(&state.db, target.id, banned).await?;
  Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/admin/users/{username}/ban`: the user can't sign in, their
/// sessions end, and their content leaves listings.
pub async fn ban_user(
  State(state): State<AppState>,
  ApiUser(user): ApiUser,
  Path(username): Path<String>,
) -> ApiResult<StatusCode> {
  set_banned(&state, &user, &username, true).await
}

/// `POST /api/admin/users/{username}/unban`.
pub async fn unban_user(
  State(state): State<AppState>,
  ApiUser(user): ApiUser,
  Path(username): Path<String>,
) -> ApiResult<StatusCode> {
  set_banned(&state, &user, &username, false).await
}

/// `POST /api/admin/reports/{id}/resolve`.
pub async fn resolve_report(
  State(state): State<AppState>,
  ApiUser(user): ApiUser,
  Path(id): Path<String>,
) -> ApiResult<StatusCode> {
  require_admin(&user)?;
  let id: i64 = id.parse().map_err(|_| AppError::NotFound)?;
  if !reports::resolve(&state.db, id, user.id).await? {
    return Err(AppError::NotFound);
  }
  Ok(StatusCode::NO_CONTENT)
}
