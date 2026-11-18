//! The moderation log, who deleted comments, restoring comments, and the
//! admin page's tabs.

mod common;

use std::fs;
use std::path::Path;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use sqlx::SqlitePool;
use sqlx::migrate::Migrator;
use sqlx::sqlite::SqliteConnectOptions;
use tempfile::TempDir;

use common::{Client, TestApp, database_path};
use easl_site_server::{db, moderation, users};

fn id_of(value: &Value) -> String {
  value["id"].as_str().expect("id").to_string()
}

/// A log row: its action, the admin's username (None for no admin), its
/// target, and its note.
type LogRow = (String, Option<String>, String, Option<String>);

async fn log_rows(db: &SqlitePool) -> Vec<LogRow> {
  sqlx::query_as(
    "SELECT m.action, u.username, m.target, m.note \
     FROM moderation_actions m LEFT JOIN users u ON u.id = m.admin_id \
     ORDER BY m.id",
  )
  .fetch_all(db)
  .await
  .expect("log rows")
}

fn row(action: &str, admin: Option<&str>, target: &str) -> LogRow {
  (
    action.to_string(),
    admin.map(str::to_string),
    target.to_string(),
    None,
  )
}

/// Who deleted a comment, by username.
async fn deleted_by(db: &SqlitePool, comment: i64) -> Option<String> {
  sqlx::query_scalar(
    "SELECT u.username FROM comments c \
     LEFT JOIN users u ON u.id = c.deleted_by WHERE c.id = ?",
  )
  .bind(comment)
  .fetch_one(db)
  .await
  .expect("comment")
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

async fn delete_comment(client: &mut Client<'_>, comment: i64) {
  let path = format!("/api/comments/{comment}");
  let response = client.api(Method::DELETE, &path, Value::Null).await;
  assert_eq!(
    response.status,
    StatusCode::NO_CONTENT,
    "{}",
    response.text()
  );
}

async fn post(client: &mut Client<'_>, path: &str) -> StatusCode {
  client.api(Method::POST, path, Value::Null).await.status
}

#[tokio::test]
async fn admin_actions_are_logged() {
  let app = TestApp::new().await;
  let db = &app.state.db;
  let mut alice = app.user("alice").await;
  app.user("bob").await;
  let mut carol = app.user("carol").await;
  let mut dana = app.admin("dana").await;
  let spam = id_of(&alice.create_sketch("FREE GPU CREDITS", "public").await);
  let hide = format!("/api/admin/sketches/{spam}/hide");
  let unhide = format!("/api/admin/sketches/{spam}/unhide");

  // Hiding twice changes (and logs) nothing the second time.
  assert_eq!(post(&mut dana, &hide).await, StatusCode::NO_CONTENT);
  assert_eq!(post(&mut dana, &hide).await, StatusCode::NO_CONTENT);
  assert_eq!(post(&mut dana, &unhide).await, StatusCode::NO_CONTENT);
  let missing = post(&mut dana, "/api/admin/sketches/missing1/hide").await;
  assert_eq!(missing, StatusCode::NOT_FOUND);

  assert_eq!(
    post(&mut dana, "/api/admin/users/bob/ban").await,
    StatusCode::NO_CONTENT
  );
  assert_eq!(
    post(&mut dana, "/api/admin/users/bob/ban").await,
    StatusCode::NO_CONTENT
  );
  assert_eq!(
    post(&mut dana, "/api/admin/users/bob/unban").await,
    StatusCode::NO_CONTENT
  );
  let mut bob_again = app.client();
  let login = bob_again
    .post_form(
      "/login",
      &[("login", "bob"), ("password", common::PASSWORD)],
    )
    .await;
  assert_eq!(login.status, StatusCode::SEE_OTHER);
  let mut bob = bob_again;

  let report = carol
    .api(
      Method::POST,
      "/api/reports",
      json!({"kind": "sketch", "target": spam, "reason": "spam"}),
    )
    .await
    .json()["id"]
    .as_i64()
    .expect("report id");
  let resolve = format!("/api/admin/reports/{report}/resolve");
  assert_eq!(post(&mut dana, &resolve).await, StatusCode::NO_CONTENT);
  assert_eq!(post(&mut dana, &resolve).await, StatusCode::NO_CONTENT);

  // Comments on alice's sketch: deleted by a moderator, by the sketch's
  // owner, and by their author.
  let by_carol = comment_on(&mut carol, &spam, "buy now").await;
  let by_bob = comment_on(&mut bob, &spam, "me too").await;
  let carol_own = comment_on(&mut carol, &spam, "oops").await;
  delete_comment(&mut dana, by_carol).await;
  delete_comment(&mut alice, by_bob).await;
  delete_comment(&mut carol, carol_own).await;
  assert_eq!(deleted_by(db, by_carol).await.as_deref(), Some("dana"));
  assert_eq!(deleted_by(db, by_bob).await.as_deref(), Some("alice"));
  assert_eq!(deleted_by(db, carol_own).await.as_deref(), Some("carol"));

  // An admin acting on their own sketch or comment isn't moderating.
  let danas = id_of(&dana.create_sketch("Dana's", "public").await);
  let on_danas = comment_on(&mut bob, &danas, "hello").await;
  let danas_own = comment_on(&mut dana, &spam, "a moderator's note").await;
  delete_comment(&mut dana, on_danas).await;
  delete_comment(&mut dana, danas_own).await;
  assert_eq!(deleted_by(db, on_danas).await.as_deref(), Some("dana"));
  let own_sketch_delete = dana
    .api(
      Method::DELETE,
      &format!("/api/sketches/{danas}"),
      Value::Null,
    )
    .await;
  assert_eq!(own_sketch_delete.status, StatusCode::NO_CONTENT);

  let restore = format!("/api/admin/comments/{by_bob}/restore");
  assert_eq!(post(&mut dana, &restore).await, StatusCode::NO_CONTENT);

  // Owners deleting their own sketches aren't logged; admins deleting
  // someone else's are.
  let bobs = id_of(&bob.create_sketch("Bob's", "public").await);
  let path = format!("/api/sketches/{bobs}");
  let deleted = bob.api(Method::DELETE, &path, Value::Null).await;
  assert_eq!(deleted.status, StatusCode::NO_CONTENT);
  let deleted = dana
    .api(
      Method::DELETE,
      &format!("/api/sketches/{spam}"),
      Value::Null,
    )
    .await;
  assert_eq!(deleted.status, StatusCode::NO_CONTENT);

  // The command line.
  assert_eq!(
    moderation::set_admin(db, "alice", true)
      .await
      .expect("grant"),
    Some(true)
  );
  assert_eq!(
    moderation::set_admin(db, "alice", true)
      .await
      .expect("again"),
    Some(false)
  );
  assert_eq!(
    moderation::set_admin(db, "nobody", true)
      .await
      .expect("none"),
    None
  );
  moderation::set_admin(db, "alice", false)
    .await
    .expect("revoke");

  let dana_name = Some("dana");
  let cli = |action: &str| {
    (
      action.to_string(),
      None,
      "alice".to_string(),
      Some("command line".to_string()),
    )
  };
  assert_eq!(
    log_rows(db).await,
    vec![
      row("hide_sketch", dana_name, &spam),
      row("unhide_sketch", dana_name, &spam),
      row("ban_user", dana_name, "bob"),
      row("unban_user", dana_name, "bob"),
      row("resolve_report", dana_name, &report.to_string()),
      row("delete_comment", dana_name, &by_carol.to_string()),
      row("restore_comment", dana_name, &by_bob.to_string()),
      row("delete_sketch", dana_name, &spam),
      cli("grant_admin"),
      cli("revoke_admin"),
    ]
  );

  // The log reads as sentences, and keeps the deleted sketch's title.
  let log = dana.get("/admin?tab=log").await;
  assert_eq!(log.status, StatusCode::OK);
  let text = log.visible_text();
  for sentence in [
    "@dana hid “FREE GPU CREDITS” (deleted) by @alice",
    "@dana unhid “FREE GPU CREDITS” (deleted) by @alice",
    "@dana banned @bob",
    "@dana unbanned @bob",
    &format!(
      "@dana resolved report #{report} about “FREE GPU CREDITS” (deleted) \
       by @alice"
    ),
    "@dana deleted a comment by @carol on “FREE GPU CREDITS” (deleted)",
    "@dana restored a comment by @bob on “FREE GPU CREDITS” (deleted)",
    "@dana deleted the sketch “FREE GPU CREDITS” (deleted) by @alice",
    "The command line made @alice an admin",
    "The command line took away @alice’s admin rights",
  ] {
    assert!(text.contains(sentence), "{sentence:?} in {text}");
  }
  assert!(text.contains("buy now"), "the comment's excerpt");
  assert!(text.contains("Reason: spam"));
}

#[tokio::test]
async fn restoring_comments() {
  let app = TestApp::new().await;
  let mut alice = app.user("alice").await;
  let mut bob = app.user("bob").await;
  let mut dana = app.admin("dana").await;
  let sketch = id_of(&alice.create_sketch("Sketch", "public").await);
  let sketch_path = format!("/api/sketches/{sketch}");
  let comment_count = async |client: &mut Client<'_>| {
    client
      .api(Method::GET, &sketch_path, Value::Null)
      .await
      .json()["comment_count"]
      .as_i64()
      .expect("count")
  };
  let comment = comment_on(&mut bob, &sketch, "worth keeping").await;
  comment_on(&mut bob, &sketch, "another").await;
  assert_eq!(comment_count(&mut alice).await, 2);

  let restore = format!("/api/admin/comments/{comment}/restore");
  assert_eq!(post(&mut dana, &restore).await, StatusCode::BAD_REQUEST);
  delete_comment(&mut bob, comment).await;
  assert_eq!(comment_count(&mut alice).await, 1);

  assert_eq!(post(&mut bob, &restore).await, StatusCode::FORBIDDEN);
  assert_eq!(post(&mut alice, &restore).await, StatusCode::FORBIDDEN);
  assert_eq!(
    post(&mut dana, "/api/admin/comments/99999/restore").await,
    StatusCode::NOT_FOUND
  );
  assert_eq!(
    post(&mut dana, "/api/admin/comments/nope/restore").await,
    StatusCode::NOT_FOUND
  );
  assert_eq!(post(&mut dana, &restore).await, StatusCode::NO_CONTENT);
  assert_eq!(comment_count(&mut alice).await, 2);
  assert_eq!(deleted_by(&app.state.db, comment).await, None);
  let comments = alice
    .api(Method::GET, &format!("{sketch_path}/comments"), Value::Null)
    .await
    .json();
  assert_eq!(comments["comments"][0]["body"], "worth keeping");
  assert_eq!(post(&mut dana, &restore).await, StatusCode::BAD_REQUEST);

  // Deleting it again counts down once more, and the count survives the
  // comment's author being deleted.
  delete_comment(&mut alice, comment).await;
  assert_eq!(comment_count(&mut alice).await, 1);
  assert_eq!(post(&mut dana, &restore).await, StatusCode::NO_CONTENT);
  assert_eq!(comment_count(&mut alice).await, 2);
  let bob_id: i64 =
    sqlx::query_scalar("SELECT id FROM users WHERE username = 'bob'")
      .fetch_one(&app.state.db)
      .await
      .expect("bob");
  users::delete(&app.state.db, bob_id)
    .await
    .expect("delete bob");
  assert_eq!(comment_count(&mut alice).await, 0);
}

