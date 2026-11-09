//! `/`: the public gallery, the editor host's home page.

use axum::extract::{Query, RawQuery, State};
use axum::response::{IntoResponse, Redirect, Response};
use minijinja::context;
use serde::Deserialize;

use super::{PageContext, page_links, page_number, render_page, url_with};
use crate::auth::Viewer;
use crate::error::PageResult;
use crate::sketches::{self, Sort};
use crate::state::AppState;

/// The longest search query used, in characters.
const MAX_QUERY_CHARS: usize = 100;

#[derive(Deserialize)]
pub struct ExploreQuery {
  sort: Option<String>,
  q: Option<String>,
  page: Option<String>,
}

/// `GET /?sort=new|popular|trending&q=…&page=N`.
pub async fn explore(
  State(state): State<AppState>,
  page: PageContext,
  Query(query): Query<ExploreQuery>,
) -> PageResult {
  let viewer = Viewer::of(page.user.as_ref());
  let sort = Sort::parse(query.sort.as_deref());
  let search: String = query
    .q
    .as_deref()
    .unwrap_or("")
    .trim()
    .chars()
    .take(MAX_QUERY_CHARS)
    .collect();
  let number = page_number(query.page.as_deref());
  let results =
    sketches::explore(&state.db, &viewer, sort, Some(search.as_str()), number)
      .await?;
  let cards: Vec<_> = results
    .items
    .iter()
    .map(|row| row.summary(&viewer))
    .collect();
  let sorts: Vec<_> = [
    (Sort::New, "New"),
    (Sort::Popular, "Popular"),
    (Sort::Trending, "Trending"),
  ]
  .iter()
  .map(|(option, label)| {
    context! {
      label,
      url => url_with("/", &[("sort", option.as_str()), ("q", &search)]),
      current => *option == sort,
    }
  })
  .collect();
  let pages = page_links(
    "/",
    &[("sort", sort.as_str()), ("q", &search)],
    number,
    results.has_more,
  );
  render_page(
    &state,
    &page,
    "explore.html",
    context! {
      sketches => cards,
      sort => sort.as_str(),
      sorts,
      q => search,
      pages,
    },
  )
}

/// `GET /explore`: the gallery's old address, redirecting to `/` with the
/// same query.
pub async fn redirect_home(RawQuery(query): RawQuery) -> Response {
  let location = match query {
    Some(query) if !query.is_empty() => format!("/?{query}"),
    _ => "/".to_string(),
  };
  Redirect::permanent(&location).into_response()
}
