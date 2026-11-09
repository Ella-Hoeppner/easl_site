//! The router: the API, the pages, the app's assets, and the static site,
//! with the layers every request passes through.

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::extract::RawQuery;
use axum::http::HeaderValue;
use axum::http::header::{
  CACHE_CONTROL, REFERRER_POLICY, X_CONTENT_TYPE_OPTIONS, X_FRAME_OPTIONS,
};
use axum::middleware::from_fn_with_state;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use tower_http::CompressionLevel;
use tower_http::compression::CompressionLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::TraceLayer;

use crate::api;
use crate::assets;
use crate::auth::session_layer;
use crate::pages::{self, render_error_pages};
use crate::security::csrf_guard;
use crate::site;
use crate::state::AppState;
use crate::validate::MAX_BODY_BYTES;

/// Builds the whole application.
///
/// Dynamic routes (the API and the pages) resolve the session, render
/// error pages, and are never cached. `/static/…` serves the app's assets
/// and anything else falls through to the static site. Every response gets
/// the `nosniff`, referrer, and framing headers; every request is checked
/// for cross-site origin, size-limited, and traced.
pub fn router(state: AppState) -> Router {
  let dynamic = Router::new()
    .merge(api::router())
    .merge(pages::router())
    .route_layer(from_fn_with_state(state.clone(), render_error_pages))
    .route_layer(from_fn_with_state(state.clone(), session_layer))
    .route_layer(SetResponseHeaderLayer::if_not_present(
      CACHE_CONTROL,
      HeaderValue::from_static("no-store"),
    ));
  Router::new()
    .merge(dynamic)
    .route("/static/{*path}", get(assets::serve))
    .route("/playground.html", get(redirect_playground))
    .with_state(state.clone())
    .fallback_service(site::router(state.clone()))
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

/// `/playground.html` moved to `/playground`; the query string carries over.
async fn redirect_playground(RawQuery(query): RawQuery) -> Response {
  let location = match query {
    Some(query) if !query.is_empty() => format!("/playground?{query}"),
    _ => "/playground".to_string(),
  };
  Redirect::permanent(&location).into_response()
}
