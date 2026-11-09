//! `/api/sketches`: creating, reading, changing, forking, and liking
//! sketches, and uploading their thumbnails.

use axum::Json;
use axum::body::Bytes;
use axum::extract::rejection::BytesRejection;
use axum::extract::{Path, State};
use axum::http::header::CONTENT_TYPE;
use axum::http::{HeaderMap, StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{ApiJson, nullable};
use crate::auth::{ApiUser, CurrentUser, MaybeUser, Viewer};
use crate::error::{ApiResult, AppError};
use crate::ratelimit::Action;
use crate::sketches::{self, NewSketch, Sketch, SketchRow, SketchUpdate};
use crate::state::AppState;
use crate::validate::{self, SketchFile};
use crate::visibility::Visibility;

/// The body of `POST /api/sketches`.
#[derive(Deserialize)]
pub struct CreateSketch {
  title: String,
  #[serde(default)]
  description: String,
  files: Vec<SketchFile>,
  entry_file: Option<String>,
  entry_name: Option<String>,
  visibility: Option<Visibility>,
  forked_from: Option<String>,
}

/// The body of `PUT /api/sketches/{id}`; absent fields stay as they are,
/// and `null` clears the entry fields.
#[derive(Deserialize)]
pub struct UpdateSketch {
  title: Option<String>,
  description: Option<String>,
  files: Option<Vec<SketchFile>>,
  #[serde(default, deserialize_with = "nullable")]
  entry_file: Option<Option<String>>,
  #[serde(default, deserialize_with = "nullable")]
  entry_name: Option<Option<String>>,
  visibility: Option<Visibility>,
}

/// The body of `POST /api/sketches/{id}/fork`, which may be empty.
#[derive(Default, Deserialize)]
pub struct ForkSketch {
  title: Option<String>,
  files: Option<Vec<SketchFile>>,
  #[serde(default, deserialize_with = "nullable")]
  entry_file: Option<Option<String>>,
  #[serde(default, deserialize_with = "nullable")]
  entry_name: Option<Option<String>>,
}

/// The API error for a request body that couldn't be read.
fn body_error(rejection: BytesRejection) -> AppError {
  if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
    AppError::TooLarge("The request body is too large.".into())
  } else {
    AppError::bad_request(rejection.body_text())
  }
}

/// Checks a sketch's files and entry point together.
fn validate_contents(
  files: &[SketchFile],
  entry_file: Option<&str>,
  entry_name: Option<&str>,
) -> Result<(), AppError> {
  validate::sketch_files(files)?;
  validate::entry_file(entry_file, files)?;
  if let Some(name) = entry_name {
    validate::entry_name(name)?;
  }
  Ok(())
}

/// Fails unless the user may give something this visibility: publishing
/// needs a verified email address.
fn check_visibility(
  user: &CurrentUser,
  visibility: Visibility,
) -> Result<(), AppError> {
  if visibility == Visibility::Public {
    user.require_verified("publish sketches")?;
  }
  Ok(())
}

/// A sketch the user owns: 404 when they can't see it, 403 when they can
/// but don't own it.
async fn find_owned(
  state: &AppState,
  id: &str,
  user: &CurrentUser,
) -> Result<SketchRow, AppError> {
  let viewer = Viewer::of(Some(user));
  let row = sketches::find_visible(&state.db, id, &viewer).await?;
  if viewer.is(row.owner_id) {
    Ok(row)
  } else {
    Err(AppError::forbidden(
      "Only the sketch's owner can change it.",
    ))
  }
}

/// A sketch's full API shape after a change.
async fn reload(
  state: &AppState,
  id: &str,
  viewer: &Viewer,
) -> Result<Sketch, AppError> {
  let row = sketches::find(&state.db, id, viewer)
    .await?
    .ok_or(AppError::NotFound)?;
  Ok(row.detail(&state.db, viewer).await?)
}

fn check_create_limit(
  state: &AppState,
  user: &CurrentUser,
) -> Result<(), AppError> {
  state.limiter.check(
    Action::SketchCreate,
    &user.id.to_string(),
    state.config.limits.sketch_creates,
  )
}

/// `POST /api/sketches`: a new sketch owned by the caller (private unless
/// another visibility is given).
pub async fn create(
  State(state): State<AppState>,
  ApiUser(user): ApiUser,
  ApiJson(body): ApiJson<CreateSketch>,
) -> ApiResult<(StatusCode, Json<Sketch>)> {
  let viewer = Viewer::of(Some(&user));
  let visibility = body.visibility.unwrap_or(Visibility::Private);
  check_visibility(&user, visibility)?;
  let sketch = NewSketch {
    title: validate::sketch_title(&body.title)?,
    description: validate::sketch_description(&body.description)?,
    entry_file: body.entry_file,
    entry_name: body.entry_name,
    files: body.files,
    visibility,
    forked_from: body.forked_from,
  };
  validate_contents(
    &sketch.files,
    sketch.entry_file.as_deref(),
    sketch.entry_name.as_deref(),
  )?;
  if let Some(source) = &sketch.forked_from {
    sketches::find_visible(&state.db, source, &viewer)
      .await
      .map_err(|_| {
        AppError::bad_request("The original sketch wasn't found.")
      })?;
  }
  check_create_limit(&state, &user)?;
  let id = sketches::create(&state.db, user.id, &sketch).await?;
  Ok((
    StatusCode::CREATED,
    Json(reload(&state, &id, &viewer).await?),
  ))
}

/// `GET /api/sketches/{id}`.
pub async fn get(
  State(state): State<AppState>,
  MaybeUser(user): MaybeUser,
  Path(id): Path<String>,
) -> ApiResult<Json<Sketch>> {
  let viewer = Viewer::of(user.as_ref());
  let row = sketches::find_visible(&state.db, &id, &viewer).await?;
  Ok(Json(row.detail(&state.db, &viewer).await?))
}

