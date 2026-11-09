//! The generated static site (`--site-dir`): serving its files, and the
//! editor build's manifest and example index, re-read when they change.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use axum::Router;
use axum::extract::{Request, State};
use axum::handler::Handler;
use axum::http::header::CACHE_CONTROL;
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::Response;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use tower_http::services::ServeDir;

use crate::db::is_public_id;
use crate::pages;
use crate::state::AppState;

/// The Monaco version used when the site has no editor manifest.
const DEFAULT_MONACO_VERSION: &str = "0.52.2";

/// The editor build's directory in the site.
pub const EDITOR_DIR: &str = "editor";

/// The editor build's versions, from `editor/manifest.json`.
#[derive(Clone, Debug, Deserialize)]
pub struct Manifest {
  pub version: String,
  pub monaco_version: String,
}

impl Default for Manifest {
  fn default() -> Self {
    Manifest {
      version: "dev".into(),
      monaco_version: DEFAULT_MONACO_VERSION.into(),
    }
  }
}

/// `editor/examples.json`, where the editor build lists the docs' example
/// programs.
pub fn examples_path(site_dir: &Path) -> PathBuf {
  site_dir.join(EDITOR_DIR).join("examples.json")
}

/// What the docs' redirects need from `editor/examples.json`: each
/// example's old id and the sketch that holds it.
#[derive(Deserialize)]
struct ExampleIndex {
  examples: Vec<ExampleLink>,
}

#[derive(Deserialize)]
struct ExampleLink {
  id: String,
  sketch_id: String,
}

/// A JSON file parsed on demand and cached until its modification time
/// changes, so a site rebuild needs no server restart.
struct WatchedJson<T> {
  path: PathBuf,
  cache: Mutex<Option<(SystemTime, Arc<T>)>>,
}

impl<T: DeserializeOwned> WatchedJson<T> {
  fn new(path: PathBuf) -> Self {
    WatchedJson {
      path,
      cache: Mutex::new(None),
    }
  }

  /// The file's content, or `None` when it's missing or unreadable (the
  /// latter logged).
  fn get(&self) -> Option<Arc<T>> {
    let modified = fs::metadata(&self.path).and_then(|m| m.modified()).ok()?;
    if let Ok(cache) = self.cache.lock()
      && let Some((cached_at, value)) = cache.as_ref()
      && *cached_at == modified
    {
      return Some(value.clone());
    }
    let parsed = fs::read_to_string(&self.path)
      .ok()
      .and_then(|text| serde_json::from_str::<T>(&text).ok());
    let Some(value) = parsed.map(Arc::new) else {
      tracing::warn!("couldn't read {}", self.path.display());
      return None;
    };
    if let Ok(mut cache) = self.cache.lock() {
      *cache = Some((modified, value.clone()));
    }
    Some(value)
  }
}

/// The site directory and caches of the editor build's files.
pub struct SiteFiles {
  pub dir: PathBuf,
  manifest: WatchedJson<Manifest>,
  examples: WatchedJson<ExampleIndex>,
}

impl SiteFiles {
  pub fn new(dir: PathBuf) -> Self {
    SiteFiles {
      manifest: WatchedJson::new(dir.join(EDITOR_DIR).join("manifest.json")),
      examples: WatchedJson::new(examples_path(&dir)),
      dir,
    }
  }

  /// The editor manifest; the defaults when it's missing or unreadable.
  pub fn manifest(&self) -> Manifest {
    self
      .manifest
      .get()
      .map(|manifest| (*manifest).clone())
      .unwrap_or_default()
  }

  /// The id of the sketch holding the example `example_id`, if the site
  /// lists one.
  pub fn example_sketch(&self, example_id: &str) -> Option<String> {
    let index = self.examples.get()?;
    index
      .examples
      .iter()
      .find(|example| example.id == example_id)
      .map(|example| example.sketch_id.clone())
      .filter(|id| is_public_id(id))
  }
}

/// A router serving the files under `dir`, with the site's cache policy
/// and the app's 404 page for missing files.
pub fn files(state: &AppState, dir: PathBuf) -> Router {
  let not_found = not_found_page.with_state(state.clone());
  let files = ServeDir::new(dir)
    .append_index_html_on_directories(true)
    .not_found_service(not_found);
  Router::new()
    .fallback_service(files)
    .layer(middleware::from_fn(cache_headers))
}

async fn not_found_page(State(state): State<AppState>) -> Response {
  pages::static_not_found(&state)
}

/// Sets static files' `Cache-Control`: a year for URLs with a `v=` query
/// (the build's hash), else revalidate on every load.
async fn cache_headers(request: Request, next: Next) -> Response {
  let versioned = request
    .uri()
    .query()
    .is_some_and(|q| q.split('&').any(|p| p.starts_with("v=")));
  let mut response = next.run(request).await;
  let status = response.status();
  if status.is_success() || status == StatusCode::NOT_MODIFIED {
    let value = if versioned {
      "public, max-age=31536000, immutable"
    } else {
      "no-cache"
    };
    response
      .headers_mut()
      .insert(CACHE_CONTROL, HeaderValue::from_static(value));
  }
  response
}
