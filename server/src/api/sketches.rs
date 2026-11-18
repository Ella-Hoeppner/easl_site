//! `/api/sketches`: creating, reading, changing, publishing, forking, and
//! liking sketches, their revisions, and uploading their thumbnails.

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
use crate::moderation;
use crate::ratelimit::Action;
use crate::revisions::{self, Contents};
use crate::sketches::{
  self, NewSketch, Published, Sketch, SketchRow, SketchUpdate,
};
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
  #[serde(default)]
  tags: Vec<String>,
}

/// The body of `PUT /api/sketches/{id}`; absent fields stay as they are,
/// and `null` clears the entry fields. Files or entry fields save a new
/// head revision (when they change it); `tags` replaces the tags.
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
  tags: Option<Vec<String>>,
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
fn validate_contents(contents: &Contents) -> Result<(), AppError> {
  validate::sketch_files(&contents.files)?;
  validate::entry_file(contents.entry_file.as_deref(), &contents.files)?;
  if let Some(name) = &contents.entry_name {
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
    contents: Contents {
      files: body.files,
      entry_file: body.entry_file,
      entry_name: body.entry_name,
    },
    visibility,
    forked_from: body.forked_from,
    tags: validate::tags(&body.tags)?,
  };
  validate_contents(&sketch.contents)?;
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

/// `GET /api/sketches/{id}`: the head for the owner, the published
/// revision for anyone else.
pub async fn get(
  State(state): State<AppState>,
  MaybeUser(user): MaybeUser,
  Path(id): Path<String>,
) -> ApiResult<Json<Sketch>> {
  let viewer = Viewer::of(user.as_ref());
  let row = sketches::find_visible(&state.db, &id, &viewer).await?;
  Ok(Json(row.detail(&state.db, &viewer).await?))
}

/// `PUT /api/sketches/{id}` (owner only). Changing the visibility from
/// private publishes the head; making the sketch private unpublishes it.
/// Tags apply at once, like the title and description.
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
  let saves = body.files.is_some()
    || body.entry_file.is_some()
    || body.entry_name.is_some();
  let contents = if saves {
    let head = row.head_contents(&state.db).await?;
    let contents = Contents {
      files: body.files.unwrap_or(head.files),
      entry_file: body.entry_file.unwrap_or(head.entry_file),
      entry_name: body.entry_name.unwrap_or(head.entry_name),
    };
    validate_contents(&contents)?;
    Some(contents)
  } else {
    None
  };
  let update = SketchUpdate {
    title: match &body.title {
      Some(title) => validate::sketch_title(title)?,
      None => row.title.clone(),
    },
    description: match &body.description {
      Some(description) => validate::sketch_description(description)?,
      None => row.description.clone(),
    },
    contents,
    visibility,
    publish: false,
    tags: body.tags.as_deref().map(validate::tags).transpose()?,
  };
  sketches::update(&state.db, &id, &update).await?;
  Ok(Json(reload(&state, &id, &viewer).await?))
}

/// `DELETE /api/sketches/{id}` (owner or admin; the moderation log records
/// an admin deleting someone else's sketch).
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
  if viewer.is(row.owner_id) {
    sketches::delete(&state.db, &id).await?;
  } else {
    moderation::delete_sketch(&state.db, user.id, &id).await?;
  }
  Ok(StatusCode::NO_CONTENT)
}

/// The longest a title can be, in characters.
const MAX_TITLE_CHARS: usize = 100;

/// `POST /api/sketches/{id}/fork`: a private copy owned by the caller.
/// Files and entry point default to those of the revision the caller sees
/// (the head of their own sketch, else the published one), and the title
/// to "Fork of …". The fork has the original's tags.
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
    None => {
      let mut conn = state.db.acquire().await?;
      revisions::files(&mut conn, &id, source.shown_revision(&viewer)).await?
    }
  };
  let (entry_file, entry_name) = source.shown_entry(&viewer);
  let sketch = NewSketch {
    title,
    description: String::new(),
    contents: Contents {
      files,
      entry_file: body.entry_file.unwrap_or(entry_file),
      entry_name: body.entry_name.unwrap_or(entry_name),
    },
    visibility: Visibility::Private,
    forked_from: Some(source.id.clone()),
    tags: source.tags(),
  };
  validate_contents(&sketch.contents)?;
  check_create_limit(&state, &user)?;
  let new_id = sketches::create(&state.db, user.id, &sketch).await?;
  Ok((
    StatusCode::CREATED,
    Json(reload(&state, &new_id, &viewer).await?),
  ))
}

/// `POST /api/sketches/{id}/publish` (owner only): the head becomes the
/// published revision. A private sketch can't be published.
pub async fn publish(
  State(state): State<AppState>,
  ApiUser(user): ApiUser,
  Path(id): Path<String>,
) -> ApiResult<Json<Sketch>> {
  let viewer = Viewer::of(Some(&user));
  let row = find_owned(&state, &id, &user).await?;
  if row.visibility == Visibility::Private {
    return Err(AppError::bad_request(
      "Make it unlisted or public to publish it.",
    ));
  }
  sketches::publish(&state.db, &id).await?;
  Ok(Json(reload(&state, &id, &viewer).await?))
}

/// `POST /api/sketches/{id}/discard-draft` (owner only): the head goes back
/// to the published revision. The draft's revisions stay in the history.
pub async fn discard_draft(
  State(state): State<AppState>,
  ApiUser(user): ApiUser,
  Path(id): Path<String>,
) -> ApiResult<Json<Sketch>> {
  let viewer = Viewer::of(Some(&user));
  let row = find_owned(&state, &id, &user).await?;
  if !row.has_draft() {
    return Err(AppError::bad_request("There's no draft to discard."));
  }
  sketches::discard_draft(&state.db, &id).await?;
  Ok(Json(reload(&state, &id, &viewer).await?))
}

/// `GET /api/sketches/{id}/published`: the published revision, for anyone
/// who can see the sketch; 404 when it has none.
pub async fn published(
  State(state): State<AppState>,
  MaybeUser(user): MaybeUser,
  Path(id): Path<String>,
) -> ApiResult<Json<Published>> {
  let viewer = Viewer::of(user.as_ref());
  let row = sketches::find_visible(&state.db, &id, &viewer).await?;
  let published = row.published(&state.db).await?;
  Ok(Json(published.ok_or(AppError::NotFound)?))
}

/// `GET /api/sketches/{id}/revisions` (owner only): `{revisions: [{number,
/// created_at, published, head}]}`, newest first.
pub async fn revisions(
  State(state): State<AppState>,
  ApiUser(user): ApiUser,
  Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
  let row = find_owned(&state, &id, &user).await?;
  let revisions =
    revisions::list(&state.db, &id, row.head_revision, row.published_revision)
      .await?;
  Ok(Json(json!({ "revisions": revisions })))
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

/// `POST /api/sketches/{id}/like`: `{liked, like_count}`. Needs a verified
/// email address (likes rank sketches on explore); unliking doesn't.
pub async fn like(
  State(state): State<AppState>,
  ApiUser(user): ApiUser,
  Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
  user.require_verified("like sketches")?;
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
