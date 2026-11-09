//! Reports, the admin page, and bans.

mod common;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};

use common::{PASSWORD, TestApp};

fn id_of(value: &Value) -> String {
  value["id"].as_str().expect("id").to_string()
}

#[tokio::test]
async fn reports_and_admin_actions() {
  let app = TestApp::new().await;
  let mut alice = app.user("alice").await;
  let sketch = id_of(&alice.create_sketch("Offensive", "public").await);
  let comment = alice
    .api(
      Method::POST,
      &format!("/api/sketches/{sketch}/comments"),
      json!({"body": "rude words"}),
    )
    .await
    .json();
  let mut bob = app.user("bob").await;
  let mut unverified = app.unverified_user("carol").await;

  let refused = unverified
    .api(
      Method::POST,
      "/api/reports",
      json!({"kind": "sketch", "target": sketch, "reason": "bad"}),
    )
    .await;
  assert_eq!(refused.status, StatusCode::FORBIDDEN);

  let reports = [
    json!({"kind": "sketch", "target": sketch, "reason": "Not nice <b>"}),
    json!({"kind": "comment", "target": comment["id"].to_string(), "reason": ""}),
    json!({"kind": "user", "target": "alice", "reason": "spammer"}),
  ];
  for report in &reports {
    let response = bob.api(Method::POST, "/api/reports", report.clone()).await;
    assert_eq!(response.status, StatusCode::CREATED, "{}", response.text());
    assert!(response.json()["id"].is_number());
  }
  for bad in [
    json!({"kind": "sketch", "target": "missing1", "reason": ""}),
    json!({"kind": "comment", "target": "99999", "reason": ""}),
    json!({"kind": "user", "target": "nobody", "reason": ""}),
  ] {
    let response = bob.api(Method::POST, "/api/reports", bad).await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);
  }
  let too_long = bob
    .api(
      Method::POST,
      "/api/reports",
      json!({"kind": "user", "target": "alice", "reason": "x".repeat(1001)}),
    )
    .await;
  assert_eq!(too_long.status, StatusCode::BAD_REQUEST);

  // Only admins see the queue.
  assert_eq!(bob.get("/admin").await.status, StatusCode::NOT_FOUND);
  assert_eq!(
    app.client().get("/admin").await.status,
    StatusCode::SEE_OTHER
  );
  let mut admin = app.admin("dana").await;
  let page = admin.get("/admin").await;
  assert_eq!(page.status, StatusCode::OK);
  let text = page.page();
  assert!(text.contains("Open reports (3)"));
  assert!(text.contains("Offensive"));
  assert!(text.contains("rude words"));
  assert!(text.contains("Not nice &lt;b&gt;"));
  assert!(text.contains(&format!("/api/admin/sketches/{sketch}/hide")));
  assert!(text.contains("/api/admin/users/alice/ban"));

  let first_report = 1;
  let resolve = format!("/api/admin/reports/{first_report}/resolve");
  let not_admin = bob.api(Method::POST, &resolve, Value::Null).await;
  assert_eq!(not_admin.status, StatusCode::FORBIDDEN);
  let resolved = admin.api(Method::POST, &resolve, Value::Null).await;
  assert_eq!(resolved.status, StatusCode::NO_CONTENT);
  assert!(
    admin
      .get("/admin")
      .await
      .page()
      .contains("Open reports (2)")
  );
  let missing = admin
    .api(Method::POST, "/api/admin/reports/999/resolve", Value::Null)
    .await;
  assert_eq!(missing.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn banned_users() {
  let app = TestApp::new().await;
  let mut alice = app.user("alice").await;
  let sketch = id_of(&alice.create_sketch("Alice's sketch", "public").await);
  let mut bob = app.user("bob").await;
  bob
    .api(
      Method::POST,
      &format!("/api/sketches/{sketch}/comments"),
      json!({"body": "comment by bob"}),
    )
    .await;
  let bob_sketch = id_of(&bob.create_sketch("Bob's sketch", "public").await);
  let mut admin = app.admin("dana").await;

  let not_admin = alice
    .api(Method::POST, "/api/admin/users/bob/ban", Value::Null)
    .await;
  assert_eq!(not_admin.status, StatusCode::FORBIDDEN);
  let self_ban = admin
    .api(Method::POST, "/api/admin/users/dana/ban", Value::Null)
    .await;
  assert_eq!(self_ban.status, StatusCode::BAD_REQUEST);
  let banned = admin
    .api(Method::POST, "/api/admin/users/bob/ban", Value::Null)
    .await;
  assert_eq!(banned.status, StatusCode::NO_CONTENT);

  // Signed out, can't sign in, and his content is gone from view.
  let me = bob.api(Method::GET, "/api/me", Value::Null).await.json();
  assert!(me["user"].is_null());
  let mut again = app.client();
  let login = again
    .post_form("/login", &[("login", "bob"), ("password", PASSWORD)])
    .await;
  assert_eq!(login.status, StatusCode::FORBIDDEN);
  assert!(again.cookie.is_none());
  let explore = alice.get("/").await.text();
  assert!(!explore.contains("Bob&#x27;s sketch"));
  assert!(explore.contains("Alice&#x27;s sketch"));
  assert_eq!(alice.get("/u/bob").await.status, StatusCode::NOT_FOUND);
  let sketch_path = format!("/api/sketches/{bob_sketch}");
  assert_eq!(
    alice
      .api(Method::GET, &sketch_path, Value::Null)
      .await
      .status,
    StatusCode::NOT_FOUND
  );
  let comments = alice
    .api(
      Method::GET,
      &format!("/api/sketches/{sketch}/comments"),
      Value::Null,
    )
    .await
    .json();
  assert!(
    comments["comments"]
      .as_array()
      .expect("comments")
      .is_empty()
  );
  // Admins still see it all.
  assert_eq!(admin.get("/u/bob").await.status, StatusCode::OK);

  admin
    .api(Method::POST, "/api/admin/users/bob/unban", Value::Null)
    .await;
  let login = again
    .post_form("/login", &[("login", "bob"), ("password", PASSWORD)])
    .await;
  assert_eq!(login.status, StatusCode::SEE_OTHER);
  assert!(alice.get("/").await.page().contains("Bob's sketch"));
}
