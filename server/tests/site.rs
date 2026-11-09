//! Explore, profiles, the editor pages, the static site, and request-level
//! protections (CSRF, headers, rate limits).

mod common;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use serde_json::{Value, json};

use common::{ORIGIN, PASSWORD, TestApp};
use easl_site_server::config::Limit;

fn id_of(value: &Value) -> String {
  value["id"].as_str().expect("id").to_string()
}

/// The sketch titles on a page, in order.
fn card_titles(html: &str) -> Vec<String> {
  html
    .split("class=\"card-title\"><a href=\"")
    .skip(1)
    .filter_map(|rest| {
      let start = rest.find('>')? + 1;
      let end = rest.find("</a>")?;
      Some(rest[start..end].to_string())
    })
    .collect()
}

#[tokio::test]
async fn explore_sorting_search_and_pages() {
  let app = TestApp::new().await;
  let mut alice = app.user("alice").await;
  let old = id_of(&alice.create_sketch("Old favorite", "public").await);
  let mid = id_of(&alice.create_sketch("Middle", "public").await);
  let new = id_of(&alice.create_sketch("Newest fractal", "public").await);
  for (id, published) in [(&old, 1000), (&mid, 2000), (&new, 3000)] {
    sqlx::query("UPDATE sketches SET published_at = ? WHERE id = ?")
      .bind(published)
      .bind(id)
      .execute(&app.state.db)
      .await
      .expect("update");
  }
  // "Old favorite" has the most likes, all long ago; "Middle" has a recent
  // one.
  let mut likers = Vec::new();
  for name in ["bob", "carol", "dave"] {
    likers.push(app.user(name).await);
  }
  for liker in &mut likers {
    liker
      .api(
        Method::POST,
        &format!("/api/sketches/{old}/like"),
        Value::Null,
      )
      .await;
  }
  sqlx::query("UPDATE likes SET created_at = 0")
    .execute(&app.state.db)
    .await
    .expect("age likes");
  likers[0]
    .api(
      Method::POST,
      &format!("/api/sketches/{mid}/like"),
      Value::Null,
    )
    .await;

  let mut client = app.client();
  let titles = |html: String| card_titles(&html);
  assert_eq!(
    titles(client.get("/explore").await.text()),
    ["Newest fractal", "Middle", "Old favorite"]
  );
  assert_eq!(
    titles(client.get("/explore?sort=popular").await.text()),
    ["Old favorite", "Middle", "Newest fractal"]
  );
  assert_eq!(
    titles(client.get("/explore?sort=trending").await.text()),
    ["Middle", "Old favorite", "Newest fractal"]
  );
  assert_eq!(
    titles(client.get("/explore?q=FRACTAL").await.text()),
    ["Newest fractal"]
  );
  assert_eq!(titles(client.get("/explore?q=alice").await.text()).len(), 3);
  // LIKE wildcards are literal.
  assert!(titles(client.get("/explore?q=%25").await.text()).is_empty());
  let none = client.get("/explore?q=zzz").await.page();
  assert!(none.contains("No sketches match"));
  // Search text is escaped back into the page.
  let echoed = client.get("/explore?q=%3Cscript%3E").await.text();
  assert!(!echoed.contains("<script>"));

  for i in 0..25 {
    alice.create_sketch(&format!("Bulk {i:02}"), "public").await;
  }
  let first = client.get("/explore").await.page();
  assert_eq!(card_titles(&first).len(), 24);
  assert!(first.contains("/explore?sort=new&amp;page=2"));
  let second = client.get("/explore?page=2").await.text();
  assert_eq!(card_titles(&second).len(), 4);
  assert_eq!(client.get("/explore?page=-3").await.status, StatusCode::OK);
}

