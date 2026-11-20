//! Rate limits on creating things and checking passwords, the limits on
//! sending email (per address, per account, per recipient, and the site's
//! daily budget), and which password-reset links stay valid.

mod common;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};

use common::{Client, PASSWORD, TestApp, TestResponse, email_of};
use easl_site_server::config::{Config, Limit, RateLimits};
use easl_site_server::outbox;
use easl_site_server::revisions::Contents;
use easl_site_server::sketches::{self, NewSketch};
use easl_site_server::users;
use easl_site_server::validate::SketchFile;
use easl_site_server::visibility::Visibility;

const HOUR: u64 = 60 * 60;
const DAY: u64 = 24 * HOUR;

/// How many emails with a subject starting `subject` went to `to`.
fn emails_to(app: &TestApp, to: &str, subject: &str) -> usize {
  app
    .mail
    .lock()
    .expect("mail lock")
    .iter()
    .filter(|email| email.to == to && email.subject.starts_with(subject))
    .count()
}

const RESET: &str = "Reset your";
const VERIFY: &str = "Verify your";

async fn forgot(client: &mut Client<'_>, email: &str) -> TestResponse {
  client.post_form("/forgot", &[("email", email)]).await
}

/// A page's text with an email address replaced, for comparing the pages
/// two addresses get.
fn without(response: &TestResponse, email: &str) -> String {
  response.visible_text().replace(email, "<email>")
}

async fn sign_up(client: &mut Client<'_>, username: &str) -> TestResponse {
  let email = email_of(username);
  let fields = [
    ("username", username),
    ("email", &email),
    ("password", PASSWORD),
  ];
  client.post_form("/signup", &fields).await
}

/// A user's email address, from the database (no API shows it).
async fn email_of_user(app: &TestApp, username: &str) -> String {
  users::by_username(&app.state.db, username)
    .await
    .expect("user")
    .expect("a user")
    .email
}

/// Sets the daily email budget.
fn budget(budget: i64) -> impl FnOnce(&mut Config) {
  move |config| config.daily_email_budget = budget
}

async fn create(client: &mut Client<'_>, title: &str) -> StatusCode {
  client
    .api(
      Method::POST,
      "/api/sketches",
      json!({"title": title, "files": [{"path": "a.easl", "content": ""}]}),
    )
    .await
    .status
}

async fn fork(client: &mut Client<'_>, id: &str) -> StatusCode {
  client
    .api(
      Method::POST,
      &format!("/api/sketches/{id}/fork"),
      Value::Null,
    )
    .await
    .status
}

#[test]
fn default_limits() {
  let limits = RateLimits::default();
  let pair = |limits: [Limit; 2]| {
    limits.map(|limit| (limit.count, limit.window.as_secs()))
  };
  assert_eq!(pair(limits.sketch_creates), [(30, HOUR), (100, DAY)]);
  assert_eq!(pair(limits.reset_emails), [(3, HOUR), (6, DAY)]);
  assert_eq!(pair(limits.verification_emails), [(3, HOUR), (10, DAY)]);
  let one = |limit: Limit| (limit.count, limit.window.as_secs());
  assert_eq!(one(limits.playlist_creates), (20, HOUR));
  assert_eq!(one(limits.password_checks), (10, 600));
  assert_eq!(one(limits.auth), (10, 600));
  assert_eq!(one(limits.signups), (5, HOUR));
  assert_eq!(one(limits.reset_requests), (10, HOUR));
}

