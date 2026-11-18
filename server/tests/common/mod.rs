//! A test harness: the real router over a fresh database and site
//! directory, a capturing mailer, and a cookie-keeping client that talks to
//! the editor host unless told otherwise.

#![allow(dead_code)]

use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::header::{CONTENT_TYPE, COOKIE, HOST, LOCATION, SET_COOKIE};
use axum::http::request::Builder;
use axum::http::{HeaderMap, Method, Request, StatusCode};
use serde_json::{Value, json};
use tempfile::TempDir;
use tower::ServiceExt;

use easl_site_server::app;
use easl_site_server::config::{Config, Limit, RateLimits};
use easl_site_server::db;
use easl_site_server::mail::{Email, Mailer};
use easl_site_server::state::AppState;
use easl_site_server::text::url_encode;
use easl_site_server::users;

/// The editor (app) origin, which `Client`s send as their `Origin`.
pub const ORIGIN: &str = "http://editor.test.local";
/// The docs origin.
pub const DOCS_ORIGIN: &str = "http://test.local";
/// The editor host, which requests go to by default.
pub const EDITOR_HOST: &str = "editor.test.local";
/// The docs host.
pub const DOCS_HOST: &str = "test.local";
pub const PASSWORD: &str = "correct horse battery";

/// Limits high enough that ordinary tests never hit them.
pub fn relaxed_limits() -> RateLimits {
  let generous = Limit::new(10_000, 3600);
  RateLimits {
    auth: generous,
    comments: generous,
    sketch_creates: generous,
    reports: generous,
    verification_emails: generous,
  }
}

pub struct TestApp {
  pub router: Router,
  pub state: AppState,
  pub mail: Arc<Mutex<Vec<Email>>>,
  _dir: TempDir,
}

impl TestApp {
  pub async fn new() -> Self {
    Self::with_config(|_| {}).await
  }

  /// An app whose configuration `adjust` can change first.
  pub async fn with_config(adjust: impl FnOnce(&mut Config)) -> Self {
    Self::build(TempDir::new().expect("temp dir"), adjust).await
  }

  /// An app over the database already at [`database_path`] in `dir` (made
  /// by a test, say, with older migrations), migrating it as the server
  /// does at startup.
  pub async fn with_database_in(dir: TempDir) -> Self {
    Self::build(dir, |_| {}).await
  }

  async fn build(dir: TempDir, adjust: impl FnOnce(&mut Config)) -> Self {
    let site = dir.path().join("site");
    for sub in ["editor", "easl-web"] {
      fs::create_dir_all(site.join(sub)).expect("site dir");
    }
    fs::write(site.join("index.html"), "<h1>landing</h1>").expect("index");
    fs::write(site.join("guide.html"), "<h1>guide</h1>").expect("guide");
    fs::write(site.join("easl-web").join("runtime.wasm"), b"\0asm")
      .expect("wasm");
    fs::write(site.join("editor").join("editor.js"), "// editor")
      .expect("editor");
    fs::write(
      site.join("editor").join("manifest.json"),
      r#"{"version": "abc123", "monaco_version": "0.99.0"}"#,
    )
    .expect("manifest");
    let mut config = Config::new(site, ORIGIN, DOCS_ORIGIN, false);
    config.limits = relaxed_limits();
    adjust(&mut config);
    let pool = db::open(&database_path(&dir)).await.expect("db");
    let (mailer, mail) = Mailer::capture();
    let state = AppState::new(config, pool, mailer).expect("state");
    TestApp {
      router: app::router(state.clone()),
      state,
      mail,
      _dir: dir,
    }
  }

  /// Where the app's database lives.
  pub fn database_path(&self) -> PathBuf {
    database_path(&self._dir)
  }

