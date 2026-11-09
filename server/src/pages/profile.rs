//! `/u/{username}`: a user's profile, with tabs for their sketches,
//! playlists, and likes.

use axum::extract::{Path, Query, State};
use minijinja::context;
use serde::{Deserialize, Serialize};

use super::{PageContext, page_links, page_number, render_page, url_with};
use crate::auth::Viewer;
use crate::error::{PageError, PageResult};
use crate::playlists::{self, PlaylistRow};
use crate::sketches;
use crate::state::AppState;
use crate::users;
use crate::validate;
use crate::visibility::Visibility;

#[derive(Deserialize)]
pub struct ProfileQuery {
  tab: Option<String>,
  page: Option<String>,
}

/// A playlist as a profile lists it.
#[derive(Serialize)]
struct PlaylistCard {
  id: String,
  title: String,
  visibility: Visibility,
  item_count: i64,
  updated_at: i64,
}

impl From<&PlaylistRow> for PlaylistCard {
  fn from(row: &PlaylistRow) -> Self {
    PlaylistCard {
      id: row.id.clone(),
      title: row.title.clone(),
      visibility: row.visibility,
      item_count: row.item_count,
      updated_at: row.updated_at,
    }
  }
}

/// `GET /u/{username}?tab=sketches|playlists|likes&page=N`. Owners also see
/// their private and unlisted things; banned users' profiles are 404s
/// except to admins.
pub async fn profile(
  State(state): State<AppState>,
  page: PageContext,
  Path(username): Path<String>,
  Query(query): Query<ProfileQuery>,
) -> PageResult {
  if !validate::is_username_shaped(&username) {
    return Err(PageError::not_found());
  }
  let viewer = Viewer::of(page.user.as_ref());
  let user = users::by_username(&state.db, &username)
    .await?
    .filter(|u| u.banned_at.is_none() || viewer.is_admin)
    .ok_or_else(PageError::not_found)?;
  let own = viewer.is(user.id);
  let tab = match query.tab.as_deref() {
    Some("playlists") => "playlists",
    Some("likes") => "likes",
    _ => "sketches",
  };
  let number = page_number(query.page.as_deref());
  let (sketch_count, likes_received) =
    sketches::listed_counts(&state.db, user.id).await?;
  let playlist_rows = playlists::by_owner(&state.db, user.id, own).await?;
  let (cards, has_next) = match tab {
    "sketches" => {
      let page =
        sketches::by_owner(&state.db, &viewer, user.id, own, number).await?;
      (page.items, page.has_more)
    }
    "likes" => {
      let page =
        sketches::liked_by(&state.db, &viewer, user.id, own, number).await?;
      (page.items, page.has_more)
    }
    _ => (Vec::new(), false),
  };
  let cards: Vec<_> = cards.iter().map(|row| row.summary(&viewer)).collect();
  let playlist_cards: Vec<PlaylistCard> =
    playlist_rows.iter().map(PlaylistCard::from).collect();
  let base = format!("/u/{}", user.username);
  let tabs: Vec<_> = [
    ("sketches", "Sketches"),
    ("playlists", "Playlists"),
    ("likes", "Likes"),
  ]
  .iter()
  .map(|(key, label)| {
    let param = if *key == "sketches" { "" } else { key };
    context! {
      label,
      url => url_with(&base, &[("tab", param)]),
      current => *key == tab,
    }
  })
  .collect();
  let tab_param = if tab == "sketches" { "" } else { tab };
  let pages = page_links(&base, &[("tab", tab_param)], number, has_next);
  render_page(
    &state,
    &page,
    "profile.html",
    context! {
      profile => context! {
        username => &user.username,
        display_name => user.shown_name(),
        bio => &user.bio,
        created_at => user.created_at,
        avatar_url => users::avatar_url(&user.username),
        banned => user.banned_at.is_some(),
      },
      own,
      tab,
      tabs,
      pages,
      sketches => cards,
      playlists => playlist_cards,
      sketch_count,
      likes_received,
      playlist_count => playlist_rows.len(),
    },
  )
}
