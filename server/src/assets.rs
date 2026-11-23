//! The app's own static assets (`server/static/`), served at `/static/…`.
//!
//! Release builds embed them and version each URL with a hash of its
//! content (`/static/app.css?v=…`), so browsers can keep them forever. In
//! dev they're read from disk on every request, with a fresh hash, so edits
//! show on refresh.

use std::fs;
use std::path::PathBuf;

use axum::body::Body;
use axum::extract::{Path, RawQuery, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};

use crate::error::AppError;
use crate::state::AppState;

/// The assets: published path, content type, and embedded content.
const ASSETS: &[(&str, &str, &[u8])] = &[
  (
    "app.css",
    "text/css; charset=utf-8",
    include_bytes!("../static/app.css"),
  ),
  (
    "app.js",
    "text/javascript; charset=utf-8",
    include_bytes!("../static/app.js"),
  ),
  (
    "visit.js",
    "text/javascript; charset=utf-8",
    include_bytes!("../static/visit.js"),
  ),
];

/// The directory dev mode reads assets from.
fn dev_dir() -> PathBuf {
  PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("static")
}

fn content_hash(bytes: &[u8]) -> String {
  Sha256::digest(bytes)
    .iter()
    .take(6)
    .map(|b| format!("{b:02x}"))
    .collect()
}

/// One asset's content and version.
pub struct Asset {
  pub content_type: &'static str,
  pub bytes: Vec<u8>,
  pub version: String,
}

/// The asset table, embedded or (in dev) on disk.
pub struct Assets {
  dev: bool,
  versions: Vec<String>,
}

impl Assets {
  pub fn new(dev: bool) -> Self {
    Assets {
      dev,
      versions: ASSETS.iter().map(|(_, _, b)| content_hash(b)).collect(),
    }
  }

  /// The asset published at `path`, if there is one.
  pub fn get(&self, path: &str) -> Option<Asset> {
    let index = ASSETS.iter().position(|(p, _, _)| *p == path)?;
    let (_, content_type, embedded) = ASSETS[index];
    if self.dev {
      let bytes = fs::read(dev_dir().join(path)).ok()?;
      let version = content_hash(&bytes);
      return Some(Asset {
        content_type,
        bytes,
        version,
      });
    }
    Some(Asset {
      content_type,
      bytes: embedded.to_vec(),
      version: self.versions[index].clone(),
    })
  }

  /// The versioned URL of an asset (unversioned if it doesn't exist).
  pub fn url(&self, path: &str) -> String {
    match self.version(path) {
      Some(version) => format!("/static/{path}?v={version}"),
      None => format!("/static/{path}"),
    }
  }

  fn version(&self, path: &str) -> Option<String> {
    if self.dev {
      return self.get(path).map(|asset| asset.version);
    }
    let index = ASSETS.iter().position(|(p, _, _)| *p == path)?;
    Some(self.versions[index].clone())
  }
}

/// Serves `/static/{path}`: cached for a year when the URL carries a
/// version (never in dev), else revalidated on every load.
pub async fn serve(
  State(state): State<AppState>,
  Path(path): Path<String>,
  RawQuery(query): RawQuery,
) -> Result<Response, AppError> {
  let asset = state.assets.get(&path).ok_or(AppError::NotFound)?;
  let versioned = query
    .as_deref()
    .is_some_and(|q| q.split('&').any(|p| p.starts_with("v=")));
  let cache = if versioned && !state.config.dev {
    "public, max-age=31536000, immutable"
  } else {
    "no-cache"
  };
  Ok(
    (
      [(CONTENT_TYPE, asset.content_type), (CACHE_CONTROL, cache)],
      Body::from(asset.bytes),
    )
      .into_response(),
  )
}