  /// A signed-out client of the editor host.
  pub fn client(&self) -> Client<'_> {
    Client {
      app: self,
      cookie: None,
      host: EDITOR_HOST,
    }
  }

  /// A client of the docs host.
  pub fn docs_client(&self) -> Client<'_> {
    Client {
      host: DOCS_HOST,
      ..self.client()
    }
  }

  /// Writes `editor/examples.json` into the site directory.
  pub fn write_examples(&self, examples: Value) {
    let path = self.state.config.site_dir.join("editor/examples.json");
    fs::write(path, examples.to_string()).expect("examples.json");
  }

  /// The most recent email sent to `to`.
  pub fn last_email_to(&self, to: &str) -> Option<Email> {
    self
      .mail
      .lock()
      .expect("mail lock")
      .iter()
      .rev()
      .find(|email| email.to == to)
      .cloned()
  }

  /// The site path of the first link in the most recent email to `to`.
  pub fn link_in_email(&self, to: &str) -> String {
    let email = self.last_email_to(to).expect("an email");
    let start = email.body.find(ORIGIN).expect("a link") + ORIGIN.len();
    email.body[start..]
      .split_whitespace()
      .next()
      .expect("a path")
      .to_string()
  }

  /// Signs up a user and verifies their email, returning a signed-in
  /// client.
  pub async fn user(&self, username: &str) -> Client<'_> {
    let mut client = self.unverified_user(username).await;
    let link = self.link_in_email(&email_of(username));
    let response = client.get(&link).await;
    assert_eq!(response.status, StatusCode::OK, "verifying {username}");
    client
  }

  /// Signs up a user without verifying their email.
  pub async fn unverified_user(&self, username: &str) -> Client<'_> {
    let mut client = self.client();
    let response = client
      .post_form(
        "/signup",
        &[
          ("username", username),
          ("email", &email_of(username)),
          ("password", PASSWORD),
        ],
      )
      .await;
    assert_eq!(response.status, StatusCode::OK, "signing up {username}");
    assert!(client.cookie.is_some(), "signed in after signup");
    client
  }

  /// Signs up a verified user and makes them an admin.
  pub async fn admin(&self, username: &str) -> Client<'_> {
    let client = self.user(username).await;
    users::set_admin(&self.state.db, username, true)
      .await
      .expect("grant admin");
    client
  }
}

/// Where a test app's database lives in its directory.
pub fn database_path(dir: &TempDir) -> PathBuf {
  dir.path().join("test.db")
}

pub fn email_of(username: &str) -> String {
  format!("{username}@example.com")
}

/// A response with its body read.
pub struct TestResponse {
  pub status: StatusCode,
  pub headers: HeaderMap,
  pub body: Vec<u8>,
}

impl TestResponse {
  pub fn text(&self) -> String {
    String::from_utf8_lossy(&self.body).into_owned()
  }

  /// The body with the entities autoescaping produces for `'` and `/`
  /// decoded, for matching readable text. Markup stays escaped.
  pub fn page(&self) -> String {
    self.text().replace("&#x27;", "'").replace("&#x2f;", "/")
  }

  /// The page's text as a reader sees it: tags dropped, whitespace runs
  /// collapsed, and common entities decoded, for matching sentences that
  /// span links.
  pub fn visible_text(&self) -> String {
    let mut text = String::new();
    let mut in_tag = false;
    for c in self.text().chars() {
      match c {
        '<' => in_tag = true,
        '>' if in_tag => in_tag = false,
        _ if !in_tag => text.push(c),
        _ => {}
      }
    }
    text
      .replace("&#x27;", "'")
      .replace("&#39;", "'")
      .replace("&#x2f;", "/")
      .replace("&quot;", "\"")
      .replace("&lt;", "<")
      .replace("&gt;", ">")
      .replace("&amp;", "&")
      .split_whitespace()
      .collect::<Vec<_>>()
      .join(" ")
  }

  pub fn json(&self) -> Value {
    serde_json::from_slice(&self.body).unwrap_or_else(|e| {
      panic!("not JSON ({e}): {}", self.text());
    })
  }

  pub fn header(&self, name: &str) -> Option<String> {
    self
      .headers
      .get(name)
      .and_then(|v| v.to_str().ok())
      .map(str::to_string)
  }