#[tokio::test]
async fn profile_pages() {
  let app = TestApp::new().await;
  let mut alice = app.user("alice").await;
  alice
    .post_form(
      "/settings/profile",
      &[
        ("display_name", "Alice"),
        ("bio", "Shaders at https://example.com/a?b=1&c=2."),
      ],
    )
    .await;
  alice.create_sketch("Visible", "public").await;
  let mut client = app.client();
  let page = client.get("/u/ALICE").await;
  assert_eq!(page.status, StatusCode::OK);
  let text = page.page();
  assert!(text.contains("@alice"));
  assert!(text.contains(
    "<a href=\"https://example.com/a?b=1&amp;c=2\" rel=\"nofollow ugc noopener\">"
  ));
  assert!(text.contains("Visible"));
  assert!(!text.contains("Edit profile"));
  assert!(alice.get("/u/alice").await.page().contains("Edit profile"));
  assert_eq!(client.get("/u/nobody").await.status, StatusCode::NOT_FOUND);
  assert_eq!(
    client.get("/u/bad%20name").await.status,
    StatusCode::NOT_FOUND
  );
  assert_eq!(
    client.get("/u/alice?tab=playlists").await.status,
    StatusCode::OK
  );
  assert_eq!(
    client.get("/u/alice?tab=likes").await.status,
    StatusCode::OK
  );
}

#[tokio::test]
async fn header_shows_who_is_signed_in() {
  let app = TestApp::new().await;
  let anonymous = app.client().get("/explore").await.page();
  assert!(anonymous.contains("/login?next=/explore"));
  assert!(anonymous.contains("Sign up"));
  let mut alice = app.user("alice").await;
  let signed_in = alice.get("/explore").await.page();
  assert!(signed_in.contains("/u/alice/avatar.svg"));
  assert!(signed_in.contains("action=\"/logout\""));
  assert!(!signed_in.contains("Sign up"));
}

#[tokio::test]
async fn editor_pages() {
  let app = TestApp::new().await;
  let mut client = app.client();
  let playground = client.get("/playground?example=raymarch-materials").await;
  assert_eq!(playground.status, StatusCode::OK);
  let data = playground.page_json();
  assert_eq!(data["mode"], "playground");
  assert_eq!(data["example"], "raymarch-materials");
  assert!(data["user"].is_null());
  assert!(data["sketch"].is_null());

  let new = client.get("/new").await.page_json();
  assert_eq!(new["mode"], "new");
  assert!(new["example"].is_null());

  let redirect = client.get("/playground.html?example=landing").await;
  assert_eq!(redirect.status, StatusCode::PERMANENT_REDIRECT);
  assert_eq!(redirect.location(), "/playground?example=landing");
}

#[tokio::test]
async fn static_site_serving() {
  let app = TestApp::new().await;
  let mut client = app.client();
  let index = client.get("/").await;
  assert_eq!(index.status, StatusCode::OK);
  assert!(index.text().contains("landing"));
  assert_eq!(index.header("cache-control").as_deref(), Some("no-cache"));
  let guide = client.get("/guide.html").await;
  assert_eq!(guide.status, StatusCode::OK);
  let versioned = client.get("/guide.html?v=123").await;
  assert_eq!(
    versioned.header("cache-control").as_deref(),
    Some("public, max-age=31536000, immutable")
  );
  let wasm = client.get("/runtime.wasm").await;
  assert_eq!(
    wasm.header("content-type").as_deref(),
    Some("application/wasm")
  );
  let missing = client.get("/nope.html").await;
  assert_eq!(missing.status, StatusCode::NOT_FOUND);
  assert!(missing.page().contains("Nothing here"));
  let traversal = client.get("/../Cargo.toml").await;
  assert_ne!(traversal.status, StatusCode::OK);

  let css = client.get("/static/app.css").await;
  assert_eq!(css.status, StatusCode::OK);
  assert!(
    css
      .header("content-type")
      .expect("type")
      .starts_with("text/css")
  );
  let explore = client.get("/explore").await.page();
  let start = explore.find("/static/app.css?v=").expect("versioned css");
  let url = &explore[start..start + explore[start..].find('"').expect("end")];
  let cached = client.get(url).await;
  assert!(
    cached
      .header("cache-control")
      .expect("cache")
      .contains("immutable")
  );
  assert_eq!(
    client.get("/static/nope.css").await.status,
    StatusCode::NOT_FOUND
  );

  let api_missing = client.get("/api/nope").await;
  assert_eq!(api_missing.status, StatusCode::NOT_FOUND);
  assert_eq!(api_missing.json()["error"], "Not found.");
}

