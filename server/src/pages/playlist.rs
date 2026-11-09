//! `/p/{id}`: a playlist.

use axum::extract::{Path, State};
use minijinja::context;

use super::{PageContext, render_page};
use crate::auth::Viewer;
use crate::error::PageResult;
use crate::playlists;
use crate::state::AppState;

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
