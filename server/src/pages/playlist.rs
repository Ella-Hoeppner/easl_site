//! `/p/{id}`: a playlist, and `/playlists/new`: making one.

use axum::Form;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use minijinja::context;
use serde::Deserialize;

use super::{PageContext, render_page, render_status, see_other};
use crate::auth::{PageUser, Viewer};
use crate::error::{AppError, PageResult};
use crate::playlists;
use crate::state::AppState;
use crate::validate;
use crate::visibility::Visibility;

/// `GET /p/{id}`: the playlist's sketches in order, with a link that plays
/// through them. Its owner can edit, arrange, and delete it here (through
/// the JSON API).
pub async fn playlist(
  State(state): State<AppState>,
  page: PageContext,
  Path(id): Path<String>,
) -> PageResult {
  let viewer = Viewer::of(page.user.as_ref());
  let row = playlists::find_visible(&state.db, &id, &viewer).await?;
  let playlist = row.detail(&state.db, &viewer).await?;
  let play_url = playlist
    .items
    .first()
    .map(|first| format!("/s/{}?playlist={}", first.id, playlist.id));
  render_page(
    &state,
    &page,
    "playlist.html",
    context! { playlist, play_url },
  )
}

/// The new-playlist form's fields.
#[derive(Default, Deserialize)]
pub struct NewPlaylistForm {
  #[serde(default)]
  title: String,
  #[serde(default)]
  description: String,
  #[serde(default)]
  visibility: String,
}

/// Renders the new-playlist form with `values`, and `error` above it.
fn new_playlist_page(
  state: &AppState,
  page: &PageContext,
  status: StatusCode,
  values: &NewPlaylistForm,
  error: Option<String>,
) -> PageResult {
  render_status(
    state,
    page,
    status,
    "playlist_new.html",
    context! {
      title => &values.title,
      description => &values.description,
      visibility => &values.visibility,
      error,
    },
  )
}

/// `GET /playlists/new`: the form for a new playlist, public by default
/// for verified users (who may publish) and private otherwise.
pub async fn new_form(
  State(state): State<AppState>,
  PageUser(user): PageUser,
  page: PageContext,
) -> PageResult {
  let visibility = if user.email_verified() {
    Visibility::Public
  } else {
    Visibility::Private
  };
  let values = NewPlaylistForm {
    visibility: visibility.as_str().into(),
    ..NewPlaylistForm::default()
  };
  new_playlist_page(&state, &page, StatusCode::OK, &values, None)
}

/// `POST /playlists/new`: creates the playlist and goes to its page, or
/// shows the form again with what was wrong.
pub async fn create(
  State(state): State<AppState>,
  PageUser(user): PageUser,
  page: PageContext,
  Form(form): Form<NewPlaylistForm>,
) -> PageResult {
  let created = async {
    let visibility = Visibility::parse(&form.visibility)
      .ok_or_else(|| AppError::bad_request("Choose who can see it."))?;
    playlists::check_visibility(&user, visibility)?;
    let title = validate::playlist_title(&form.title)?;
    let description = validate::playlist_description(&form.description)?;
    playlists::check_create_limit(&state, &user)?;
    playlists::create(&state.db, user.id, &title, &description, visibility)
      .await
  }
  .await;
  match created {
    Ok(id) => Ok(see_other(&format!("/p/{id}"))),
    Err(error) => new_playlist_page(
      &state,
      &page,
      error.status(),
      &form,
      Some(error.message()),
    ),
  }
}
