//! History budgets: pruning a sketch's oldest revisions past its budget
//! (never its head or published revision), refusing saves and new
//! sketches past the account's, and the migration that measured existing
//! revisions.

mod common;

use std::fs;
use std::path::Path;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use sqlx::SqlitePool;
use sqlx::migrate::Migrator;
use sqlx::sqlite::SqliteConnectOptions;
use tempfile::TempDir;

use common::{Client, TestApp, TestResponse, database_path, email_of};
use easl_site_server::config::{
  HistoryBudget, SKETCH_HISTORY_BYTES, UNVERIFIED_ACCOUNT_HISTORY_BYTES,
  VERIFIED_ACCOUNT_HISTORY_BYTES,
};
use easl_site_server::revisions::{self, Contents};
use easl_site_server::sketches::{self, SketchUpdate};
use easl_site_server::validate::SketchFile;
use easl_site_server::visibility::Visibility;

fn id_of(sketch: &Value) -> String {
  sketch["id"].as_str().expect("id").to_string()
}

/// One file named `main.easl` holding `source`.
fn main_file(source: &str) -> Value {
  json!([{"path": "main.easl", "content": source}])
}

/// One file holding `bytes` bytes, starting with `tag` so each save
/// differs.
fn file_of_size(tag: &str, bytes: usize) -> Value {
  let content = format!("{tag}{}", "x".repeat(bytes - tag.len()));
  main_file(&content)
}

/// Five files of almost 200 KB each: close to the most one save holds.
fn huge_files(tag: &str) -> Value {
  let files: Vec<Value> = (0..5)
    .map(|i| {
      let content = format!("{tag} {i}{}", "x".repeat(200 * 1024 - 100));
      json!({"path": format!("f{i}.easl"), "content": content})
    })
    .collect();
  Value::Array(files)
}

async fn put(client: &mut Client<'_>, id: &str, body: Value) -> TestResponse {
  client
    .api(Method::PUT, &format!("/api/sketches/{id}"), body)
    .await
}

/// Saves new files, expecting success, and returns the owner's view.
async fn save(client: &mut Client<'_>, id: &str, files: Value) -> Value {
  let response = put(client, id, json!({"files": files})).await;
  assert_eq!(response.status, StatusCode::OK, "{}", response.text());
  response.json()
}

async fn create(
  client: &mut Client<'_>,
  title: &str,
  files: Value,
) -> TestResponse {
  client
    .api(
      Method::POST,
      "/api/sketches",
      json!({"title": title, "files": files}),
    )
    .await
}

async fn post(client: &mut Client<'_>, id: &str, action: &str) -> Value {
  let response = client
    .api(
      Method::POST,
      &format!("/api/sketches/{id}/{action}"),
      Value::Null,
    )
    .await;
  assert_eq!(response.status, StatusCode::OK, "{}", response.text());
  response.json()
}

/// The numbers `GET …/revisions` lists, newest first.
async fn revision_numbers(client: &mut Client<'_>, id: &str) -> Vec<i64> {
  let response = client
    .api(
      Method::GET,
      &format!("/api/sketches/{id}/revisions"),
      Value::Null,
    )
    .await;
  assert_eq!(response.status, StatusCode::OK, "{}", response.text());
  response.json()["revisions"]
    .as_array()
    .expect("revisions")
    .iter()
    .map(|r| r["number"].as_i64().expect("number"))
    .collect()
}

/// A sketch's `history_bytes`, checked against the sum of its revisions'
/// sizes (which the triggers keep it equal to).
async fn history_bytes(db: &SqlitePool, id: &str) -> i64 {
  let (total, sum): (i64, i64) = sqlx::query_as(
    "SELECT s.history_bytes, (SELECT COALESCE(SUM(size), 0) \
     FROM sketch_revisions r WHERE r.sketch_id = s.id) \
     FROM sketches s WHERE s.id = ?",
  )
  .bind(id)
  .fetch_one(db)
  .await
  .expect("history");
  assert_eq!(total, sum, "history_bytes is its revisions' total");
  total
}

#[tokio::test]
async fn many_small_saves_are_all_kept() {
  let app = TestApp::new().await;
  let mut alice = app.user("alice").await;
  let response = create(&mut alice, "Small", main_file("(v 0)")).await;
  let id = id_of(&response.json());
  for i in 1..=300 {
    save(&mut alice, &id, main_file(&format!("(v {i})"))).await;
  }
  let numbers = revision_numbers(&mut alice, &id).await;
  assert_eq!(numbers, (1..=301).rev().collect::<Vec<_>>());
  assert!(history_bytes(&app.state.db, &id).await < SKETCH_HISTORY_BYTES);
}

