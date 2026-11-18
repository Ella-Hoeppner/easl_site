//! The JSON API under `/api`. Requests and responses are JSON; errors are
//! `{"error": "<message>"}` with a matching status.

mod admin;
mod comments;
mod me;
mod playlists;
mod reports;
mod sketches;

use axum::Router;
use axum::extract::FromRequest;
use axum::routing::{delete, get, post, put};
use serde::{Deserialize, Deserializer};

use crate::error::AppError;
use crate::state::AppState;

/// A JSON request body whose rejections are API errors.
#[derive(FromRequest)]
#[from_request(via(axum::Json), rejection(AppError))]
pub struct ApiJson<T>(pub T);

/// Deserializes a field that may be absent (`None`), `null`
/// (`Some(None)`), or a value (`Some(Some(v))`), for updates where `null`
/// clears a field and absence leaves it alone. Use with
/// `#[serde(default, deserialize_with = "nullable")]`.
pub fn nullable<'de, T, D>(
  deserializer: D,
) -> Result<Option<Option<T>>, D::Error>
where
  T: Deserialize<'de>,
  D: Deserializer<'de>,
{
  Option::<T>::deserialize(deserializer).map(Some)
}

/// The API's routes.
pub fn router() -> Router<AppState> {
  Router::new()
    .route("/api/me", get(me::me))
    .route("/api/me/sketches", get(me::sketches))
    .route("/api/me/playlists", get(me::playlists))
    .route("/api/sketches", post(sketches::create))
    .route(
      "/api/sketches/{id}",
      get(sketches::get)
        .put(sketches::update)
        .delete(sketches::delete),
    )
    .route("/api/sketches/{id}/fork", post(sketches::fork))
    .route("/api/sketches/{id}/publish", post(sketches::publish))
    .route(
      "/api/sketches/{id}/discard-draft",
      post(sketches::discard_draft),
    )
    .route("/api/sketches/{id}/published", get(sketches::published))
    .route("/api/sketches/{id}/revisions", get(sketches::revisions))
    .route("/api/sketches/{id}/thumbnail", put(sketches::put_thumbnail))
    .route(
      "/api/sketches/{id}/like",
      post(sketches::like).delete(sketches::unlike),
    )
    .route(
      "/api/sketches/{id}/comments",
      get(comments::list).post(comments::create),
    )
    .route("/api/comments/{id}", delete(comments::delete))
    .route("/api/playlists", post(playlists::create))
    .route(
      "/api/playlists/{id}",
      get(playlists::get)
        .put(playlists::update)
        .delete(playlists::delete),
    )
    .route("/api/playlists/{id}/items", post(playlists::add_item))
    .route(
      "/api/playlists/{id}/items/{sketch_id}",
      delete(playlists::remove_item),
    )
    .route("/api/playlists/{id}/order", put(playlists::reorder))
    .route("/api/reports", post(reports::create))
    .route("/api/admin/sketches/{id}/hide", post(admin::hide_sketch))
    .route(
      "/api/admin/sketches/{id}/unhide",
      post(admin::unhide_sketch),
    )
    .route("/api/admin/users/{username}/ban", post(admin::ban_user))
    .route("/api/admin/users/{username}/unban", post(admin::unban_user))
    .route(
      "/api/admin/reports/{id}/resolve",
      post(admin::resolve_report),
    )
    .route(
      "/api/admin/comments/{id}/restore",
      post(admin::restore_comment),
    )
    .route("/api", get(not_found))
    .route("/api/{*rest}", get(not_found).fallback(not_found))
}

/// Unknown API paths are JSON 404s, not the site's HTML 404 page.
async fn not_found() -> AppError {
  AppError::NotFound
}
