//! Comments on sketches.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

use super::ApiJson;
use crate::auth::{ApiUser, MaybeUser, Viewer};
use crate::comments::{self, Comment};
use crate::db::now;
use crate::error::{ApiResult, AppError};
use crate::moderation;
use crate::ratelimit::Action;
use crate::sketches;
use crate::state::AppState;
use crate::validate;

/// `GET /api/sketches/{id}/comments`: `{"comments": [...]}`, oldest first.
pub async fn list(
  State(state): State<AppState>,
  MaybeUser(user): MaybeUser,
  Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
  let viewer = Viewer::of(user.as_ref());
  sketches::find_visible(&state.db, &id, &viewer).await?;
  let comments = comments::list(&state.db, &id, &viewer).await?;
  Ok(Json(json!({ "comments": comments })))
}

#[derive(Deserialize)]
pub struct NewComment {
  body: String,
}

/// `POST /api/sketches/{id}/comments` `{body}`: the new comment (201).
/// Needs a verified email address.
pub async fn create(
  State(state): State<AppState>,
  ApiUser(user): ApiUser,
  Path(id): Path<String>,
  ApiJson(new): ApiJson<NewComment>,
) -> ApiResult<(StatusCode, Json<Comment>)> {
  user.require_verified("comment")?;
  let viewer = Viewer::of(Some(&user));
  sketches::find_visible(&state.db, &id, &viewer).await?;
  let body = validate::comment_body(&new.body)?;
  state.limiter.check(
    Action::Comment,
    &user.id.to_string(),
    state.config.limits.comments,
  )?;
  let comment = comments::create(&state.db, &id, user.id, &body).await?;
  Ok((StatusCode::CREATED, Json(comment.to_api(&viewer))))
}

/// `DELETE /api/comments/{id}` (author, sketch owner, or admin). An admin
/// deleting someone else's comment on someone else's sketch is moderating,
/// and the moderation log records it.
pub async fn delete(
  State(state): State<AppState>,
  ApiUser(user): ApiUser,
  Path(id): Path<String>,
) -> ApiResult<StatusCode> {
  let viewer = Viewer::of(Some(&user));
  let id: i64 = id.parse().map_err(|_| AppError::NotFound)?;
  let comment = comments::find(&state.db, id)
    .await?
    .filter(|c| c.deleted_at.is_none())
    .ok_or(AppError::NotFound)?;
  sketches::find_visible(&state.db, &comment.sketch_id, &viewer).await?;
  if !comment.deletable_by(&viewer) {
    return Err(AppError::forbidden(
      "Only the comment's author or the sketch's owner can delete it.",
    ));
  }
  if viewer.is(comment.user_id) || viewer.is(comment.sketch_owner_id) {
    comments::soft_delete(&state.db, id, user.id, now()).await?;
  } else {
    moderation::delete_comment(&state.db, user.id, id).await?;
  }
  Ok(StatusCode::NO_CONTENT)
}