#[tokio::test]
async fn huge_saves_prune_the_oldest_but_keep_head_and_published() {
  let app = TestApp::new().await;
  let db = &app.state.db;
  let mut alice = app.user("alice").await;
  let id = id_of(&alice.create_sketch("Pruned", "public").await);
  save(&mut alice, &id, main_file("(two)")).await;
  save(&mut alice, &id, main_file("(three)")).await;
  post(&mut alice, &id, "publish").await;
  save(&mut alice, &id, main_file("(four)")).await;
  for i in 5..=16 {
    let own = save(&mut alice, &id, huge_files(&format!("({i})"))).await;
    assert_eq!(own["head_revision"], i);
    assert_eq!(own["published_revision"], 3);
    assert!(history_bytes(db, &id).await <= SKETCH_HISTORY_BYTES);
  }

  // The oldest went first; the published revision and the newest stay.
  let numbers = revision_numbers(&mut alice, &id).await;
  assert_eq!(numbers.last(), Some(&3), "{numbers:?}");
  let newest = &numbers[..numbers.len() - 1];
  let oldest_kept = *newest.last().expect("kept revisions");
  assert!(oldest_kept > 5, "huge revisions were pruned: {numbers:?}");
  assert_eq!(newest, (oldest_kept..=16).rev().collect::<Vec<_>>());

  // Everyone else still sees the published revision, and can fork it.
  let mut bob = app.user("bob").await;
  let shown = bob
    .api(Method::GET, &format!("/api/sketches/{id}"), Value::Null)
    .await
    .json();
  assert_eq!(shown["files"][0]["content"], "(three)");
  let published = bob
    .api(
      Method::GET,
      &format!("/api/sketches/{id}/published"),
      Value::Null,
    )
    .await
    .json();
  assert_eq!(published["revision"], 3);
  let fork = bob
    .api(
      Method::POST,
      &format!("/api/sketches/{id}/fork"),
      Value::Null,
    )
    .await;
  assert_eq!(fork.status, StatusCode::CREATED, "{}", fork.text());
  assert_eq!(fork.json()["files"][0]["content"], "(three)");

  // Discarding the draft goes back to the published revision, and the next
  // save is numbered after the highest ever, not reusing a pruned number.
  let own = post(&mut alice, &id, "discard-draft").await;
  assert_eq!(own["head_revision"], 3);
  assert_eq!(own["files"][0]["content"], "(three)");
  let own = save(&mut alice, &id, main_file("(again)")).await;
  assert_eq!(own["head_revision"], 17);
}