#[tokio::test]
async fn admin_tabs() {
  let app = TestApp::new().await;
  let mut alice = app.user("alice").await;
  let mut bob = app.user("bob").await;
  let mut dana = app.admin("dana").await;
  let hidden = id_of(&alice.create_sketch("Spam sketch", "public").await);
  let kept = id_of(&alice.create_sketch("Kept sketch", "public").await);
  let comment = comment_on(&mut bob, &kept, "rude words").await;
  comment_on(&mut bob, &kept, "fine words").await;
  let own = comment_on(&mut alice, &kept, "my own words").await;
  bob
    .api(
      Method::POST,
      "/api/reports",
      json!({"kind": "user", "target": "alice", "reason": "first report"}),
    )
    .await;
  bob
    .api(
      Method::POST,
      "/api/reports",
      json!({"kind": "sketch", "target": kept, "reason": "second report"}),
    )
    .await;
  post(&mut dana, &format!("/api/admin/sketches/{hidden}/hide")).await;
  delete_comment(&mut dana, comment).await;
  delete_comment(&mut alice, own).await;
  post(&mut dana, "/api/admin/reports/1/resolve").await;
  post(&mut dana, "/api/admin/users/bob/ban").await;

  let tabs = [
    "/admin",
    "/admin?tab=reports&resolved=1",
    "/admin?tab=banned",
    "/admin?tab=hidden",
    "/admin?tab=comments",
    "/admin?tab=log",
  ];
  for tab in tabs {
    assert_eq!(alice.get(tab).await.status, StatusCode::NOT_FOUND, "{tab}");
    assert_eq!(
      app.client().get(tab).await.status,
      StatusCode::SEE_OTHER,
      "{tab}"
    );
  }

  let reports = dana.get("/admin").await;
  let text = reports.visible_text();
  assert!(text.contains("Reports (1)"), "{text}");
  assert!(text.contains("Banned users (1)"));
  assert!(text.contains("Hidden sketches (1)"));
  assert!(text.contains("Deleted comments (2)"));
  assert!(text.contains("Open (1) Resolved (1)"));
  assert!(text.contains("second report"));
  assert!(!text.contains("first report"));

  let resolved = dana.get("/admin?tab=reports&resolved=1").await;
  let text = resolved.visible_text();
  assert!(text.contains("first report"));
  assert!(!text.contains("second report"));
  assert!(text.contains("by @dana"), "{text}");

  let banned = dana.get("/admin?tab=banned").await;
  let text = banned.visible_text();
  assert!(text.contains("bob @bob Banned just now by @dana"), "{text}");
  // Banned users' content still counts for admins.
  assert!(text.contains("0 sketches · 1 comment"), "{text}");
  assert!(banned.text().contains("/api/admin/users/bob/unban"));
  assert!(!text.contains("alice"));

  let hidden_tab = dana.get("/admin?tab=hidden").await;
  let text = hidden_tab.visible_text();
  assert!(text.contains("Spam sketch by @alice"), "{text}");
  assert!(text.contains("Hidden just now by @dana"));
  assert!(!text.contains("Kept sketch"));
  assert!(
    hidden_tab
      .text()
      .contains(&format!("/api/admin/sketches/{hidden}/unhide"))
  );

  let comments = dana.get("/admin?tab=comments").await;
  let text = comments.visible_text();
  assert!(text.contains("rude words"));
  assert!(text.contains("my own words"));
  assert!(!text.contains("fine words"));
  assert!(
    text.contains("deleted just now by moderator @dana"),
    "{text}"
  );
  assert!(text.contains("deleted just now by the author"));
  assert!(
    comments
      .text()
      .contains(&format!("/api/admin/comments/{comment}/restore"))
  );

  // A sketch owner deleting someone's comment.
  let unbanned = post(&mut dana, "/api/admin/users/bob/unban").await;
  assert_eq!(unbanned, StatusCode::NO_CONTENT);
  let mut bob = app.client();
  bob
    .post_form(
      "/login",
      &[("login", "bob"), ("password", common::PASSWORD)],
    )
    .await;
  let by_bob = comment_on(&mut bob, &kept, "owner removes this").await;
  delete_comment(&mut alice, by_bob).await;
  let text = dana.get("/admin?tab=comments").await.visible_text();
  assert!(
    text.contains("deleted just now by the sketch owner"),
    "{text}"
  );

  // An unknown tab is the reports tab.
  let text = dana.get("/admin?tab=nope").await.visible_text();
  assert!(text.contains("Open reports (1)"));
}

