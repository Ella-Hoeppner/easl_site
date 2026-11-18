//! Server-rendered HTML pages.
//!
//! The pages live on the editor host. Every page template extends
//! `base.html`, which gets the base context from [`render_page`]: `me` (the
//! signed-in user's API shape, or none), `path` (the request's path and
//! query, for sign-in links), `login_url`, `dev`, `editor_origin` and
//! `docs_origin` (the two public origins), and `app_base` (what the app's
//! own links start with: empty, except on pages served on the docs host,
//! where it's the editor origin).

mod account;
mod admin;
mod editor;
mod explore;
mod images;
mod playlist;
mod profile;
mod settings;

use axum::Router;
use axum::extract::{FromRequestParts, Request, State};
use axum::http::header::{CONTENT_SECURITY_POLICY, CONTENT_TYPE, LOCATION};
use axum::http::request::Parts;
use axum::http::{HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use minijinja::value::merge_maps;
use minijinja::{Value, context};

use crate::auth::{CurrentUser, Session};
use crate::error::{AppError, ErrorPage, PageError, PageResult};
use crate::state::AppState;
use crate::text::url_encode;

/// The Content-Security-Policy of the app's own pages (not the editor's,
/// which loads its editor from a CDN): only same-origin scripts, styles,
/// and images, no plugins, no framing by other sites.
const PAGE_CSP: &str = "default-src 'self'; img-src 'self' data:; \
  object-src 'none'; base-uri 'self'; form-action 'self'; \
  frame-ancestors 'self'";

/// The pages' routes.
pub fn router() -> Router<AppState> {
  Router::new()
    .route("/", get(explore::explore))
    .route("/explore", get(explore::redirect_home))
    .route("/u/{username}", get(profile::profile))
    .route("/u/{username}/avatar.svg", get(images::avatar))
    .route("/s/{id}", get(editor::sketch))
    .route("/s/{id}/thumb", get(images::thumbnail))
    .route("/p/{id}", get(playlist::playlist))
    .route(
      "/playlists/new",
      get(playlist::new_form).post(playlist::create),
    )
    .route("/new", get(editor::new_sketch))
    .route("/login", get(account::login_form).post(account::login))
    .route("/signup", get(account::signup_form).post(account::signup))
    .route("/logout", post(account::logout))
    .route("/forgot", get(account::forgot_form).post(account::forgot))
    .route(
      "/reset/{token}",
      get(account::reset_form).post(account::reset),
    )
    .route("/verify/{token}", get(account::verify))
    .route("/settings", get(settings::show))
    .route("/settings/profile", post(settings::profile))
    .route("/settings/email", post(settings::email))
    .route("/settings/password", post(settings::password))
    .route(
      "/settings/verification",
      post(settings::resend_verification),
    )
    .route("/settings/delete", post(settings::delete_account))
    .route("/admin", get(admin::admin))
    .route("/admin/users/{username}", get(admin::user))
}

/// The editor host's 404 page, for paths nothing else serves.
pub async fn not_found() -> PageError {
  PageError::not_found()
}

/// The signed-in user and the request's path, which every page needs.
pub struct PageContext {
  pub user: Option<CurrentUser>,
  /// The path and query, e.g. `/explore?sort=new`.
  pub path: String,
}

impl<S: Send + Sync> FromRequestParts<S> for PageContext {
  type Rejection = AppError;

  async fn from_request_parts(
    parts: &mut Parts,
    _state: &S,
  ) -> Result<Self, Self::Rejection> {
    Ok(PageContext {
      user: parts
        .extensions
        .get::<Session>()
        .and_then(|session| session.0.clone()),
      path: path_and_query(parts),
    })
  }
}

fn path_and_query(parts: &Parts) -> String {
  parts
    .uri
    .path_and_query()
    .map(|pq| pq.as_str().to_string())
    .unwrap_or_else(|| "/".into())
}

/// Renders a page template with the base context and `context`.
pub fn render_page(
  state: &AppState,
  page: &PageContext,
  template: &str,
  context: Value,
) -> PageResult {
  render_status(state, page, StatusCode::OK, template, context)
}

/// Like [`render_page`], with a status other than 200.
pub fn render_status(
  state: &AppState,
  page: &PageContext,
  status: StatusCode,
  template: &str,
  context: Value,
) -> PageResult {
  let body =
    render_html(state, page.user.as_ref(), &page.path, template, context)?;
  Ok(html_response(status, body, true))
}

fn render_html(
  state: &AppState,
  user: Option<&CurrentUser>,
  path: &str,
  template: &str,
  context: Value,
) -> anyhow::Result<String> {
  let base = context! {
    me => user.map(|u| u.me()),
    path => path,
    login_url => login_url(path),
    dev => state.config.dev,
    editor_origin => &state.config.editor_origin,
    docs_origin => &state.config.docs_origin,
    app_base => "",
  };
  // Later maps win, so a page can override the base context.
  state
    .templates
    .render(template, merge_maps([base, context]))
}

/// An HTML response, with the page CSP when `restrict` is set.
pub fn html_response(
  status: StatusCode,
  body: String,
  restrict: bool,
) -> Response {
  let mut response = (status, body).into_response();
  let headers = response.headers_mut();
  headers.insert(
    CONTENT_TYPE,
    HeaderValue::from_static("text/html; charset=utf-8"),
  );
  if restrict {
    headers.insert(CONTENT_SECURITY_POLICY, HeaderValue::from_static(PAGE_CSP));
  }
  response
}

/// A redirect that makes the browser GET `location`.
pub fn see_other(location: &str) -> Response {
  let mut response = StatusCode::SEE_OTHER.into_response();
  if let Ok(value) = HeaderValue::from_str(location) {
    response.headers_mut().insert(LOCATION, value);
  }
  response
}

/// The sign-in page's URL, returning to `next` afterwards.
pub fn login_url(next: &str) -> String {
  format!("/login?next={}", url_encode(next))
}

/// The HTML error page.
pub fn error_response(
  state: &AppState,
  user: Option<&CurrentUser>,
  status: StatusCode,
  message: &str,
) -> Response {
  error_page(state, user, status, message, context! {})
}

/// The 404 page for a missing static file. Static files are served on both
/// hosts, so its links to the app are absolute.
pub fn static_not_found(state: &AppState) -> Response {
  let status = StatusCode::NOT_FOUND;
  let message = AppError::NotFound.message();
  let context = context! { app_base => &state.config.editor_origin };
  error_page(state, None, status, &message, context)
}

/// The error page, with `extra` merged into its context.
fn error_page(
  state: &AppState,
  user: Option<&CurrentUser>,
  status: StatusCode,
  message: &str,
  extra: Value,
) -> Response {
  let context = context! {
    status => status.as_u16(),
    title => status.canonical_reason().unwrap_or("Error"),
    message => message,
    ..extra
  };
  match render_html(state, user, "/", "error.html", context) {
    Ok(body) => html_response(status, body, true),
    Err(error) => {
      tracing::error!("rendering the error page: {error:#}");
      (status, message.to_string()).into_response()
    }
  }
}

/// Middleware turning [`ErrorPage`] responses into HTML error pages, or,
/// for 401s on GET requests, into redirects to sign in.
pub async fn render_error_pages(
  State(state): State<AppState>,
  request: Request,
  next: Next,
) -> Response {
  let user = request
    .extensions()
    .get::<Session>()
    .and_then(|session| session.0.clone());
  let is_get = request.method() == Method::GET;
  let path = request
    .uri()
    .path_and_query()
    .map(|pq| pq.as_str().to_string())
    .unwrap_or_else(|| "/".into());
  let response = next.run(request).await;
  let Some(marker) = response.extensions().get::<ErrorPage>().cloned() else {
    return response;
  };
  if marker.status == StatusCode::UNAUTHORIZED {
    let next = if is_get { path.as_str() } else { "/" };
    return see_other(&login_url(next));
  }
  error_response(&state, user.as_ref(), marker.status, &marker.message)
}

/// A URL for `path` with query parameters, leaving out empty values.
pub fn url_with(path: &str, params: &[(&str, &str)]) -> String {
  let query: Vec<String> = params
    .iter()
    .filter(|(_, value)| !value.is_empty())
    .map(|(name, value)| format!("{name}={}", url_encode(value)))
    .collect();
  if query.is_empty() {
    path.to_string()
  } else {
    format!("{path}?{}", query.join("&"))
  }
}

/// The previous and next page links of a paginated listing at `path` with
/// `params`.
pub fn page_links(
  path: &str,
  params: &[(&str, &str)],
  number: i64,
  has_more: bool,
) -> Value {
  let link = |n: i64| {
    let page = if n > 1 { n.to_string() } else { String::new() };
    let mut all = params.to_vec();
    all.push(("page", &page));
    url_with(path, &all)
  };
  context! {
    number,
    prev => (number > 1).then(|| link(number - 1)),
    next => has_more.then(|| link(number + 1)),
  }
}

/// The page number from a `page` query parameter (1 when absent or bad).
pub fn page_number(page: Option<&str>) -> i64 {
  page.and_then(|p| p.parse().ok()).unwrap_or(1).max(1)
}