#[tokio::test]
async fn the_account_budget_refuses_what_pruning_cannot_fit() {
  // Each revision of one 100,000-byte file counts 100,329 bytes, so a
  // sketch keeps 3 of them, and a verified account holds about 10.
  let app = TestApp::with_config(|config| {
    config.history = HistoryBudget {
      per_sketch: 400_000,
      per_unverified_account: 250_000,
      per_verified_account: 1_000_000,
    };
  })
  .await;
  let db = &app.state.db;
  let mut alice = app.user("alice").await;
  let size = |tag: &str| file_of_size(tag, 100_000);

  // Saving over and over prunes the sketch, so the account never fills.
  let a = id_of(&create(&mut alice, "A", size("a0")).await.json());
  for i in 1..=5 {
    save(&mut alice, &a, size(&format!("a{i}"))).await;
  }
  assert_eq!(revision_numbers(&mut alice, &a).await, [6, 5, 4]);
  let b = id_of(&create(&mut alice, "B", size("b0")).await.json());
  save(&mut alice, &b, size("b1")).await;
  save(&mut alice, &b, size("b2")).await;
  let c = id_of(&create(&mut alice, "C", size("c0")).await.json());
  save(&mut alice, &c, size("c1")).await;
  save(&mut alice, &c, size("c2")).await;
  save(&mut alice, &c, size("c3")).await;
  let total = 9 * 100_329;
  for id in [&a, &b, &c] {
    assert_eq!(history_bytes(db, id).await, 3 * 100_329);
  }

  // A tenth revision doesn't fit: not as a new sketch, nor as a fork.
  let refused = create(&mut alice, "D", size("d0")).await;
  assert_eq!(refused.status, StatusCode::PAYLOAD_TOO_LARGE);
  let message = refused.json()["error"].as_str().expect("error").to_string();
  assert!(
    message.contains("Delete sketches you no longer need"),
    "{message}"
  );
  assert!(message.contains("1.0 MB"), "{message}");
  let fork = alice
    .api(
      Method::POST,
      &format!("/api/sketches/{a}/fork"),
      Value::Null,
    )
    .await;
  assert_eq!(fork.status, StatusCode::PAYLOAD_TOO_LARGE);
  let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sketches")
    .fetch_one(db)
    .await
    .expect("count");
  assert_eq!(count, 3, "nothing was created");

  // A save that grows the account past its budget is refused, with nothing
  // saved or pruned.
  let grown = put(
    &mut alice,
    &b,
    json!({"files": file_of_size("b3", 199_000)}),
  )
  .await;
  assert_eq!(grown.status, StatusCode::PAYLOAD_TOO_LARGE);
  assert!(grown.text().contains("Delete sketches you no longer need"));
  assert_eq!(revision_numbers(&mut alice, &b).await, [3, 2, 1]);
  let own = alice
    .api(Method::GET, &format!("/api/sketches/{b}"), Value::Null)
    .await
    .json();
  assert!(
    own["files"][0]["content"]
      .as_str()
      .expect("content")
      .starts_with("b2")
  );
  // One that keeps it the same size is fine, since pruning makes room.
  save(&mut alice, &b, size("b3")).await;
  assert_eq!(revision_numbers(&mut alice, &b).await, [4, 3, 2]);

  // Other accounts have their own budget, and deleting a sketch makes room.
  let mut bob = app.user("bob").await;
  assert_eq!(
    create(&mut bob, "Bob's", size("x")).await.status,
    StatusCode::CREATED
  );
  let deleted = alice
    .api(Method::DELETE, &format!("/api/sketches/{c}"), Value::Null)
    .await;
  assert_eq!(deleted.status, StatusCode::NO_CONTENT);
  assert_eq!(
    create(&mut alice, "D", size("d0")).await.status,
    StatusCode::CREATED
  );
  let account: i64 = sqlx::query_scalar(
    "SELECT SUM(history_bytes) FROM sketches s JOIN users u \
     ON u.id = s.owner_id WHERE u.username = 'alice'",
  )
  .fetch_one(db)
  .await
  .expect("account total");
  assert_eq!(account, total - 3 * 100_329 + 100_329);
}

#[test]
fn default_budgets() {
  let budget = HistoryBudget::default();
  assert_eq!(budget.per_sketch, 10 * 1024 * 1024);
  assert_eq!(budget.per_sketch, SKETCH_HISTORY_BYTES);
  assert_eq!(budget.per_account(false), 5 * 1024 * 1024);
  assert_eq!(budget.per_account(false), UNVERIFIED_ACCOUNT_HISTORY_BYTES);
  assert_eq!(budget.per_account(true), 50 * 1024 * 1024);
  assert_eq!(budget.per_account(true), VERIFIED_ACCOUNT_HISTORY_BYTES);
}

#[tokio::test]
async fn unverified_accounts_get_a_smaller_budget() {
  let app = TestApp::with_config(|config| {
    config.history = HistoryBudget {
      per_sketch: 400_000,
      per_unverified_account: 250_000,
      per_verified_account: 1_000_000,
    };
  })
  .await;
  let mut alice = app.unverified_user("alice").await;
  let size = |tag: &str| file_of_size(tag, 100_000);
  let a = id_of(&create(&mut alice, "A", size("a0")).await.json());
  save(&mut alice, &a, size("a1")).await;
  // A third revision would be 300,987 bytes, past the unverified 250,000.
  let refused = put(&mut alice, &a, json!({"files": size("a2")})).await;
  assert_eq!(refused.status, StatusCode::PAYLOAD_TOO_LARGE);
  let message = refused.json()["error"].as_str().expect("error").to_string();
  assert!(message.contains("until its email address is verified"));
  assert!(
    message.contains("Verify your email address to raise the limit to 1.0 MB"),
    "{message}"
  );
  let created = create(&mut alice, "B", size("b0")).await;
  assert_eq!(created.status, StatusCode::PAYLOAD_TOO_LARGE);
  assert!(created.text().contains("Verify your email address"));

  // Verifying raises it.
  let link = app.link_in_email(&email_of("alice"));
  assert_eq!(alice.get(&link).await.status, StatusCode::OK);
  save(&mut alice, &a, size("a2")).await;
  assert_eq!(revision_numbers(&mut alice, &a).await, [3, 2, 1]);
  assert_eq!(
    create(&mut alice, "B", size("b0")).await.status,
    StatusCode::CREATED
  );
}

