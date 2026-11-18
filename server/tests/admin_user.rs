//! A user's moderation page (`/admin/users/<username>`), the links into
//! it, and keeping email addresses out of everything but it.

mod common;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};

use common::{Client, TestApp, email_of};

fn id_of(value: &Value) -> String {
  value["id"].as_str().expect("id").to_string()
}

async fn comment_on(client: &mut Client<'_>, sketch: &str, body: &str) -> i64 {
  let response = client
    .api(
      Method::POST,
      &format!("/api/sketches/{sketch}/comments"),
      json!({ "body": body }),
    )
    .await;
  assert_eq!(response.status, StatusCode::CREATED, "{}", response.text());
  response.json()["id"].as_i64().expect("comment id")
}

async fn report(
  client: &mut Client<'_>,
  kind: &str,
  target: &str,
  reason: &str,
) -> i64 {
  let response = client
    .api(
      Method::POST,
      "/api/reports",
      json!({ "kind": kind, "target": target, "reason": reason }),
    )
    .await;
  assert_eq!(response.status, StatusCode::CREATED, "{}", response.text());
  response.json()["id"].as_i64().expect("report id")
}

async fn post(client: &mut Client<'_>, path: &str) -> StatusCode {
  client.api(Method::POST, path, Value::Null).await.status
}

#[tokio::test]
async fn the_moderation_page() {
  let app = TestApp::new().await;
  let mut alice = app.user("alice").await;
  let mut bob = app.user("bob").await;
  let mut dana = app.admin("dana").await;
  bob.create_sketch("Bob's public", "public").await;
  bob.create_sketch("Bob's private", "private").await;
  let hidden = id_of(&bob.create_sketch("Bob's spam", "public").await);
  let alices = id_of(&alice.create_sketch("Alice's", "public").await);
  let rude = comment_on(&mut bob, &alices, "rude words").await;
  let regret = comment_on(&mut bob, &alices, "regrettable").await;
  comment_on(&mut bob, &alices, "kind words").await;
  bob
    .api(
      Method::DELETE,
      &format!("/api/comments/{regret}"),
      Value::Null,
    )
    .await;

  let about_sketch = report(&mut alice, "sketch", &hidden, "spam sketch").await;
  report(&mut alice, "comment", &rude.to_string(), "rude comment").await;
  report(&mut alice, "user", "bob", "spammer").await;
  report(&mut bob, "sketch", &alices, "bob's own report").await;
  post(&mut dana, &format!("/api/admin/sketches/{hidden}/hide")).await;
  post(
    &mut dana,
    &format!("/api/admin/reports/{about_sketch}/resolve"),
  )
  .await;
  dana
    .api(
      Method::DELETE,
      &format!("/api/comments/{rude}"),
      Value::Null,
    )
    .await;
  post(&mut dana, "/api/admin/users/bob/ban").await;

  // Only admins.
  let refused = alice.get("/admin/users/bob").await;
  assert_eq!(refused.status, StatusCode::NOT_FOUND);
  assert!(!refused.text().contains(&email_of("bob")));
  let signed_out = app.client().get("/admin/users/bob").await;
  assert_eq!(signed_out.status, StatusCode::SEE_OTHER);
  assert_eq!(
    dana.get("/admin/users/nobody").await.status,
    StatusCode::NOT_FOUND
  );

  let page = dana.get("/admin/users/bob").await;
  assert_eq!(page.status, StatusCode::OK);
  let html = page.page();
  let text = page.visible_text();
  assert!(text.contains("Moderation view"), "{text}");
  assert!(html.contains("href=\"mailto:bob@example.com\""), "{html}");
  assert!(text.contains("bob@example.com verified"));
  assert!(text.contains("Admin no"));
  assert!(text.contains("Status Banned just now by @dana"), "{text}");
  assert!(text.contains("Sketches 3 (1 hidden, 1 private)"), "{text}");
  assert!(text.contains("Comments 1 (2 deleted)"), "{text}");
  assert!(
    html.contains("href=\"/u/bob\""),
    "a link to the public profile"
  );
  assert!(html.contains("/api/admin/users/bob/unban"));

  assert!(text.contains("Reports they submitted (1)"));
  assert!(text.contains("bob's own report"));
  assert!(text.contains("Reports against them (3)"));
  for reason in ["spam sketch", "rude comment", "spammer"] {
    assert!(text.contains(reason), "{reason}");
  }
  assert!(text.contains("Resolved just now by @dana"));
  assert!(text.contains("open"));
  // The reporter links to their moderation page.
  assert!(
    html.contains("Reported by <a href=\"/admin/users/alice\">@alice</a>")
  );

  assert!(text.contains("Moderation actions (4)"), "{text}");
  assert!(text.contains("@dana banned @bob"));
  assert!(text.contains("@dana resolved report #1 about “Bob's spam” by @bob"));
  assert!(text.contains("@dana hid “Bob's spam” by @bob"));
  assert!(text.contains("@dana deleted a comment by @bob on “Alice's”"));
  assert!(text.contains("Deleted comments (2)"));
  assert!(text.contains("deleted just now by the author"));
  assert!(text.contains("deleted just now by moderator @dana"));
  assert!(!text.contains("Show all"));

  // Admins can't be banned, so their pages offer no ban.
  let own = dana.get("/admin/users/dana").await;
  assert!(!own.text().contains("/api/admin/users/dana/ban"));
  assert!(own.page().contains("mailto:dana@example.com"));

  // Long lists are capped, with a link to all of each.
  post(&mut dana, "/api/admin/users/bob/unban").await;
  let mut bob = app.client();
  bob
    .post_form(
      "/login",
      &[("login", "bob"), ("password", common::PASSWORD)],
    )
    .await;
  for i in 0..12 {
    report(&mut bob, "sketch", &alices, &format!("report {i:02}")).await;
  }
  let page = dana.get("/admin/users/bob").await;
  let text = page.visible_text();
  assert!(text.contains("Reports they submitted (13)"));
  assert!(text.contains("report 11"));
  assert!(!text.contains("report 01"), "only the newest 10");
  assert!(
    page
      .page()
      .contains("href=\"/admin/users/bob?list=submitted\"")
  );
  assert!(text.contains("Show all 13"));
  let all = dana.get("/admin/users/bob?list=submitted").await;
  let text = all.visible_text();
  assert!(text.contains("report 00"));
  assert!(text.contains("bob's own report"));
  assert!(!text.contains("Reports against them"), "only that list");
  assert!(text.contains("All of @bob’s record"));
}

