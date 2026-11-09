//! Request-level protections: the cross-site request check and the client
//! address used for rate limiting.

use std::net::SocketAddr;

use axum::extract::{ConnectInfo, FromRequestParts, Request, State};
use axum::http::request::Parts;
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::config::Config;
use crate::error::AppError;
use crate::state::AppState;

/// Whether a state-changing request comes from this site: its `Origin` is
/// an allowed origin, or, when a browser sends no `Origin`, its
/// `Sec-Fetch-Site` is `same-origin`.
pub fn is_same_site_request(config: &Config, parts: &Parts) -> bool {
  if let Some(origin) = parts.headers.get("origin") {
    return origin
      .to_str()
      .is_ok_and(|origin| config.is_allowed_origin(origin));
  }
  parts
    .headers
    .get("sec-fetch-site")
    .is_some_and(|site| site == "same-origin")
}

/// Middleware rejecting cross-site requests (403) for every method other
/// than GET, HEAD, and OPTIONS. Together with `SameSite=Lax` session
/// cookies, this is the site's CSRF protection.
pub async fn csrf_guard(
  State(state): State<AppState>,
  request: Request,
  next: Next,
) -> Response {
  let safe = matches!(
    *request.method(),
    Method::GET | Method::HEAD | Method::OPTIONS
  );
  if safe {
    return next.run(request).await;
  }
  let (parts, body) = request.into_parts();
  if !is_same_site_request(&state.config, &parts) {
    let error = AppError::forbidden(
      "This request didn't come from this site, so it was refused.",
    );
    let wants_json = parts.uri.path().starts_with("/api/");
    return if wants_json {
      error.into_response()
    } else {
      (StatusCode::FORBIDDEN, error.message()).into_response()
    };
  }
  next.run(Request::from_parts(parts, body)).await
}

/// The client's address, for rate limiting: `X-Forwarded-For`'s last hop
/// behind a trusted proxy (which appends the address it saw), else the
/// socket's peer address.
pub struct ClientIp(pub String);

impl FromRequestParts<AppState> for ClientIp {
  type Rejection = AppError;

  async fn from_request_parts(
    parts: &mut Parts,
    state: &AppState,
  ) -> Result<Self, Self::Rejection> {
    if state.config.trust_proxy
      && let Some(ip) = parts
        .headers
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .rfind(|hop| !hop.is_empty())
    {
      return Ok(ClientIp(ip.to_string()));
    }
    let peer = parts
      .extensions
      .get::<ConnectInfo<SocketAddr>>()
      .map(|ConnectInfo(addr)| addr.ip().to_string())
      .unwrap_or_else(|| "unknown".into());
    Ok(ClientIp(peer))
  }
}