#[tokio::test]
async fn admin_lists_paginate() {
  let app = TestApp::new().await;
  let mut alice = app.user("alice").await;
  let mut dana = app.admin("dana").await;
  let sketch = id_of(&alice.create_sketch("Toggled", "public").await);
  let count = moderation::PAGE_SIZE + 2;
  for i in 0..count {
    let action = if i % 2 == 0 { "hide" } else { "unhide" };
    let path = format!("/api/admin/sketches/{sketch}/{action}");
    assert_eq!(post(&mut dana, &path).await, StatusCode::NO_CONTENT);
  }
  let first = dana.get("/admin?tab=log").await;
  let text = first.page();
  assert_eq!(text.matches("<li>").count(), moderation::PAGE_SIZE as usize);
  assert!(
    text.contains("href=\"/admin?tab=log&amp;page=2\""),
    "{text}"
  );
  let second = dana.get("/admin?tab=log&page=2").await;
  assert_eq!(second.text().matches("<li>").count(), 2);
  assert!(second.text().contains("rel=\"prev\""));
  assert!(!second.text().contains("rel=\"next\""));

  for i in 0..=moderation::PAGE_SIZE {
    let comment = comment_on(&mut alice, &sketch, &format!("note {i}")).await;
    delete_comment(&mut alice, comment).await;
  }
  let first = dana.get("/admin?tab=comments").await;
  // Newest first: the last one deleted leads, and the first is on page 2.
  let text = first.visible_text();
  assert!(text.contains(&format!("note {}", moderation::PAGE_SIZE)));
  assert!(!text.contains("note 0 "));
  assert!(
    first
      .page()
      .contains("href=\"/admin?tab=comments&amp;page=2\"")
  );
  let second = dana.get("/admin?tab=comments&page=2").await.visible_text();
  assert!(second.contains("note 0 "), "{second}");
}

