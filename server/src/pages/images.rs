//! Generated avatars and sketch thumbnails.

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::header::{
  CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE,
};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use super::PageContext;
use crate::auth::Viewer;
use crate::avatar::{avatar_svg, placeholder_thumbnail_svg};
use crate::error::{AppError, PageResult};
use crate::sketches;
use crate::state::AppState;
use crate::validate;
use crate::visibility::Visibility;

/// The Content-Security-Policy of served images: an SVG opened on its own
/// can't run scripts or load anything.
const IMAGE_CSP: &str = "default-src 'none'; style-src 'unsafe-inline'";

const SVG: &str = "image/svg+xml";

fn image(content_type: &str, cache: &str, body: Vec<u8>) -> Response {
  (
    [
      (CONTENT_TYPE, content_type),
      (CACHE_CONTROL, cache),
      (CONTENT_SECURITY_POLICY, IMAGE_CSP),
    ],
    Body::from(body),
  )
    .into_response()
}

/// `GET /u/{username}/avatar.svg`: drawn from the name alone, so it's the
/// same for every request and cacheable.
pub async fn avatar(Path(username): Path<String>) -> PageResult {
  if !validate::is_username_shaped(&username) {
    return Err(AppError::NotFound.into());
  }
  Ok(image(
    SVG,
    "public, max-age=86400",
    avatar_svg(&username).into_bytes(),
  ))
}

#[derive(Deserialize)]
pub struct ThumbQuery {
  v: Option<String>,
}

/// `GET /s/{id}/thumb?v=…`: the uploaded thumbnail, or a generated one.
/// Versioned URLs of shared sketches are cached for good; private ones only
/// by the viewer's browser.
pub async fn thumbnail(
  State(state): State<AppState>,
  page: PageContext,
  Path(id): Path<String>,
  Query(query): Query<ThumbQuery>,
) -> PageResult {
  let viewer = Viewer::of(page.user.as_ref());
  let row = sketches::find_visible(&state.db, &id, &viewer).await?;
  let shared = row.visibility != Visibility::Private && row.hidden_at.is_none();
  let cache = match (query.v.is_some(), shared) {
    (true, true) => "public, max-age=31536000, immutable",
    (true, false) => "private, max-age=31536000, immutable",
    (false, _) => "private, no-cache",
  };
  let response = match sketches::thumbnail(&state.db, &id).await? {
    Some(stored) => image(&stored.mime, cache, stored.data),
    None => image(
      SVG,
      cache,
      placeholder_thumbnail_svg(&row.id, &row.title).into_bytes(),
    ),
  };
  Ok(response)
}