#[tokio::test]
async fn sketch_creation_is_limited_per_hour_and_per_day() {
  // The hourly limit, counting forks.
  let app = TestApp::with_config(|config| {
    config.limits.sketch_creates = [Limit::new(3, HOUR), Limit::new(100, DAY)];
  })
  .await;
  let mut alice = app.user("alice").await;
  let id = alice.create_sketch("One", "public").await["id"]
    .as_str()
    .expect("id")
    .to_string();
  assert_eq!(fork(&mut alice, &id).await, StatusCode::CREATED);
  assert_eq!(create(&mut alice, "Three").await, StatusCode::CREATED);
  assert_eq!(
    create(&mut alice, "Four").await,
    StatusCode::TOO_MANY_REQUESTS
  );
  assert_eq!(fork(&mut alice, &id).await, StatusCode::TOO_MANY_REQUESTS);

  // The daily limit, on top of a looser hourly one.
  let app = TestApp::with_config(|config| {
    config.limits.sketch_creates = [Limit::new(100, HOUR), Limit::new(4, DAY)];
  })
  .await;
  let mut alice = app.user("alice").await;
  let id = alice.create_sketch("One", "public").await["id"]
    .as_str()
    .expect("id")
    .to_string();
  assert_eq!(fork(&mut alice, &id).await, StatusCode::CREATED);
  assert_eq!(create(&mut alice, "Three").await, StatusCode::CREATED);
  assert_eq!(fork(&mut alice, &id).await, StatusCode::CREATED);
  assert_eq!(
    create(&mut alice, "Five").await,
    StatusCode::TOO_MANY_REQUESTS
  );
  // Each account has its own.
  let mut bob = app.user("bob").await;
  assert_eq!(create(&mut bob, "Bob's").await, StatusCode::CREATED);
}

#[tokio::test]
async fn playlist_creation_is_limited() {
  let app = TestApp::with_config(|config| {
    config.limits.playlist_creates = Limit::new(2, HOUR);
  })
  .await;
  let mut alice = app.user("alice").await;
  let api = |title: &str| json!({"title": title});
  let form = [("title", "Form"), ("visibility", "private")];
  let first = alice.api(Method::POST, "/api/playlists", api("One")).await;
  assert_eq!(first.status, StatusCode::CREATED);
  let second = alice.post_form("/playlists/new", &form).await;
  assert_eq!(second.status, StatusCode::SEE_OTHER);
  // The API and the form share the limit.
  let third = alice
    .api(Method::POST, "/api/playlists", api("Three"))
    .await;
  assert_eq!(third.status, StatusCode::TOO_MANY_REQUESTS);
  let page = alice.post_form("/playlists/new", &form).await;
  assert_eq!(page.status, StatusCode::TOO_MANY_REQUESTS);
  assert!(page.visible_text().contains("Too many requests"));
  let mut bob = app.user("bob").await;
  let bobs = bob.api(Method::POST, "/api/playlists", api("Bob's")).await;
  assert_eq!(bobs.status, StatusCode::CREATED);
}

#[tokio::test]
async fn settings_password_checks_are_limited_per_account() {
  let app = TestApp::with_config(|config| {
    config.limits.password_checks = Limit::new(3, 600);
  })
  .await;
  let mut alice = app.user("alice").await;
  let wrong = "wrong password!";
  let new_password = [
    ("current", wrong),
    ("password", "brand new pw"),
    ("confirm", "brand new pw"),
  ];
  for _ in 0..2 {
    let response = alice.post_form("/settings/password", &new_password).await;
    assert_eq!(response.status, StatusCode::FORBIDDEN);
  }
  let email = [("email", "alice2@example.com"), ("password", wrong)];
  let response = alice.post_form("/settings/email", &email).await;
  assert_eq!(response.status, StatusCode::FORBIDDEN);

  // All three forms share the limit; the right password doesn't get past.
  let delete = [("password", PASSWORD), ("confirm", "yes")];
  let response = alice.post_form("/settings/delete", &delete).await;
  assert_eq!(response.status, StatusCode::TOO_MANY_REQUESTS);
  assert!(response.visible_text().contains("Too many requests"));
  let right = [
    ("current", PASSWORD),
    ("password", "brand new pw"),
    ("confirm", "brand new pw"),
  ];
  let response = alice.post_form("/settings/password", &right).await;
  assert_eq!(response.status, StatusCode::TOO_MANY_REQUESTS);
  let me = alice.api(Method::GET, "/api/me", Value::Null).await.json();
  assert_eq!(me["user"]["username"], "alice", "not deleted");

  // Another account isn't limited.
  let mut bob = app.user("bob").await;
  let response = bob.post_form("/settings/password", &right).await;
  assert_eq!(response.location(), "/settings?saved=password");
}