#[tokio::test]
async fn links_into_the_moderation_page() {
  let app = TestApp::new().await;
  let mut alice = app.user("alice").await;
  let mut bob = app.user("bob").await;
  let mut dana = app.admin("dana").await;
  let sketch = id_of(&alice.create_sketch("Alice's", "public").await);
  report(&mut bob, "sketch", &sketch, "please look").await;

  // The shield on profiles, for admins only.
  let shield = "href=\"/admin/users/alice\"";
  assert!(dana.get("/u/alice").await.page().contains(shield));
  assert!(
    dana
      .get("/u/alice")
      .await
      .text()
      .contains("Moderation view")
  );
  assert!(!bob.get("/u/alice").await.page().contains(shield));
  assert!(!bob.get("/u/alice").await.text().contains("Moderation view"));
  assert!(!app.client().get("/u/alice").await.page().contains(shield));

  // The reports tab links the reporter and the reported user.
  let reports = dana.get("/admin").await.page();
  assert!(
    reports.contains("Reported by <a href=\"/admin/users/bob\">@bob</a>")
  );
  assert!(reports.contains("<a href=\"/admin/users/alice\">@alice</a>"));
}

#[tokio::test]
async fn emails_stay_private() {
  let app = TestApp::new().await;
  let mut alice = app.user("alice").await;
  let mut bob = app.user("bob").await;
  let mut dana = app.admin("dana").await;
  let sketch = id_of(&alice.create_sketch("Alice's", "public").await);
  comment_on(&mut alice, &sketch, "by alice").await;
  report(&mut bob, "user", "alice", "check").await;
  let playlist = alice
    .api(
      Method::POST,
      "/api/playlists",
      json!({"title": "Alice's list", "visibility": "public"}),
    )
    .await
    .json();
  let playlist = id_of(&playlist);
  alice
    .api(
      Method::POST,
      &format!("/api/playlists/{playlist}/items"),
      json!({"sketch_id": sketch}),
    )
    .await;

  let email = email_of("alice");
  let paths = [
    "/".to_string(),
    "/u/alice".to_string(),
    "/u/alice?tab=playlists".to_string(),
    format!("/s/{sketch}"),
    format!("/p/{playlist}"),
    format!("/api/sketches/{sketch}"),
    format!("/api/sketches/{sketch}/comments"),
    format!("/api/playlists/{playlist}"),
  ];
  let mut signed_out = app.client();
  for path in &paths {
    for (who, client) in [
      ("bob", &mut bob),
      ("signed out", &mut signed_out),
      // Admins on the public pages see what everyone sees.
      ("dana", &mut dana),
    ] {
      let response = client.get(path).await;
      assert_eq!(response.status, StatusCode::OK, "{path} for {who}");
      assert!(
        !response.text().contains(&email),
        "{path} shows alice's email to {who}"
      );
    }
  }
  // The API's user shapes carry no email address, even one's own.
  for path in ["/api/me", "/api/me/sketches", "/api/me/playlists"] {
    let response = alice.api(Method::GET, path, Value::Null).await;
    assert_eq!(response.status, StatusCode::OK, "{path}");
    assert!(!response.text().contains(&email), "{path}");
  }
  // The admin page's tabs don't show it either; only the user's
  // moderation page does.
  for tab in ["/admin", "/admin?tab=log", "/admin?tab=banned"] {
    assert!(!dana.get(tab).await.text().contains(&email), "{tab}");
  }
  assert!(dana.get("/admin/users/alice").await.text().contains(&email));
}