/// The migrations before the moderation log, for building a database as it
/// was.
async fn migrate_to_before_the_log(dir: &TempDir) {
  let migrations = dir.path().join("old-migrations");
  fs::create_dir(&migrations).expect("migrations dir");
  let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
  for name in ["0001_init.sql", "0002_revisions.sql", "0003_tags.sql"] {
    fs::copy(source.join(name), migrations.join(name)).expect("copy");
  }
  let pool = SqlitePool::connect_with(
    SqliteConnectOptions::new()
      .filename(database_path(dir))
      .create_if_missing(true)
      .foreign_keys(true),
  )
  .await
  .expect("old database");
  Migrator::new(migrations.as_path())
    .await
    .expect("migrator")
    .run(&pool)
    .await
    .expect("old migrations");
  sqlx::raw_sql(
    "INSERT INTO users (id, username, email, password_hash, created_at, \
       banned_at, is_admin) VALUES \
       (1, 'ella', 'ella@example.com', 'x', 1, NULL, 1), \
       (2, 'alice', 'alice@example.com', 'x', 1, NULL, 0), \
       (3, 'bob', 'bob@example.com', 'x', 1, 100, 0); \
     INSERT INTO sketches (id, owner_id, title, visibility, created_at, \
       updated_at, hidden_at) VALUES \
       ('sketch01', 2, 'Old hidden', 'public', 1, 1, 50); \
     INSERT INTO sketch_revisions (sketch_id, number, created_at) \
       VALUES ('sketch01', 1, 1); \
     INSERT INTO comments (id, sketch_id, user_id, body, created_at, \
       deleted_at) VALUES \
       (1, 'sketch01', 3, 'old deleted comment', 2, 60), \
       (2, 'sketch01', 3, 'old live comment', 3, NULL); \
     INSERT INTO reports (id, reporter_id, kind, target, reason, created_at, \
       resolved_at, resolved_by) VALUES \
       (1, 2, 'comment', '1', 'old reason', 4, 70, 1), \
       (2, 2, 'user', 'bob', 'still open', 5, NULL, NULL);",
  )
  .execute(&pool)
  .await
  .expect("old data");
  pool.close().await;
}