#[tokio::test]
async fn security_headers() {
  let app = TestApp::new().await;
  let mut client = app.client();
  for path in ["/explore", "/", "/api/me"] {
    let response = client.get(path).await;
    assert_eq!(
      response.header("x-content-type-options").as_deref(),
      Some("nosniff")
    );
    assert_eq!(
      response.header("referrer-policy").as_deref(),
      Some("strict-origin-when-cross-origin")
    );
    assert_eq!(
      response.header("x-frame-options").as_deref(),
      Some("SAMEORIGIN")
    );
  }
  let page = client.get("/explore").await;
  assert_eq!(page.header("cache-control").as_deref(), Some("no-store"));
  assert!(page.header("content-security-policy").is_some());
  let avatar = client.get("/u/someone/avatar.svg").await;
  assert_eq!(avatar.status, StatusCode::OK);
  assert_eq!(
    avatar.header("content-type").as_deref(),
    Some("image/svg+xml")
  );
  assert!(
    avatar
      .header("cache-control")
      .expect("cache")
      .contains("max-age")
  );
  assert!(avatar.text().starts_with("<svg"));
}

#[tokio::test]
async fn csrf_protection() {
  let app = TestApp::new().await;
  let mut alice = app.user("alice").await;
  let cookie = alice.cookie.clone().expect("cookie");
  let body =
    json!({"title": "x", "files": [{"path": "a.easl", "content": ""}]});
  let attempt = |origin: Option<&str>, fetch_site: Option<&str>| {
    let mut builder = Request::builder()
      .method(Method::POST)
      .uri("/api/sketches")
      .header("content-type", "application/json")
      .header("cookie", cookie.clone());
    if let Some(origin) = origin {
      builder = builder.header("origin", origin);
    }
    if let Some(site) = fetch_site {
      builder = builder.header("sec-fetch-site", site);
    }
    builder.body(Body::from(body.to_string())).expect("request")
  };
  let cross = alice
    .send(attempt(Some("https://evil.example"), None))
    .await;
  assert_eq!(cross.status, StatusCode::FORBIDDEN);
  assert!(cross.json()["error"].is_string());
  let none = alice.send(attempt(None, None)).await;
  assert_eq!(none.status, StatusCode::FORBIDDEN);
  let cross_site = alice.send(attempt(None, Some("cross-site"))).await;
  assert_eq!(cross_site.status, StatusCode::FORBIDDEN);
  let localhost = alice
    .send(attempt(Some("http://localhost:8000"), None))
    .await;
  assert_eq!(
    localhost.status,
    StatusCode::FORBIDDEN,
    "dev origins only in dev"
  );
  let same = alice.send(attempt(None, Some("same-origin"))).await;
  assert_eq!(same.status, StatusCode::CREATED);
  let ok = alice.send(attempt(Some(ORIGIN), None)).await;
  assert_eq!(ok.status, StatusCode::CREATED);

  // Forms too: a cross-site logout is refused.
  let logout = Request::builder()
    .method(Method::POST)
    .uri("/logout")
    .header("origin", "https://evil.example")
    .header("cookie", cookie.clone())
    .body(Body::empty())
    .expect("request");
  assert_eq!(alice.send(logout).await.status, StatusCode::FORBIDDEN);
  let me = alice.api(Method::GET, "/api/me", Value::Null).await.json();
  assert_eq!(me["user"]["username"], "alice");
}

#[tokio::test]
async fn dev_origins_in_dev_mode() {
  let app = TestApp::with_config(|config| config.dev = true).await;
  let mut client = app.client();
  for (origin, status) in [
    ("http://localhost:8100", StatusCode::OK),
    ("http://127.0.0.1:3000", StatusCode::OK),
    ("http://localhost.evil.example", StatusCode::FORBIDDEN),
    ("http://localhost:", StatusCode::FORBIDDEN),
  ] {
    let request = Request::builder()
      .method(Method::POST)
      .uri("/signup")
      .header("origin", origin)
      .header("content-type", "application/x-www-form-urlencoded")
      .body(Body::from("username=x"))
      .expect("request");
    let response = client.send(request).await;
    let refused = response.status == StatusCode::FORBIDDEN;
    assert_eq!(refused, status == StatusCode::FORBIDDEN, "{origin}");
  }
}

