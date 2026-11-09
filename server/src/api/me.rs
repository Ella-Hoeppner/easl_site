//! `/api/me`: the signed-in user and their things.

use axum::Json;
use axum::extract::{Query, State};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::auth::{ApiUser, MaybeUser, Viewer};
use crate::error::ApiResult;
use crate::playlists;
use crate::sketches;
use crate::state::AppState;
use crate::visibility::Visibility;

/// `GET /api/me`: `{"user": Me | null}`.
pub async fn me(MaybeUser(user): MaybeUser) -> Json<Value> {
  Json(json!({ "user": user.map(|u| u.me()) }))
}

#[derive(Deserialize)]
pub struct SketchesQuery {
  limit: Option<i64>,
}

/// One of the user's sketches, for the editor's sketch menu.
#[derive(Serialize)]
struct OwnSketch {
  id: String,
  title: String,
  visibility: Visibility,
  updated_at: i64,
  thumbnail_url: String,
}

/// `GET /api/me/sketches?limit=N`: the user's sketches, most recently
/// updated first (default 50, at most 200).
pub async fn sketches(
  State(state): State<AppState>,
  ApiUser(user): ApiUser,
  Query(query): Query<SketchesQuery>,
) -> ApiResult<Json<Value>> {
  let limit = query.limit.unwrap_or(50).clamp(1, 200);
  let viewer = Viewer::of(Some(&user));
  let rows =
    sketches::recent_by_owner(&state.db, &viewer, user.id, limit).await?;
  let sketches: Vec<_> = rows
    .iter()
    .map(|row| OwnSketch {
      id: row.id.clone(),
      title: row.title.clone(),
      visibility: row.visibility,
      updated_at: row.updated_at,
      thumbnail_url: row.thumbnail_url(),
    })
    .collect();
  Ok(Json(json!({ "sketches": sketches })))
}

#[derive(Deserialize)]
pub struct PlaylistsQuery {
  sketch: Option<String>,
}

/// `GET /api/me/playlists?sketch=<id>`: the user's playlists, each saying
/// whether it holds the sketch.
pub async fn playlists(
  State(state): State<AppState>,
  ApiUser(user): ApiUser,
  Query(query): Query<PlaylistsQuery>,
) -> ApiResult<Json<Value>> {
  let playlists =
    playlists::own_with_membership(&state.db, user.id, query.sketch.as_deref())
      .await?;
  Ok(Json(json!({ "playlists": playlists })))
}