#[tokio::test]
async fn signups_are_limited_per_address_apart_from_sign_ins() {
  let app = TestApp::with_config(|config| {
    config.limits.signups = Limit::new(2, HOUR);
    config.limits.auth = Limit::new(3, 600);
  })
  .await;
  let mut client = app.client();
  let response = sign_up(&mut client, "alice").await;
  assert_eq!(response.status, StatusCode::OK);
  // A taken username doesn't use the limit up.
  let mut other = app.client();
  let taken = sign_up(&mut other, "alice").await;
  assert_eq!(taken.status, StatusCode::CONFLICT);
  let response = sign_up(&mut other, "bob").await;
  assert_eq!(response.status, StatusCode::OK);
  let mut third = app.client();
  let limited = sign_up(&mut third, "carol").await;
  assert_eq!(limited.status, StatusCode::TOO_MANY_REQUESTS);
  assert!(limited.visible_text().contains("Too many requests"));
  assert_eq!(emails_to(&app, &email_of("carol"), VERIFY), 0);

  // Signing in from the same address still works.
  let login = third
    .post_form("/login", &[("login", "alice"), ("password", PASSWORD)])
    .await;
  assert_eq!(login.status, StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn reset_emails_are_limited_per_recipient_without_saying_so() {
  let app = TestApp::with_config(|config| {
    config.limits.reset_emails = [Limit::new(2, HOUR), Limit::new(100, DAY)];
  })
  .await;
  app.user("alice").await;
  let alice = email_of("alice");
  let mut client = app.client();
  for _ in 0..2 {
    assert_eq!(forgot(&mut client, &alice).await.status, StatusCode::OK);
  }
  assert_eq!(emails_to(&app, &alice, RESET), 2);
  let limited = forgot(&mut client, &alice).await;
  assert_eq!(emails_to(&app, &alice, RESET), 2, "held back");
  let unknown = forgot(&mut client, "nobody@example.com").await;
  assert_eq!(limited.status, unknown.status);
  assert_eq!(
    without(&limited, &alice),
    without(&unknown, "nobody@example.com")
  );
  assert!(limited.visible_text().contains("If an account uses"));

  // The daily limit, on top of a looser hourly one.
  let app = TestApp::with_config(|config| {
    config.limits.reset_emails = [Limit::new(100, HOUR), Limit::new(3, DAY)];
  })
  .await;
  app.user("alice").await;
  let mut client = app.client();
  for _ in 0..4 {
    assert_eq!(forgot(&mut client, &alice).await.status, StatusCode::OK);
  }
  assert_eq!(emails_to(&app, &alice, RESET), 3);
}

#[tokio::test]
async fn reset_requests_are_limited_per_address_apart_from_sign_ins() {
  let app = TestApp::with_config(|config| {
    config.limits.reset_requests = Limit::new(3, HOUR);
    config.limits.auth = Limit::new(3, 600);
  })
  .await;
  app.user("alice").await;
  let mut client = app.client();
  for i in 0..3 {
    let email = format!("someone{i}@example.com");
    assert_eq!(forgot(&mut client, &email).await.status, StatusCode::OK);
  }
  let limited = forgot(&mut client, &email_of("alice")).await;
  assert_eq!(limited.status, StatusCode::TOO_MANY_REQUESTS);
  assert_eq!(emails_to(&app, &email_of("alice"), RESET), 0);
  let login = client
    .post_form("/login", &[("login", "alice"), ("password", PASSWORD)])
    .await;
  assert_eq!(login.status, StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn verification_emails_are_limited_per_account() {
  let app = TestApp::with_config(|config| {
    config.limits.verification_emails =
      [Limit::new(2, HOUR), Limit::new(100, DAY)];
  })
  .await;
  let mut alice = app.unverified_user("alice").await;
  let alice_email = email_of("alice");
  for _ in 0..2 {
    let response = alice.post_form("/settings/verification", &[]).await;
    assert_eq!(response.status, StatusCode::OK);
  }
  assert_eq!(emails_to(&app, &alice_email, VERIFY), 3, "sign-up and two");
  let limited = alice.post_form("/settings/verification", &[]).await;
  assert_eq!(limited.status, StatusCode::TOO_MANY_REQUESTS);
  assert!(limited.visible_text().contains("Too many requests"));
  assert_eq!(emails_to(&app, &alice_email, VERIFY), 3);
  // Another account has its own.
  let mut bob = app.unverified_user("bob").await;
  let response = bob.post_form("/settings/verification", &[]).await;
  assert_eq!(response.status, StatusCode::OK);

  // Changing the address sends one too, and counts.
  let app = TestApp::with_config(|config| {
    config.limits.verification_emails =
      [Limit::new(100, HOUR), Limit::new(2, DAY)];
  })
  .await;
  let mut alice = app.unverified_user("alice").await;
  let change = |email: &'static str| [("email", email), ("password", PASSWORD)];
  let changed = alice
    .post_form("/settings/email", &change("alice2@example.com"))
    .await;
  assert_eq!(changed.location(), "/settings?saved=email");
  let response = alice.post_form("/settings/verification", &[]).await;
  assert_eq!(response.status, StatusCode::OK);
  let limited = alice
    .post_form("/settings/email", &change("alice3@example.com"))
    .await;
  assert_eq!(limited.status, StatusCode::TOO_MANY_REQUESTS);
  assert_eq!(emails_to(&app, "alice3@example.com", VERIFY), 0);
  assert_eq!(email_of_user(&app, "alice").await, "alice2@example.com");
}

#[tokio::test]
async fn the_daily_email_budget_holds_across_restarts() {
  let app = TestApp::with_config(budget(3)).await;
  let mut alice = app.unverified_user("alice").await;
  let mut bob = app.unverified_user("bob").await;
  let response = alice.post_form("/settings/verification", &[]).await;
  assert_eq!(response.status, StatusCode::OK);
  assert_eq!(app.mail.lock().expect("mail").len(), 3);

  // Signing up still works, without the email.
  let mut carol = app.client();
  let signup = sign_up(&mut carol, "carol").await;
  assert_eq!(signup.status, StatusCode::OK);
  assert!(carol.cookie.is_some(), "signed in");
  let text = signup.visible_text();
  assert!(text.contains("can’t send email right now"), "{text}");
  assert_eq!(emails_to(&app, &email_of("carol"), VERIFY), 0);

  // Everything else that emails says it can't, the same for addresses
  // with and without accounts.
  let mut client = app.client();
  let known = forgot(&mut client, &email_of("alice")).await;
  let unknown = forgot(&mut client, "nobody@example.com").await;
  assert_eq!(known.status, StatusCode::SERVICE_UNAVAILABLE);
  assert_eq!(unknown.status, StatusCode::SERVICE_UNAVAILABLE);
  assert_eq!(
    without(&known, &email_of("alice")),
    without(&unknown, "nobody@example.com")
  );
  assert!(
    known
      .visible_text()
      .contains("We can't send email right now")
  );
  let resend = bob.post_form("/settings/verification", &[]).await;
  assert_eq!(resend.status, StatusCode::SERVICE_UNAVAILABLE);
  assert!(
    resend
      .visible_text()
      .contains("We can't send email right now")
  );
  let change = bob
    .post_form(
      "/settings/email",
      &[("email", "bob2@example.com"), ("password", PASSWORD)],
    )
    .await;
  assert_eq!(change.status, StatusCode::SERVICE_UNAVAILABLE);
  assert_eq!(email_of_user(&app, "bob").await, email_of("bob"));
  assert_eq!(app.mail.lock().expect("mail").len(), 3);
  assert_eq!(outbox::sent_today(&app.state.db).await.expect("count"), 3);

  // A restart doesn't reset the count.
  let app = app.restart(budget(3)).await;
  assert_eq!(outbox::sent_today(&app.state.db).await.expect("count"), 3);
  let mut client = app.client();
  let known = forgot(&mut client, &email_of("alice")).await;
  assert_eq!(known.status, StatusCode::SERVICE_UNAVAILABLE);
  assert!(app.mail.lock().expect("mail").is_empty());

  // A bigger budget has room again.
  let app = app.restart(budget(4)).await;
  let mut client = app.client();
  let known = forgot(&mut client, &email_of("alice")).await;
  assert_eq!(known.status, StatusCode::OK);
  assert_eq!(emails_to(&app, &email_of("alice"), RESET), 1);
  assert_eq!(outbox::sent_today(&app.state.db).await.expect("count"), 4);
}

#[tokio::test]
async fn a_full_disk_refuses_sketch_content() {
  let app = TestApp::with_config(|config| {
    config.min_free_disk = u64::MAX;
  })
  .await;
  let mut alice = app.user("alice").await;
  let user = users::by_username(&app.state.db, "alice")
    .await
    .expect("user")
    .expect("alice");
  let sketch = NewSketch {
    title: "Stored before".into(),
    description: String::new(),
    contents: Contents {
      files: vec![SketchFile {
        path: "main.easl".into(),
        content: "(old)".into(),
      }],
      entry_file: None,
      entry_name: None,
    },
    visibility: Visibility::Public,
    forked_from: None,
    tags: Vec::new(),
  };
  let id = sketches::create(
    &app.state.db,
    user.id,
    &sketch,
    &app.state.config.history,
  )
  .await
  .map_err(|e| e.message())
  .expect("a sketch made before the disk filled");
  let path = format!("/api/sketches/{id}");

  let expect_full = |response: TestResponse| {
    assert_eq!(response.status, StatusCode::INSUFFICIENT_STORAGE);
    let message = response.json()["error"]
      .as_str()
      .expect("error")
      .to_string();
    assert!(message.contains("out of storage right now"), "{message}");
  };
  expect_full(
    alice
      .api(
        Method::POST,
        "/api/sketches",
        json!({"title": "New", "files": [{"path": "a.easl", "content": ""}]}),
      )
      .await,
  );
  expect_full(
    alice
      .api(
        Method::PUT,
        &path,
        json!({"files": [{"path": "main.easl", "content": "(new)"}]}),
      )
      .await,
  );
  expect_full(
    alice
      .api(Method::POST, &format!("{path}/fork"), Value::Null)
      .await,
  );
  let png = b"\x89PNG\r\n\x1a\nrest of the image".to_vec();
  expect_full(
    alice
      .raw(Method::PUT, &format!("{path}/thumbnail"), "image/png", png)
      .await,
  );

  // What stores no sketch content still works.
  let renamed = alice
    .api(Method::PUT, &path, json!({"title": "Renamed"}))
    .await;
  assert_eq!(renamed.status, StatusCode::OK);
  assert_eq!(renamed.json()["files"][0]["content"], "(old)");
  let mut bob = app.client();
  let shown = bob.api(Method::GET, &path, Value::Null).await;
  assert_eq!(shown.status, StatusCode::OK);
}

#[tokio::test]
async fn changing_the_password_invalidates_reset_links() {
  let app = TestApp::new().await;
  let mut alice = app.user("alice").await;
  let mut client = app.client();
  forgot(&mut client, &email_of("alice")).await;
  let link = app.link_in_email(&email_of("alice"));
  assert_eq!(client.get(&link).await.status, StatusCode::OK);
  let changed = alice
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
  assert_eq!(client.get(&link).await.status, StatusCode::NOT_FOUND);
  let reset = client
    .post_form(
      &link,
      &[
        ("password", "another new pw"),
        ("confirm", "another new pw"),
      ],
    )
    .await;
  assert_eq!(reset.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_new_reset_request_replaces_earlier_links() {
  let app = TestApp::new().await;
  app.user("alice").await;
  let mut client = app.client();
  forgot(&mut client, &email_of("alice")).await;
  let first = app.link_in_email(&email_of("alice"));
  forgot(&mut client, &email_of("alice")).await;
  let second = app.link_in_email(&email_of("alice"));
  assert_ne!(first, second);
  assert_eq!(client.get(&first).await.status, StatusCode::NOT_FOUND);
  assert_eq!(client.get(&second).await.status, StatusCode::OK);
}
