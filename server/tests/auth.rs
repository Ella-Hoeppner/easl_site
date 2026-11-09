//! Accounts: signing up, in, and out; sessions; verification; resets;
//! settings.

mod common;

use axum::http::{Method, StatusCode};
use serde_json::Value;

use common::{PASSWORD, TestApp, email_of};

#[tokio::test]
async fn signup_signs_in_and_sends_verification() {
  let app = TestApp::new().await;
  let mut client = app.unverified_user("alice").await;
  let me = client.api(Method::GET, "/api/me", Value::Null).await.json();
  assert_eq!(me["user"]["username"], "alice");
  assert_eq!(me["user"]["display_name"], "alice");
  assert_eq!(me["user"]["avatar_url"], "/u/alice/avatar.svg");
  assert_eq!(me["user"]["email_verified"], false);
  assert_eq!(me["user"]["is_admin"], false);

  let email = app.last_email_to(&email_of("alice")).expect("email");
  assert!(email.body.contains("/verify/"));
  let link = app.link_in_email(&email_of("alice"));
  let page = client.get(&link).await;
  assert_eq!(page.status, StatusCode::OK);
  assert!(page.text().contains("Email verified"));
  let me = client.api(Method::GET, "/api/me", Value::Null).await.json();
  assert_eq!(me["user"]["email_verified"], true);

  // The link works once.
  let again = client.get(&link).await;
  assert_eq!(again.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn signup_validation_and_conflicts() {
  let app = TestApp::new().await;
  app.unverified_user("alice").await;
  let mut client = app.client();
  let cases = [
    ("ab", "x@example.com", PASSWORD, "3 to 24"),
    ("admin", "x@example.com", PASSWORD, "isn't available"),
    ("bad name", "x@example.com", PASSWORD, "letters, digits"),
    ("newuser", "not-an-email", PASSWORD, "valid email"),
    ("newuser", "x@example.com", "short", "at least 10"),
    ("ALICE", "x@example.com", PASSWORD, "taken"),
    ("newuser", "ALICE@example.com", PASSWORD, "already exists"),
  ];
  for (username, email, password, message) in cases {
    let response = client
      .post_form(
        "/signup",
        &[
          ("username", username),
          ("email", email),
          ("password", password),
        ],
      )
      .await;
    assert!(
      response.status.is_client_error(),
      "{username} {email}: {}",
      response.status
    );
    assert!(
      response.page().contains(message),
      "{username}: expected {message:?}"
    );
    assert!(client.cookie.is_none());
  }
}

#[tokio::test]
async fn login_logout_and_wrong_password() {
  let app = TestApp::new().await;
  app.unverified_user("alice").await;
  let mut client = app.client();

  let wrong = client
    .post_form(
      "/login",
      &[("login", "alice"), ("password", "wrong password")],
    )
    .await;
  assert_eq!(wrong.status, StatusCode::UNAUTHORIZED);
  let missing = client
    .post_form("/login", &[("login", "nobody"), ("password", PASSWORD)])
    .await;
  assert_eq!(missing.status, StatusCode::UNAUTHORIZED);
  // The two failures read the same.
  assert!(wrong.page().contains("don't match an account"));
  assert!(missing.page().contains("don't match an account"));
  assert!(client.cookie.is_none());

  let ok = client
    .post_form(
      "/login",
      &[
        ("login", "ALICE@example.com"),
        ("password", PASSWORD),
        ("next", "/explore"),
      ],
    )
    .await;
  assert_eq!(ok.status, StatusCode::SEE_OTHER);
  assert_eq!(ok.location(), "/explore");
  let cookie = ok.header("set-cookie").expect("cookie");
  assert!(cookie.contains("HttpOnly"));
  assert!(cookie.contains("SameSite=Lax"));
  assert!(cookie.contains("Secure"));
  assert!(cookie.contains("Path=/"));

  let me = client.api(Method::GET, "/api/me", Value::Null).await.json();
  assert_eq!(me["user"]["username"], "alice");

  let out = client.post_form("/logout", &[]).await;
  assert_eq!(out.status, StatusCode::SEE_OTHER);
  assert!(client.cookie.is_none());
  let me = client.api(Method::GET, "/api/me", Value::Null).await.json();
  assert!(me["user"].is_null());
}

#[tokio::test]
async fn login_next_is_same_site_only() {
  let app = TestApp::new().await;
  app.unverified_user("alice").await;
  for next in ["https://evil.example", "//evil.example", "/\\evil.example"] {
    let mut client = app.client();
    let response = client
      .post_form(
        "/login",
        &[("login", "alice"), ("password", PASSWORD), ("next", next)],
      )
      .await;
    assert_eq!(response.location(), "/", "{next}");
  }
}

#[tokio::test]
async fn logged_out_session_cookie_stops_working() {
  let app = TestApp::new().await;
  let mut client = app.unverified_user("alice").await;
  let stolen = client.cookie.clone();
  client.post_form("/logout", &[]).await;
  let mut thief = app.client();
  thief.cookie = stolen;
  let me = thief.api(Method::GET, "/api/me", Value::Null).await;
  assert!(me.json()["user"].is_null());
  // The dead cookie is cleared.
  assert!(
    me.header("set-cookie")
      .expect("clear")
      .contains("Max-Age=0")
  );
}

#[tokio::test]
async fn garbage_cookies_are_signed_out() {
  let app = TestApp::new().await;
  let mut client = app.client();
  client.cookie = Some("easl_session=nonsense".into());
  let me = client.api(Method::GET, "/api/me", Value::Null).await.json();
  assert!(me["user"].is_null());
}

#[tokio::test]
async fn signed_out_pages_redirect_to_sign_in() {
  let app = TestApp::new().await;
  let mut client = app.client();
  let response = client.get("/settings").await;
  assert_eq!(response.status, StatusCode::SEE_OTHER);
  assert_eq!(response.location(), "/login?next=/settings");
  let api = client
    .api(Method::GET, "/api/me/sketches", Value::Null)
    .await;
  assert_eq!(api.status, StatusCode::UNAUTHORIZED);
  assert!(api.json()["error"].is_string());
}

#[tokio::test]
async fn password_reset_flow() {
  let app = TestApp::new().await;
  let mut old_session = app.unverified_user("alice").await;
  let mut client = app.client();
  let response = client
    .post_form("/forgot", &[("email", &email_of("alice"))])
    .await;
  assert_eq!(response.status, StatusCode::OK);
  assert!(response.text().contains("If an account uses"));
  // Unknown addresses get the same page and no email.
  let unknown = client
    .post_form("/forgot", &[("email", "nobody@example.com")])
    .await;
  assert_eq!(unknown.status, StatusCode::OK);
  assert!(app.last_email_to("nobody@example.com").is_none());

  let link = app.link_in_email(&email_of("alice"));
  assert!(link.starts_with("/reset/"));
  let form = client.get(&link).await;
  assert_eq!(form.status, StatusCode::OK);

  let mismatch = client
    .post_form(&link, &[("password", "new password 1"), ("confirm", "x")])
    .await;
  assert_eq!(mismatch.status, StatusCode::BAD_REQUEST);

  let done = client
    .post_form(
      &link,
      &[
        ("password", "new password 1"),
        ("confirm", "new password 1"),
      ],
    )
    .await;
  assert_eq!(done.status, StatusCode::SEE_OTHER);
  assert!(client.cookie.is_some(), "signed in by the reset");

  // Other sessions ended; the link is used up; the new password works.
  let me = old_session.api(Method::GET, "/api/me", Value::Null).await;
  assert!(me.json()["user"].is_null());
  assert_eq!(client.get(&link).await.status, StatusCode::NOT_FOUND);
  let mut fresh = app.client();
  let old = fresh
    .post_form("/login", &[("login", "alice"), ("password", PASSWORD)])
    .await;
  assert_eq!(old.status, StatusCode::UNAUTHORIZED);
  let new = fresh
    .post_form(
      "/login",
      &[("login", "alice"), ("password", "new password 1")],
    )
    .await;
  assert_eq!(new.status, StatusCode::SEE_OTHER);
  // Following the emailed link proved the address.
  let me = fresh.api(Method::GET, "/api/me", Value::Null).await.json();
  assert_eq!(me["user"]["email_verified"], true);
}

#[tokio::test]
async fn bad_reset_and_verify_tokens() {
  let app = TestApp::new().await;
  let mut client = app.client();
  for path in [
    "/reset/nope",
    "/verify/nope",
    &format!("/reset/{}", "a".repeat(43)),
  ] {
    assert_eq!(
      client.get(path).await.status,
      StatusCode::NOT_FOUND,
      "{path}"
    );
  }
}

#[tokio::test]
async fn settings_profile_password_and_email() {
  let app = TestApp::new().await;
  let mut client = app.user("alice").await;
  let mut other_device = app.client();
  other_device
    .post_form("/login", &[("login", "alice"), ("password", PASSWORD)])
    .await;

  let saved = client
    .post_form(
      "/settings/profile",
      &[("display_name", "Alice L."), ("bio", "Hello <b>there</b>")],
    )
    .await;
  assert_eq!(saved.location(), "/settings?saved=profile");
  let me = client.api(Method::GET, "/api/me", Value::Null).await.json();
  assert_eq!(me["user"]["display_name"], "Alice L.");
  let profile = client.get("/u/alice").await.text();
  assert!(profile.contains("Hello &lt;b&gt;there&lt;/b&gt;"));

  let too_long = client
    .post_form("/settings/profile", &[("display_name", &"x".repeat(51))])
    .await;
  assert_eq!(too_long.status, StatusCode::BAD_REQUEST);

  let wrong = client
    .post_form(
      "/settings/password",
      &[
        ("current", "nope nope nope"),
        ("password", "brand new pw"),
        ("confirm", "brand new pw"),
      ],
    )
    .await;
  assert_eq!(wrong.status, StatusCode::FORBIDDEN);
  let changed = client
    .post_form(
      "/settings/password",
      &[
        ("current", PASSWORD),
        ("password", "brand new pw"),
        ("confirm", "brand new pw"),
      ],
    )
    .await;
  assert_eq!(changed.location(), "/settings?saved=password");
  // This session survives; the other device's doesn't.
  let me = client.api(Method::GET, "/api/me", Value::Null).await.json();
  assert_eq!(me["user"]["username"], "alice");
  let gone = other_device.api(Method::GET, "/api/me", Value::Null).await;
  assert!(gone.json()["user"].is_null());

  let email = client
    .post_form(
      "/settings/email",
      &[
        ("email", "alice2@example.com"),
        ("password", "brand new pw"),
      ],
    )
    .await;
  assert_eq!(email.location(), "/settings?saved=email");
  let me = client.api(Method::GET, "/api/me", Value::Null).await.json();
  assert_eq!(me["user"]["email_verified"], false);
  assert!(app.last_email_to("alice2@example.com").is_some());
}

#[tokio::test]
async fn email_change_invalidates_old_links() {
  let app = TestApp::new().await;
  let mut client = app.unverified_user("alice").await;
  let old_link = app.link_in_email(&email_of("alice"));
  client
    .post_form(
      "/settings/email",
      &[("email", "alice2@example.com"), ("password", PASSWORD)],
    )
    .await;
  // A link sent to the old address can't verify the new one.
  assert_eq!(client.get(&old_link).await.status, StatusCode::NOT_FOUND);
  let new_link = app.link_in_email("alice2@example.com");
  assert_eq!(client.get(&new_link).await.status, StatusCode::OK);
}

#[tokio::test]
async fn delete_account() {
  let app = TestApp::new().await;
  let mut alice = app.user("alice").await;
  let mut bob = app.user("bob").await;
  let sketch = bob.create_sketch("Bob's", "public").await;
  let id = sketch["id"].as_str().expect("id");
  alice
    .api(
      Method::POST,
      &format!("/api/sketches/{id}/like"),
      Value::Null,
    )
    .await;
  alice
    .api(
      Method::POST,
      &format!("/api/sketches/{id}/comments"),
      serde_json::json!({"body": "hi"}),
    )
    .await;

  let unconfirmed = alice
    .post_form("/settings/delete", &[("password", PASSWORD)])
    .await;
  assert_eq!(unconfirmed.status, StatusCode::BAD_REQUEST);
  let deleted = alice
    .post_form(
      "/settings/delete",
      &[("password", PASSWORD), ("confirm", "yes")],
    )
    .await;
  assert_eq!(deleted.status, StatusCode::OK);
  assert!(alice.cookie.is_none());
  assert_eq!(
    app.client().get("/u/alice").await.status,
    StatusCode::NOT_FOUND
  );

  // Counts follow the cascade.
  let sketch = bob
    .api(Method::GET, &format!("/api/sketches/{id}"), Value::Null)
    .await
    .json();
  assert_eq!(sketch["like_count"], 0);
  assert_eq!(sketch["comment_count"], 0);
}

#[tokio::test]
async fn dev_mode_shows_the_verification_link() {
  let app = TestApp::with_config(|config| config.dev = true).await;
  let mut client = app.client();
  let response = client
    .post_form(
      "/signup",
      &[
        ("username", "alice"),
        ("email", "a@example.com"),
        ("password", PASSWORD),
      ],
    )
    .await;
  let text = response.page();
  assert!(text.contains("Dev mode"));
  assert!(text.contains("/verify/"));
  // Dev cookies aren't Secure, so they work over plain http.
  assert!(
    !response
      .header("set-cookie")
      .expect("cookie")
      .contains("Secure")
  );
}
