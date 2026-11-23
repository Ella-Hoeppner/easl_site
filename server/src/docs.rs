//! The docs host: the static site, with redirects to the editor host for
//! the old playground and for the app's paths, and the pages' visit beacon.
//!
//! In production a static file server serves the docs itself; serving them
//! here too keeps development to one process.

use axum::Router;
use axum::extract::{Query, Request, State};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use serde::Deserialize;

use crate::site;
use crate::state::AppState;
use crate::visits::{self, Site, VISIT_PATH};

/// The first path segments of the app's pages and API, which the docs host
/// redirects to the same path on the editor host.
const APP_SEGMENTS: &[&str] = &[
  "admin",
  "api",
  "explore",
  "forgot",
  "login",
  "logout",
  "new",
  "p",
  "playlists",
  "reset",
  "s",
  "settings",
  "signup",
  "u",
  "verify",
];

/// The docs host's routes.
pub fn router(state: AppState) -> Router {
  Router::new()
    .route("/playground", get(playground))
    .route("/playground.html", get(playground))
    .route(VISIT_PATH, visits::route(Site::Docs))
    .fallback_service(site::files(&state, state.config.site_dir.clone()))
    .layer(middleware::from_fn_with_state(
      state.clone(),
      redirect_app_paths,
    ))
    .with_state(state)
}

/// Whether a path belongs to the app rather than the docs: its first
/// segment is one of [`APP_SEGMENTS`].
fn is_app_path(path: &str) -> bool {
  let first = path.trim_start_matches('/').split('/').next().unwrap_or("");
  APP_SEGMENTS.contains(&first)
}

/// Middleware sending the app's paths to the editor host (308, so the
/// method and body carry over), query included.
async fn redirect_app_paths(
  State(state): State<AppState>,
  request: Request,
  next: Next,
) -> Response {
  let uri = request.uri();
  if !is_app_path(uri.path()) {
    return next.run(request).await;
  }
  let path = uri.path_and_query().map_or(uri.path(), |pq| pq.as_str());
  Redirect::permanent(&state.config.editor_url(path)).into_response()
}

#[derive(Deserialize)]
pub struct PlaygroundQuery {
  example: Option<String>,
}

/// `GET /playground?example=<id>` (and `/playground.html`): the old
/// playground, now the editor. An example goes to the sketch holding it
/// (from `editor/examples.json`); anything else to a new sketch.
async fn playground(
  State(state): State<AppState>,
  Query(query): Query<PlaygroundQuery>,
) -> Response {
  let sketch = query
    .example
    .and_then(|example| state.site.example_sketch(&example));
  let path = match sketch {
    Some(id) => format!("/s/{id}"),
    None => "/new".to_string(),
  };
  Redirect::permanent(&state.config.editor_url(&path)).into_response()
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn app_paths() {
    for path in ["/explore", "/s/abc", "/api/me", "/u/x/avatar.svg", "/new"] {
      assert!(is_app_path(path), "{path}");
    }
    for path in ["/", "/guide.html", "/s.html", "/editor/editor.js", "/news"] {
      assert!(!is_app_path(path), "{path}");
    }
  }
}
