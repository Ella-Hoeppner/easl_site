//! The generated static site (`--site-dir`), served at `/`, and the
//! playground build's manifest.

use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::SystemTime;

use axum::Router;
use axum::extract::{Request, State};
use axum::handler::Handler;
use axum::http::header::CACHE_CONTROL;
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::Response;
use serde::Deserialize;
use tower_http::services::ServeDir;

use crate::error::PageError;
use crate::pages;
use crate::state::AppState;

/// The Monaco version used when the site has no playground manifest.
const DEFAULT_MONACO_VERSION: &str = "0.52.2";

/// The playground build's versions, from `playground/manifest.json`.
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

/// One example program from `playground/examples.json`.
#[derive(Clone, Debug, Deserialize)]
pub struct Example {
  pub id: String,
  pub menu: Option<String>,
  pub title: String,
  pub source: String,
}

#[derive(Deserialize)]
struct ExamplesFile {
  examples: Vec<Example>,
}

/// The site directory and a cache of its manifest.
pub struct SiteFiles {
  pub dir: PathBuf,
  manifest: Mutex<Option<(SystemTime, Manifest)>>,
}

impl SiteFiles {
  pub fn new(dir: PathBuf) -> Self {
    SiteFiles {
      dir,
      manifest: Mutex::new(None),
    }
  }

  fn manifest_path(&self) -> PathBuf {
    self.dir.join("playground").join("manifest.json")
  }

  /// The playground manifest, re-read whenever the file changes (a site
  /// rebuild needs no server restart); the defaults when it's missing or
  /// unreadable.
  pub fn manifest(&self) -> Manifest {
    let path = self.manifest_path();
    let Ok(modified) = fs::metadata(&path).and_then(|m| m.modified()) else {
      return Manifest::default();
    };
    if let Ok(cache) = self.manifest.lock()
      && let Some((cached_at, manifest)) = cache.as_ref()
      && *cached_at == modified
    {
      return manifest.clone();
    }
    let manifest = fs::read_to_string(&path)
      .ok()
      .and_then(|text| serde_json::from_str::<Manifest>(&text).ok());
    let Some(manifest) = manifest else {
      tracing::warn!("couldn't read {}", path.display());
      return Manifest::default();
    };
    if let Ok(mut cache) = self.manifest.lock() {
      *cache = Some((modified, manifest.clone()));
    }
    manifest
  }

  /// The playground's example programs, if the site has them.
  pub fn examples(&self) -> Vec<Example> {
    let path = self.dir.join("playground").join("examples.json");
    fs::read_to_string(path)
      .ok()
      .and_then(|text| serde_json::from_str::<ExamplesFile>(&text).ok())
      .map(|file| file.examples)
      .unwrap_or_default()
  }
}

/// The router serving the site directory, with the site's cache policy and
/// the app's 404 page for missing files.
pub fn router(state: AppState) -> Router {
  let not_found = not_found_page.with_state(state.clone());
  let files = ServeDir::new(&state.site.dir)
    .append_index_html_on_directories(true)
    .not_found_service(not_found);
  Router::new()
    .fallback_service(files)
    .layer(middleware::from_fn(cache_headers))
}

async fn not_found_page(State(state): State<AppState>) -> Response {
  pages::error_response(
    &state,
    None,
    StatusCode::NOT_FOUND,
    &PageError::not_found().0.message(),
  )
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
