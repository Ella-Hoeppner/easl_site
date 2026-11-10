//! `/`: the public gallery, the editor host's home page, with the most-used
//! tags above it.

use axum::extract::{Query, RawQuery, State};
use axum::response::{IntoResponse, Redirect, Response};
use minijinja::context;
use serde::Deserialize;

use super::{PageContext, page_links, page_number, render_page, url_with};
use crate::auth::Viewer;
use crate::error::PageResult;
use crate::sketches::{self, Sort};
use crate::state::AppState;
use crate::tags;
use crate::validate;

/// The longest search query or tag used, in characters.
const MAX_QUERY_CHARS: usize = 100;

/// How many of the most-used tags explore shows.
const POPULAR_TAGS: i64 = 16;

#[derive(Deserialize)]
pub struct ExploreQuery {
  sort: Option<String>,
  q: Option<String>,
  tag: Option<String>,
  page: Option<String>,
}

/// `GET /?sort=new|popular|trending&q=…&tag=…&page=N`. A tag is normalized
/// the way tags are saved (`?tag=Ray%20Marching` is `ray-marching`).
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
  let tag: String = validate::normalize_tag(query.tag.as_deref().unwrap_or(""))
    .chars()
    .take(MAX_QUERY_CHARS)
    .collect();
  let number = page_number(query.page.as_deref());
  let results = sketches::explore(
    &state.db,
    &viewer,
    sort,
    Some(search.as_str()),
    Some(tag.as_str()).filter(|t| !t.is_empty()),
    number,
  )
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
      url => url_with(
        "/",
        &[("sort", option.as_str()), ("q", &search), ("tag", &tag)],
      ),
      current => *option == sort,
    }
  })
  .collect();
  // A tag's link keeps the sort and search; the current tag's clears it.
  let tag_url = |name: &str| {
    url_with(
      "/",
      &[("sort", sort.as_str()), ("q", &search), ("tag", name)],
    )
  };
  let clear_tag_url = tag_url("");
  let popular: Vec<_> = tags::popular(&state.db, POPULAR_TAGS)
    .await?
    .into_iter()
    .map(|entry| {
      let current = entry.tag == tag;
      let url = if current {
        clear_tag_url.clone()
      } else {
        tag_url(&entry.tag)
      };
      context! { tag => entry.tag, count => entry.count, url, current }
    })
    .collect();
  let params = [("sort", sort.as_str()), ("q", &search), ("tag", &tag)];
  let pages = page_links("/", &params, number, results.has_more);
  render_page(
    &state,
    &page,
    "explore.html",
    context! {
      sketches => cards,
      sort => sort.as_str(),
      sorts,
      q => search,
      tag,
      popular_tags => popular,
      clear_tag_url,
      clear_search_url =>
        url_with("/", &[("sort", sort.as_str()), ("tag", &tag)]),
      first_page_url => url_with("/", &params),
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