#[tokio::test]
async fn migrating_an_existing_database() {
  let dir = TempDir::new().expect("temp dir");
  migrate_to_before_the_log(&dir).await;
  let app = TestApp::with_database_in(dir).await;
  let db = &app.state.db;
  // Opening it again changes nothing.
  db::open(&app.database_path()).await.expect("reopen");

  assert_eq!(
    log_rows(db).await,
    vec![row("resolve_report", Some("ella"), "1")],
    "resolved reports are backfilled"
  );
  let count: i64 =
    sqlx::query_scalar("SELECT comment_count FROM sketches WHERE id = ?")
      .bind("sketch01")
      .fetch_one(db)
      .await
      .expect("count");
  assert_eq!(count, 1, "the live comment");

  let mut dana = app.admin("dana").await;
  let text = dana.get("/admin?tab=banned").await.visible_text();
  assert!(text.contains("Banned"), "{text}");
  assert!(text.contains("by unknown"), "{text}");
  let text = dana.get("/admin?tab=hidden").await.visible_text();
  assert!(text.contains("Old hidden by @alice"), "{text}");
  assert!(text.contains("by unknown"), "{text}");
  let text = dana.get("/admin?tab=comments").await.visible_text();
  assert!(text.contains("old deleted comment"));
  assert!(text.contains("by unknown"), "{text}");
  let text = dana.get("/admin?tab=log").await.visible_text();
  assert!(
    text.contains(
      "@ella resolved report #1 about a comment by @bob on “Old hidden”"
    ),
    "{text}"
  );

  // Restoring an old deletion counts the comment.
  let restore = post(&mut dana, "/api/admin/comments/1/restore").await;
  assert_eq!(restore, StatusCode::NO_CONTENT);
  let count: i64 =
    sqlx::query_scalar("SELECT comment_count FROM sketches WHERE id = ?")
      .bind("sketch01")
      .fetch_one(db)
      .await
      .expect("count");
  assert_eq!(count, 2);
}