/// The migrations before history budgets, for building a database as it
/// was.
async fn migrate_to_before_budgets(dir: &TempDir) {
  let migrations = dir.path().join("old-migrations");
  fs::create_dir(&migrations).expect("migrations dir");
  let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
  for name in [
    "0001_init.sql",
    "0002_revisions.sql",
    "0003_tags.sql",
    "0004_moderation_log.sql",
  ] {
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
    "INSERT INTO users (id, username, email, password_hash, created_at) \
       VALUES (1, 'alice', 'alice@example.com', 'x', 1); \
     INSERT INTO sketches (id, owner_id, title, visibility, created_at, \
       updated_at, head_revision, published_revision) VALUES \
       ('sketch01', 1, 'Old', 'public', 1, 1, 2, 1), \
       ('sketch02', 1, 'Other', 'private', 1, 1, 1, NULL); \
     INSERT INTO sketch_revisions (sketch_id, number, entry_file, \
       created_at) VALUES ('sketch01', 1, 'main.easl', 1), \
       ('sketch01', 2, 'main.easl', 2), ('sketch02', 1, 'a.easl', 1); \
     INSERT INTO revision_files (sketch_id, number, path, content, \
       position) VALUES \
       ('sketch01', 1, 'main.easl', '(café)', 0), \
       ('sketch01', 2, 'main.easl', '(café 2)', 0), \
       ('sketch01', 2, 'lib/more.easl', '(defn more [])', 1), \
       ('sketch02', 1, 'a.easl', 'é', 0);",
  )
  .execute(&pool)
  .await
  .expect("old data");
  pool.close().await;
}

fn file(path: &str, content: &str) -> SketchFile {
  SketchFile {
    path: path.into(),
    content: content.into(),
  }
}

fn contents(files: Vec<SketchFile>) -> Contents {
  Contents {
    files,
    entry_file: None,
    entry_name: None,
  }
}

#[tokio::test]
async fn migration_measures_existing_revisions() {
  let dir = TempDir::new().expect("temp dir");
  migrate_to_before_budgets(&dir).await;
  let app = TestApp::with_database_in(dir).await;
  let db = &app.state.db;

  // Sizes count bytes (`é` is two), as `revisions::size` does.
  let expected = [
    ("sketch01", 1, contents(vec![file("main.easl", "(café)")])),
    (
      "sketch01",
      2,
      contents(vec![
        file("main.easl", "(café 2)"),
        file("lib/more.easl", "(defn more [])"),
      ]),
    ),
    ("sketch02", 1, contents(vec![file("a.easl", "é")])),
  ];
  for (id, number, contents) in &expected {
    let size: i64 = sqlx::query_scalar(
      "SELECT size FROM sketch_revisions WHERE sketch_id = ? AND number = ?",
    )
    .bind(id)
    .bind(number)
    .fetch_one(db)
    .await
    .expect("size");
    assert_eq!(size, revisions::size(contents), "{id} revision {number}");
  }
  assert_eq!(revisions::size(&expected[2].2), 256 + 64 + 6 + 2);
  let first = revisions::size(&expected[0].2) + revisions::size(&expected[1].2);
  assert_eq!(history_bytes(db, "sketch01").await, first);
  assert_eq!(
    history_bytes(db, "sketch02").await,
    revisions::size(&expected[2].2)
  );
  let sent: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM email_days")
    .fetch_one(db)
    .await
    .expect("the email count table");
  assert_eq!(sent, 0);

  // Saves after the migration keep the totals.
  let saved = contents(vec![file("main.easl", "(new)")]);
  let update = SketchUpdate {
    title: "Old".into(),
    description: String::new(),
    contents: Some(saved.clone()),
    visibility: Visibility::Public,
    publish: false,
    tags: None,
  };
  sketches::update(db, "sketch01", &update, &HistoryBudget::default())
    .await
    .map_err(|e| e.message())
    .expect("save");
  assert_eq!(
    history_bytes(db, "sketch01").await,
    first + revisions::size(&saved)
  );
  // Opening it again changes nothing.
  easl_site_server::db::open(&app.database_path())
    .await
    .expect("reopen");
}
