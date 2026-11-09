//! `/api/playlists`: creating, changing, and arranging playlists.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;

use super::ApiJson;
use crate::auth::{ApiUser, MaybeUser, Viewer};
use crate::error::{ApiResult, AppError};
use crate::playlists::{self, Playlist};
use crate::sketches;
use crate::state::AppState;
use crate::validate;
use crate::visibility::Visibility;

/// The body of `POST /api/playlists`.
#[derive(Deserialize)]
pub struct CreatePlaylist {
  title: String,
  #[serde(default)]
  description: String,
  visibility: Option<Visibility>,
  sketch_id: Option<String>,
}

/// The body of `PUT /api/playlists/{id}`; absent fields stay as they are.
#[derive(Deserialize)]
pub struct UpdatePlaylist {
  title: Option<String>,
  description: Option<String>,
  visibility: Option<Visibility>,
}

#[derive(Deserialize)]
pub struct AddItem {
  sketch_id: String,
}

#[derive(Deserialize)]
pub struct Reorder {
  sketch_ids: Vec<String>,
}

async fn detail(
  state: &AppState,
  id: &str,
  viewer: &Viewer,
) -> Result<Playlist, AppError> {
  let row = playlists::find_visible(&state.db, id, viewer).await?;
  Ok(row.detail(&state.db, viewer).await?)
}

/// `POST /api/playlists`: a new playlist (private unless another
/// visibility is given), optionally holding a first sketch.
pub async fn create(
  State(state): State<AppState>,
  ApiUser(user): ApiUser,
  ApiJson(body): ApiJson<CreatePlaylist>,
) -> ApiResult<(StatusCode, Json<Playlist>)> {
  let viewer = Viewer::of(Some(&user));
  let visibility = body.visibility.unwrap_or(Visibility::Private);
  playlists::check_visibility(&user, visibility)?;
  let title = validate::playlist_title(&body.title)?;
  let description = validate::playlist_description(&body.description)?;
  if let Some(sketch_id) = &body.sketch_id {
    sketches::find_visible(&state.db, sketch_id, &viewer).await?;
  }
  let id =
    playlists::create(&state.db, user.id, &title, &description, visibility)
      .await?;
  if let Some(sketch_id) = &body.sketch_id {
    playlists::add_item(&state.db, &id, sketch_id).await?;
  }
  Ok((
    StatusCode::CREATED,
    Json(detail(&state, &id, &viewer).await?),
  ))
}

/// `GET /api/playlists/{id}`.
pub async fn get(
  State(state): State<AppState>,
  MaybeUser(user): MaybeUser,
  Path(id): Path<String>,
) -> ApiResult<Json<Playlist>> {
  let viewer = Viewer::of(user.as_ref());
  Ok(Json(detail(&state, &id, &viewer).await?))
}

/// `PUT /api/playlists/{id}` (owner only).
pub async fn update(
  State(state): State<AppState>,
  ApiUser(user): ApiUser,
  Path(id): Path<String>,
  ApiJson(body): ApiJson<UpdatePlaylist>,
) -> ApiResult<Json<Playlist>> {
  let viewer = Viewer::of(Some(&user));
  let row = playlists::find_owned(&state.db, &id, &viewer).await?;
  let visibility = body.visibility.unwrap_or(row.visibility);
  if visibility != row.visibility {
    playlists::check_visibility(&user, visibility)?;
  }
  let title = match &body.title {
    Some(title) => validate::playlist_title(title)?,
    None => row.title.clone(),
  };
  let description = match &body.description {
    Some(description) => validate::playlist_description(description)?,
    None => row.description.clone(),
  };
  playlists::update(&state.db, &id, &title, &description, visibility).await?;
  Ok(Json(detail(&state, &id, &viewer).await?))
}

/// `DELETE /api/playlists/{id}` (owner or admin).
pub async fn delete(
  State(state): State<AppState>,
  ApiUser(user): ApiUser,
  Path(id): Path<String>,
) -> ApiResult<StatusCode> {
  let viewer = Viewer::of(Some(&user));
  let row = playlists::find_visible(&state.db, &id, &viewer).await?;
  if !viewer.is(row.owner_id) && !viewer.is_admin {
    return Err(AppError::forbidden(
      "Only the playlist's owner can delete it.",
    ));
  }
  playlists::delete(&state.db, &id).await?;
  Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/playlists/{id}/items` `{sketch_id}` (owner only): appends a
/// sketch the owner can see.
pub async fn add_item(
  State(state): State<AppState>,
  ApiUser(user): ApiUser,
  Path(id): Path<String>,
  ApiJson(body): ApiJson<AddItem>,
) -> ApiResult<Json<Playlist>> {
  let viewer = Viewer::of(Some(&user));
  playlists::find_owned(&state.db, &id, &viewer).await?;
  sketches::find_visible(&state.db, &body.sketch_id, &viewer).await?;
  playlists::add_item(&state.db, &id, &body.sketch_id).await?;
  Ok(Json(detail(&state, &id, &viewer).await?))
}

/// `DELETE /api/playlists/{id}/items/{sketch_id}` (owner only).
pub async fn remove_item(
  State(state): State<AppState>,
  ApiUser(user): ApiUser,
  Path((id, sketch_id)): Path<(String, String)>,
) -> ApiResult<StatusCode> {
  let viewer = Viewer::of(Some(&user));
  playlists::find_owned(&state.db, &id, &viewer).await?;
  if !playlists::remove_item(&state.db, &id, &sketch_id).await? {
    return Err(AppError::NotFound);
  }
  Ok(StatusCode::NO_CONTENT)
}

/// `PUT /api/playlists/{id}/order` `{sketch_ids}` (owner only).
pub async fn reorder(
  State(state): State<AppState>,
  ApiUser(user): ApiUser,
  Path(id): Path<String>,
  ApiJson(body): ApiJson<Reorder>,
) -> ApiResult<Json<Playlist>> {
  let viewer = Viewer::of(Some(&user));
  playlists::find_owned(&state.db, &id, &viewer).await?;
  playlists::reorder(&state.db, &id, &body.sketch_ids).await?;
  Ok(Json(detail(&state, &id, &viewer).await?))
}