/// `PUT /api/sketches/{id}` (owner only).
pub async fn update(
  State(state): State<AppState>,
  ApiUser(user): ApiUser,
  Path(id): Path<String>,
  ApiJson(body): ApiJson<UpdateSketch>,
) -> ApiResult<Json<Sketch>> {
  let viewer = Viewer::of(Some(&user));
  let row = find_owned(&state, &id, &user).await?;
  let visibility = body.visibility.unwrap_or(row.visibility);
  if visibility != row.visibility {
    check_visibility(&user, visibility)?;
  }
  let update = SketchUpdate {
    title: match &body.title {
      Some(title) => validate::sketch_title(title)?,
      None => row.title.clone(),
    },
    description: match &body.description {
      Some(description) => validate::sketch_description(description)?,
      None => row.description.clone(),
    },
    entry_file: body.entry_file.unwrap_or(row.entry_file.clone()),
    entry_name: body.entry_name.unwrap_or(row.entry_name.clone()),
    files: body.files,
    visibility,
  };
  let files = match &update.files {
    Some(files) => files.clone(),
    None => sketches::files(&state.db, &id).await?,
  };
  validate_contents(
    &files,
    update.entry_file.as_deref(),
    update.entry_name.as_deref(),
  )?;
  sketches::update(&state.db, &id, &update).await?;
  Ok(Json(reload(&state, &id, &viewer).await?))
}

/// `DELETE /api/sketches/{id}` (owner or admin).
pub async fn delete(
  State(state): State<AppState>,
  ApiUser(user): ApiUser,
  Path(id): Path<String>,
) -> ApiResult<StatusCode> {
  let viewer = Viewer::of(Some(&user));
  let row = sketches::find_visible(&state.db, &id, &viewer).await?;
  if !viewer.is(row.owner_id) && !viewer.is_admin {
    return Err(AppError::forbidden(
      "Only the sketch's owner can delete it.",
    ));
  }
  sketches::delete(&state.db, &id).await?;
  Ok(StatusCode::NO_CONTENT)
}

/// The longest a title can be, in characters.
const MAX_TITLE_CHARS: usize = 100;

/// `POST /api/sketches/{id}/fork`: a private copy owned by the caller.
/// Files, entry point, and title default to the original's ("Fork of …").
pub async fn fork(
  State(state): State<AppState>,
  ApiUser(user): ApiUser,
  Path(id): Path<String>,
  body: Result<Bytes, BytesRejection>,
) -> ApiResult<(StatusCode, Json<Sketch>)> {
  let body = body.map_err(body_error)?;
  let body: ForkSketch = if body.iter().all(u8::is_ascii_whitespace) {
    ForkSketch::default()
  } else {
    serde_json::from_slice(&body)
      .map_err(|e| AppError::bad_request(format!("Invalid JSON: {e}")))?
  };
  let viewer = Viewer::of(Some(&user));
  let source = sketches::find_visible(&state.db, &id, &viewer).await?;
  let title = match &body.title {
    Some(title) => validate::sketch_title(title)?,
    None => format!("Fork of {}", source.title)
      .chars()
      .take(MAX_TITLE_CHARS)
      .collect(),
  };
  let files = match body.files {
    Some(files) => files,
    None => sketches::files(&state.db, &id).await?,
  };
  let sketch = NewSketch {
    title,
    description: String::new(),
    entry_file: body.entry_file.unwrap_or(source.entry_file.clone()),
    entry_name: body.entry_name.unwrap_or(source.entry_name.clone()),
    files,
    visibility: Visibility::Private,
    forked_from: Some(source.id.clone()),
  };
  validate_contents(
    &sketch.files,
    sketch.entry_file.as_deref(),
    sketch.entry_name.as_deref(),
  )?;
  check_create_limit(&state, &user)?;
  let new_id = sketches::create(&state.db, user.id, &sketch).await?;
  Ok((
    StatusCode::CREATED,
    Json(reload(&state, &new_id, &viewer).await?),
  ))
}

/// `PUT /api/sketches/{id}/thumbnail` (owner only): the raw image as the
/// body, with its `Content-Type`.
pub async fn put_thumbnail(
  State(state): State<AppState>,
  ApiUser(user): ApiUser,
  Path(id): Path<String>,
  headers: HeaderMap,
  body: Result<Bytes, BytesRejection>,
) -> ApiResult<StatusCode> {
  let body = body.map_err(body_error)?;
  find_owned(&state, &id, &user).await?;
  let content_type = headers.get(CONTENT_TYPE).and_then(|v| v.to_str().ok());
  let mime = validate::thumbnail(content_type, &body)?;
  sketches::set_thumbnail(&state.db, &id, mime, &body).await?;
  Ok(StatusCode::NO_CONTENT)
}

async fn set_liked(
  state: &AppState,
  user: &CurrentUser,
  id: &str,
  liked: bool,
) -> ApiResult<Json<Value>> {
  let viewer = Viewer::of(Some(user));
  sketches::find_visible(&state.db, id, &viewer).await?;
  let like_count = sketches::set_liked(&state.db, user.id, id, liked).await?;
  Ok(Json(json!({ "liked": liked, "like_count": like_count })))
}

/// `POST /api/sketches/{id}/like`: `{liked, like_count}`.
pub async fn like(
  State(state): State<AppState>,
  ApiUser(user): ApiUser,
  Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
  set_liked(&state, &user, &id, true).await
}

/// `DELETE /api/sketches/{id}/like`: `{liked, like_count}`.
pub async fn unlike(
  State(state): State<AppState>,
  ApiUser(user): ApiUser,
  Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
  set_liked(&state, &user, &id, false).await
}