#[tokio::test]
async fn auth_rate_limit_per_address() {
  let app = TestApp::with_config(|config| {
    config.limits.auth = Limit::new(3, 600);
    config.trust_proxy = true;
  })
  .await;
  app.unverified_user("alice").await;
  let attempt = |ip: &str| {
    Request::builder()
      .method(Method::POST)
      .uri("/login")
      .header("origin", ORIGIN)
      .header("x-forwarded-for", format!("10.9.9.9, {ip}"))
      .header("content-type", "application/x-www-form-urlencoded")
      .body(Body::from(format!(
        "login=alice&password={}",
        "wrong".repeat(3)
      )))
      .expect("request")
  };
  let mut client = app.client();
  for _ in 0..3 {
    let response = client.send(attempt("1.2.3.4")).await;
    assert_eq!(response.status, StatusCode::UNAUTHORIZED);
  }
  let limited = client.send(attempt("1.2.3.4")).await;
  assert_eq!(limited.status, StatusCode::TOO_MANY_REQUESTS);
  assert!(limited.page().contains("Too many requests"));
  // Another address (the last hop is what counts) isn't limited.
  let other = client.send(attempt("5.6.7.8")).await;
  assert_eq!(other.status, StatusCode::UNAUTHORIZED);
  // Even the right password is refused while limited.
  let right = Request::builder()
    .method(Method::POST)
    .uri("/login")
    .header("origin", ORIGIN)
    .header("x-forwarded-for", "1.2.3.4")
    .header("content-type", "application/x-www-form-urlencoded")
    .body(Body::from(format!(
      "login=alice&password={}",
      PASSWORD.replace(' ', "+")
    )))
    .expect("request");
  assert_eq!(
    client.send(right).await.status,
    StatusCode::TOO_MANY_REQUESTS
  );
}

#[tokio::test]
async fn per_user_rate_limits() {
  let app = TestApp::with_config(|config| {
    config.limits.comments = Limit::new(2, 60);
    config.limits.sketch_creates = Limit::new(2, 3600);
    config.limits.reports = Limit::new(1, 3600);
  })
  .await;
  let mut alice = app.user("alice").await;
  let id = id_of(&alice.create_sketch("One", "public").await);
  alice.create_sketch("Two", "public").await;
  let third = alice
    .api(
      Method::POST,
      "/api/sketches",
      json!({"title": "Three", "files": [{"path": "a.easl", "content": ""}]}),
    )
    .await;
  assert_eq!(third.status, StatusCode::TOO_MANY_REQUESTS);
  assert!(
    third.json()["error"]
      .as_str()
      .expect("error")
      .contains("Too many")
  );
  let fork = alice
    .api(
      Method::POST,
      &format!("/api/sketches/{id}/fork"),
      Value::Null,
    )
    .await;
  assert_eq!(fork.status, StatusCode::TOO_MANY_REQUESTS);

  let comments = format!("/api/sketches/{id}/comments");
  for _ in 0..2 {
    let ok = alice
      .api(Method::POST, &comments, json!({"body": "hi"}))
      .await;
    assert_eq!(ok.status, StatusCode::CREATED);
  }
  let limited = alice
    .api(Method::POST, &comments, json!({"body": "hi"}))
    .await;
  assert_eq!(limited.status, StatusCode::TOO_MANY_REQUESTS);
  // Other users have their own budget.
  let mut bob = app.user("bob").await;
  let ok = bob
    .api(Method::POST, &comments, json!({"body": "hi"}))
    .await;
  assert_eq!(ok.status, StatusCode::CREATED);

  let report = json!({"kind": "user", "target": "alice", "reason": ""});
  let ok = bob.api(Method::POST, "/api/reports", report.clone()).await;
  assert_eq!(ok.status, StatusCode::CREATED);
  let limited = bob.api(Method::POST, "/api/reports", report).await;
  assert_eq!(limited.status, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn oversized_bodies_are_refused() {
  let app = TestApp::new().await;
  let mut alice = app.user("alice").await;
  let huge = "x".repeat(3 * 1024 * 1024);
  let response = alice
    .api(
      Method::POST,
      "/api/sketches",
      json!({"title": "t", "files": [{"path": "a.easl", "content": huge}]}),
    )
    .await;
  assert_eq!(response.status, StatusCode::PAYLOAD_TOO_LARGE);
}
