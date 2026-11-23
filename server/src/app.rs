//! The router: requests are dispatched by `Host` to the editor host (the
//! app) or the docs host (the static site), behind the layers every request
//! passes through.

use axum::Router;
use axum::extract::{DefaultBodyLimit, Request};
use axum::http::HeaderValue;
use axum::http::header::{
  CACHE_CONTROL, HOST, REFERRER_POLICY, X_CONTENT_TYPE_OPTIONS, X_FRAME_OPTIONS,
};
use axum::middleware::from_fn_with_state;
use axum::routing::get;
use tower::ServiceExt;
use tower::service_fn;
use tower_http::CompressionLevel;
use tower_http::compression::CompressionLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::TraceLayer;

use crate::api;
use crate::assets;
use crate::auth::session_layer;
use crate::docs;
use crate::pages::{self, render_error_pages};
use crate::security::csrf_guard;
use crate::site::{self, EDITOR_DIR};
use crate::state::AppState;
use crate::validate::MAX_BODY_BYTES;
use crate::visits::{self, Site, VISIT_PATH};

/// The web runtime's directory in the site, served on the editor host too.
const RUNTIME_DIR: &str = "easl-web";

/// Builds the whole application.
///
/// A request whose host is the editor origin's goes to [`editor_router`],
/// any other to [`docs::router`]. Every response gets the `nosniff`,
/// referrer, and framing headers; every request is checked for cross-site
/// origin, size-limited, and traced.
pub fn router(state: AppState) -> Router {
  let editor = editor_router(state.clone());
  let docs = docs::router(state.clone());
  let config = state.config.clone();
  let by_host = service_fn(move |request: Request| {
    let target = if config.is_editor_host(request_host(&request)) {
      editor.clone()
    } else {
      docs.clone()
    };
    target.oneshot(request)
  });
  Router::new()
    .fallback_service(by_host)
    .layer(from_fn_with_state(state, csrf_guard))
    .layer(SetResponseHeaderLayer::if_not_present(
      X_CONTENT_TYPE_OPTIONS,
      HeaderValue::from_static("nosniff"),
    ))
    .layer(SetResponseHeaderLayer::if_not_present(
      REFERRER_POLICY,
      HeaderValue::from_static("strict-origin-when-cross-origin"),
    ))
    .layer(SetResponseHeaderLayer::if_not_present(
      X_FRAME_OPTIONS,
      HeaderValue::from_static("SAMEORIGIN"),
    ))
    .layer(CompressionLayer::new().quality(CompressionLevel::Precise(4)))
    .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
    .layer(RequestBodyLimitLayer::new(MAX_BODY_BYTES))
    .layer(TraceLayer::new_for_http())
}

/// The host a request was sent to: the URI's authority (HTTP/2, or an
/// absolute request target), else the `Host` header.
fn request_host(request: &Request) -> &str {
  if let Some(authority) = request.uri().authority() {
    return authority.as_str();
  }
  request
    .headers()
    .get(HOST)
    .and_then(|host| host.to_str().ok())
    .unwrap_or("")
}

/// The editor host: the API and the pages (with the explore page at `/`),
/// the app's assets at `/static/…`, the pages' visit beacon (`/visit`), and
/// from the site directory only the editor's files (`/editor/…`) and the
/// web runtime (`/easl-web/…`).
///
/// Dynamic routes (the API and the pages, and the 404 page for anything
/// else) resolve the session, render error pages, and are never cached.
fn editor_router(state: AppState) -> Router {
  let site_dir = &state.config.site_dir;
  Router::new()
    .merge(api::router())
    .merge(pages::router())
    .fallback(pages::not_found)
    .layer(from_fn_with_state(state.clone(), render_error_pages))
    .layer(from_fn_with_state(state.clone(), session_layer))
    .layer(SetResponseHeaderLayer::if_not_present(
      CACHE_CONTROL,
      HeaderValue::from_static("no-store"),
    ))
    .route("/static/{*path}", get(assets::serve))
    .route(VISIT_PATH, visits::route(Site::Editor))
    .nest_service(
      &format!("/{EDITOR_DIR}"),
      site::files(&state, site_dir.join(EDITOR_DIR)),
    )
    .nest_service(
      &format!("/{RUNTIME_DIR}"),
      site::files(&state, site_dir.join(RUNTIME_DIR)),
    )
    .with_state(state)
}