  pub fn location(&self) -> String {
    self.header(LOCATION.as_str()).expect("a Location header")
  }

  /// The `page_json` an editor page embeds.
  pub fn page_json(&self) -> Value {
    let text = self.text();
    let marker = "<script id=\"page-data\" type=\"application/json\">";
    let start = text.find(marker).expect("page data") + marker.len();
    let end = start + text[start..].find("</script>").expect("end");
    serde_json::from_str(&text[start..end]).expect("page JSON")
  }
}

/// A browser-like client: keeps the session cookie, sends its host as
/// `Host` when a request names none, and sends the app's `Origin` on
/// state-changing requests.
pub struct Client<'a> {
  app: &'a TestApp,
  pub cookie: Option<String>,
  pub host: &'static str,
}

impl Client<'_> {
  pub async fn send(&mut self, request: Request<Body>) -> TestResponse {
    let mut request = request;
    if !request.headers().contains_key(HOST) {
      request
        .headers_mut()
        .insert(HOST, self.host.parse().expect("host header"));
    }
    if let Some(cookie) = &self.cookie {
      request
        .headers_mut()
        .insert(COOKIE, cookie.parse().expect("cookie header"));
    }
    let response = self
      .app
      .router
      .clone()
      .oneshot(request)
      .await
      .expect("infallible");
    let status = response.status();
    let headers = response.headers().clone();
    for value in headers.get_all(SET_COOKIE) {
      let value = value.to_str().expect("cookie");
      let pair = value.split(';').next().unwrap_or_default();
      if let Some(token) = pair.strip_prefix("easl_session=") {
        self.cookie = if token.is_empty() {
          None
        } else {
          Some(pair.to_string())
        };
      }
    }
    let body = to_bytes(response.into_body(), usize::MAX)
      .await
      .expect("body")
      .to_vec();
    TestResponse {
      status,
      headers,
      body,
    }
  }

  fn builder(method: Method, path: &str) -> Builder {
    let builder = Request::builder().method(method.clone()).uri(path);
    if method == Method::GET || method == Method::HEAD {
      builder
    } else {
      builder.header("origin", ORIGIN)
    }
  }

  pub async fn get(&mut self, path: &str) -> TestResponse {
    self
      .send(
        Self::builder(Method::GET, path)
          .body(Body::empty())
          .expect("request"),
      )
      .await
  }

  pub async fn post_form(
    &mut self,
    path: &str,
    fields: &[(&str, &str)],
  ) -> TestResponse {
    let body = fields
      .iter()
      .map(|(k, v)| format!("{k}={}", url_encode(v)))
      .collect::<Vec<_>>()
      .join("&");
    self
      .send(
        Self::builder(Method::POST, path)
          .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
          .body(Body::from(body))
          .expect("request"),
      )
      .await
  }

  /// A JSON API request (`body` of `Value::Null` sends no body).
  pub async fn api(
    &mut self,
    method: Method,
    path: &str,
    body: Value,
  ) -> TestResponse {
    let builder = Self::builder(method, path);
    let request = if body.is_null() {
      builder.body(Body::empty())
    } else {
      builder
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
    };
    self.send(request.expect("request")).await
  }

  pub async fn raw(
    &mut self,
    method: Method,
    path: &str,
    content_type: &str,
    body: Vec<u8>,
  ) -> TestResponse {
    self
      .send(
        Self::builder(method, path)
          .header(CONTENT_TYPE, content_type)
          .body(Body::from(body))
          .expect("request"),
      )
      .await
  }

  /// Creates a sketch with one file, returning its JSON.
  pub async fn create_sketch(
    &mut self,
    title: &str,
    visibility: &str,
  ) -> Value {
    let response = self
      .api(
        Method::POST,
        "/api/sketches",
        json!({
          "title": title,
          "visibility": visibility,
          "files": [{"path": "main.easl", "content": "(defn main [])"}],
        }),
      )
      .await;
    assert_eq!(response.status, StatusCode::CREATED, "{}", response.text());
    response.json()
  }
}
