//! Revisions, drafts, and publishing: what owners and everyone else see,
//! and the migration that gave existing sketches their first revision.

mod common;

use std::fs;
use std::path::Path;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use sqlx::SqlitePool;
use sqlx::migrate::Migrator;
use sqlx::sqlite::SqliteConnectOptions;
use tempfile::TempDir;

use common::{Client, TestApp};
use easl_site_server::auth::Viewer;
use easl_site_server::db;
use easl_site_server::examples::{self, Example};
use easl_site_server::sketches;
use easl_site_server::validate::SketchFile;

fn id_of(sketch: &Value) -> String {
  sketch["id"].as_str().expect("id").to_string()
}

/// One file named `main.easl` holding `source`.
fn main_file(source: &str) -> Value {
  json!([{"path": "main.easl", "content": source}])
}

/// The content of a sketch JSON's first file.
fn source_of(sketch: &Value) -> &str {
  sketch["files"][0]["content"].as_str().expect("content")
}

/// Saves new files, returning the owner's view.
async fn save(client: &mut Client<'_>, id: &str, source: &str) -> Value {
  let response = client
    .api(
      Method::PUT,
      &format!("/api/sketches/{id}"),
      json!({"files": main_file(source)}),
    )
    .await;
  assert_eq!(response.status, StatusCode::OK, "{}", response.text());
  response.json()
}

async fn set_visibility(
  client: &mut Client<'_>,
  id: &str,
  visibility: &str,
) -> Value {
  let response = client
    .api(
      Method::PUT,
      &format!("/api/sketches/{id}"),
      json!({"visibility": visibility}),
    )
    .await;
  assert_eq!(response.status, StatusCode::OK, "{}", response.text());
  response.json()
}

async fn get(client: &mut Client<'_>, id: &str) -> Value {
  let response = client
    .api(Method::GET, &format!("/api/sketches/{id}"), Value::Null)
    .await;
  assert_eq!(response.status, StatusCode::OK, "{}", response.text());
  response.json()
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

/// The numbers of a sketch's revisions, newest first.
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

#[tokio::test]
async fn saves_make_revisions_unless_nothing_changed() {
  let app = TestApp::new().await;
  let mut alice = app.user("alice").await;
  let sketch = alice.create_sketch("Waves", "private").await;
  let id = id_of(&sketch);
  assert_eq!(sketch["head_revision"], 1);
  assert!(sketch["published_revision"].is_null());
  assert_eq!(sketch["has_draft"], false);
  assert!(sketch["draft_saved_at"].is_i64());

  let saved = save(&mut alice, &id, "(two)").await;
  assert_eq!(saved["head_revision"], 2);
  assert_eq!(source_of(&saved), "(two)");
  // A private sketch is one thing: no published revision, no draft.
  assert!(saved["published_revision"].is_null());
  assert_eq!(saved["has_draft"], false);

  // The same files again, or only a title, make no revision.
  let same = save(&mut alice, &id, "(two)").await;
  assert_eq!(same["head_revision"], 2);
  let titled = alice
    .api(
      Method::PUT,
      &format!("/api/sketches/{id}"),
      json!({"title": "Waves 2"}),
    )
    .await
    .json();
  assert_eq!(titled["head_revision"], 2);
  assert_eq!(titled["title"], "Waves 2");

  // The entry point alone is part of the program.
  let entry = alice
    .api(
      Method::PUT,
      &format!("/api/sketches/{id}"),
      json!({"entry_name": "start"}),
    )
    .await
    .json();
  assert_eq!(entry["head_revision"], 3);
  assert_eq!(entry["entry_name"], "start");
  assert_eq!(source_of(&entry), "(two)");
  assert_eq!(revision_numbers(&mut alice, &id).await, [3, 2, 1]);

  let listed = alice
    .api(
      Method::GET,
      &format!("/api/sketches/{id}/revisions"),
      Value::Null,
    )
    .await
    .json();
  assert_eq!(listed["revisions"][0]["head"], true);
  assert_eq!(listed["revisions"][0]["published"], false);
  assert_eq!(listed["revisions"][1]["head"], false);

  // The history is the owner's alone.
  set_visibility(&mut alice, &id, "public").await;
  let mut bob = app.user("bob").await;
  let refused = bob
    .api(
      Method::GET,
      &format!("/api/sketches/{id}/revisions"),
      Value::Null,
    )
    .await;
  assert_eq!(refused.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn visibility_changes_publish_and_unpublish() {
  let app = TestApp::new().await;
  let mut alice = app.user("alice").await;
  let id = id_of(&alice.create_sketch("Waves", "private").await);
  save(&mut alice, &id, "(two)").await;

  // Leaving private publishes the head.
  let unlisted = set_visibility(&mut alice, &id, "unlisted").await;
  assert_eq!(unlisted["published_revision"], 2);
  assert_eq!(unlisted["has_draft"], false);
  assert!(unlisted["published_updated_at"].is_i64());

  // Saving moves only the head.
  let drafted = save(&mut alice, &id, "(three)").await;
  assert_eq!(drafted["head_revision"], 3);
  assert_eq!(drafted["published_revision"], 2);
  assert_eq!(drafted["has_draft"], true);

  // Between unlisted and public the published revision stays.
  let public = set_visibility(&mut alice, &id, "public").await;
  assert_eq!(public["published_revision"], 2);
  assert_eq!(public["has_draft"], true);
  assert!(public["published_at"].is_i64());
  let mut anyone = app.client();
  assert_eq!(source_of(&get(&mut anyone, &id).await), "(two)");

  // Private again: one thing, and making it public later publishes the
  // head then.
  let private = set_visibility(&mut alice, &id, "private").await;
  assert!(private["published_revision"].is_null());
  assert!(private["published_updated_at"].is_null());
  assert_eq!(private["has_draft"], false);
  assert_eq!(private["head_revision"], 3);
  let again = set_visibility(&mut alice, &id, "public").await;
  assert_eq!(again["published_revision"], 3);
  assert_eq!(source_of(&get(&mut anyone, &id).await), "(three)");

  // Created unlisted or public, revision 1 is published at once.
  let made_public = alice.create_sketch("Rings", "public").await;
  assert_eq!(made_public["published_revision"], 1);
  assert!(made_public["published_updated_at"].is_i64());
  let made_private = alice.create_sketch("Notes", "private").await;
  assert!(made_private["published_updated_at"].is_null());
}

#[tokio::test]
async fn owners_see_the_head_and_others_the_published_revision() {
  let app = TestApp::new().await;
  let mut alice = app.user("alice").await;
  let created = alice
    .api(
      Method::POST,
      "/api/sketches",
      json!({
        "title": "Waves",
        "visibility": "public",
        "files": main_file("(published)"),
        "entry_file": "main.easl",
        "entry_name": "main",
      }),
    )
    .await
    .json();
  let id = id_of(&created);
  let draft = alice
    .api(
      Method::PUT,
      &format!("/api/sketches/{id}"),
      json!({
        "files": [
          {"path": "main.easl", "content": "(draft)"},
          {"path": "lib.easl", "content": "(lib)"},
        ],
        "entry_name": "start",
      }),
    )
    .await
    .json();
  assert_eq!(draft["has_draft"], true);
  assert_eq!(draft["entry_name"], "start");

  let own = get(&mut alice, &id).await;
  assert_eq!(source_of(&own), "(draft)");
  assert_eq!(own["files"].as_array().expect("files").len(), 2);
  assert_eq!(own["entry_name"], "start");
  assert_eq!(own["has_draft"], true);
  assert!(own["draft_saved_at"].is_i64());
  let page = alice.get(&format!("/s/{id}")).await.page_json();
  assert_eq!(source_of(&page["sketch"]), "(draft)");
  assert_eq!(page["sketch"]["has_draft"], true);

  let mut bob = app.user("bob").await;
  let mut admin = app.admin("dana").await;
  let mut anyone = app.client();
  for client in [&mut bob, &mut admin, &mut anyone] {
    let seen = get(client, &id).await;
    assert_eq!(source_of(&seen), "(published)");
    assert_eq!(seen["files"].as_array().expect("files").len(), 1);
    assert_eq!(seen["entry_name"], "main");
    assert_eq!(seen["has_draft"], false);
    assert!(seen["draft_saved_at"].is_null());
    let page = client.get(&format!("/s/{id}")).await.page_json();
    assert_eq!(source_of(&page["sketch"]), "(published)");
    assert_eq!(page["sketch"]["entry_name"], "main");
  }

  // An admin looking at a private sketch (nothing published) sees its head.
  set_visibility(&mut alice, &id, "private").await;
  assert_eq!(source_of(&get(&mut admin, &id).await), "(draft)");
}

#[tokio::test]
async fn publishing_and_discarding_drafts() {
  let app = TestApp::new().await;
  let mut alice = app.user("alice").await;
  let id = id_of(&alice.create_sketch("Waves", "private").await);

  let private = alice
    .api(
      Method::POST,
      &format!("/api/sketches/{id}/publish"),
      Value::Null,
    )
    .await;
  assert_eq!(private.status, StatusCode::BAD_REQUEST);
  assert_eq!(
    private.json()["error"],
    "Make it unlisted or public to publish it."
  );

  set_visibility(&mut alice, &id, "unlisted").await;
  let no_draft = alice
    .api(
      Method::POST,
      &format!("/api/sketches/{id}/discard-draft"),
      Value::Null,
    )
    .await;
  assert_eq!(no_draft.status, StatusCode::BAD_REQUEST);

  // Publishing makes the head what everyone sees.
  save(&mut alice, &id, "(two)").await;
  sqlx::query("UPDATE sketches SET published_updated_at = 5")
    .execute(&app.state.db)
    .await
    .expect("age");
  let published = post(&mut alice, &id, "publish").await;
  assert_eq!(published["published_revision"], 2);
  assert_eq!(published["head_revision"], 2);
  assert_eq!(published["has_draft"], false);
  assert!(published["published_updated_at"].as_i64().expect("time") > 5);
  let mut anyone = app.client();
  assert_eq!(source_of(&get(&mut anyone, &id).await), "(two)");
  // Publishing with no draft changes nothing.
  assert_eq!(
    post(&mut alice, &id, "publish").await["published_revision"],
    2
  );

  // Discarding goes back to the published revision and keeps the history;
  // the next save is numbered after the discarded ones.
  save(&mut alice, &id, "(three)").await;
  save(&mut alice, &id, "(four)").await;
  let discarded = post(&mut alice, &id, "discard-draft").await;
  assert_eq!(discarded["head_revision"], 2);
  assert_eq!(discarded["has_draft"], false);
  assert_eq!(source_of(&discarded), "(two)");
  assert_eq!(revision_numbers(&mut alice, &id).await, [4, 3, 2, 1]);
  let next = save(&mut alice, &id, "(five)").await;
  assert_eq!(next["head_revision"], 5);
  assert_eq!(next["published_revision"], 2);

  // Saving what's published after a discard makes no revision.
  post(&mut alice, &id, "discard-draft").await;
  assert_eq!(save(&mut alice, &id, "(two)").await["head_revision"], 2);

  // Only the owner publishes or discards.
  let mut bob = app.user("bob").await;
  for action in ["publish", "discard-draft"] {
    let refused = bob
      .api(
        Method::POST,
        &format!("/api/sketches/{id}/{action}"),
        Value::Null,
      )
      .await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{action}");
  }
}

#[tokio::test]
async fn the_published_endpoint() {
  let app = TestApp::new().await;
  let mut alice = app.user("alice").await;
  let id = id_of(&alice.create_sketch("Waves", "unlisted").await);
  save(&mut alice, &id, "(draft)").await;
  let path = format!("/api/sketches/{id}/published");

  let mut anyone = app.client();
  let mut bob = app.user("bob").await;
  for client in [&mut anyone, &mut bob, &mut alice] {
    let response = client.api(Method::GET, &path, Value::Null).await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.text());
    let published = response.json();
    assert_eq!(published["revision"], 1);
    assert_eq!(published["files"], main_file("(defn main [])"));
    assert!(published["entry_file"].is_null());
    assert!(published["entry_name"].is_null());
    assert!(published["published_at"].is_i64());
  }

  // Private: invisible to others, and nothing published for the owner.
  set_visibility(&mut alice, &id, "private").await;
  for client in [&mut anyone, &mut bob, &mut alice] {
    let response = client.api(Method::GET, &path, Value::Null).await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);
  }
}

#[tokio::test]
async fn forks_copy_what_the_forker_sees() {
  let app = TestApp::new().await;
  let mut alice = app.user("alice").await;
  let id = id_of(&alice.create_sketch("Waves", "public").await);
  alice
    .api(
      Method::PUT,
      &format!("/api/sketches/{id}"),
      json!({"files": main_file("(draft)"), "entry_name": "start"}),
    )
    .await;

  let mut bob = app.user("bob").await;
  let fork_path = format!("/api/sketches/{id}/fork");
  let theirs = bob.api(Method::POST, &fork_path, Value::Null).await.json();
  assert_eq!(source_of(&theirs), "(defn main [])");
  assert!(theirs["entry_name"].is_null());
  assert_eq!(theirs["head_revision"], 1);
  assert!(theirs["published_revision"].is_null());

  let own = alice
    .api(Method::POST, &fork_path, Value::Null)
    .await
    .json();
  assert_eq!(source_of(&own), "(draft)");
  assert_eq!(own["entry_name"], "start");
}

#[tokio::test]
async fn profiles_order_by_what_viewers_see() {
  let app = TestApp::new().await;
  let mut alice = app.user("alice").await;
  let older = id_of(&alice.create_sketch("Older one", "public").await);
  let newer = id_of(&alice.create_sketch("Newer one", "public").await);
  sqlx::query(
    "UPDATE sketches SET updated_at = 10, published_updated_at = 10 \
     WHERE id = ?",
  )
  .bind(&older)
  .execute(&app.state.db)
  .await
  .expect("age");
  sqlx::query(
    "UPDATE sketches SET updated_at = 20, published_updated_at = 20 \
     WHERE id = ?",
  )
  .bind(&newer)
  .execute(&app.state.db)
  .await
  .expect("age");

  // A draft moves the sketch up for its owner only.
  save(&mut alice, &older, "(draft)").await;
  let own = alice.get("/u/alice").await.text();
  assert!(own.find("Older one") < own.find("Newer one"));
  let others = app.client().get("/u/alice").await.text();
  assert!(others.find("Newer one") < others.find("Older one"));

  // Publishing it moves it up for everyone.
  post(&mut alice, &older, "publish").await;
  let others = app.client().get("/u/alice").await.text();
  assert!(others.find("Older one") < others.find("Newer one"));
}

fn example(source: &str) -> Example {
  Example {
    id: "waves".into(),
    sketch_id: "Waves123".into(),
    title: "Waves".into(),
    description: String::new(),
    files: vec![SketchFile {
      path: "main.easl".into(),
      content: source.into(),
    }],
  }
}

#[tokio::test]
async fn example_syncs_publish_their_files() {
  let app = TestApp::new().await;
  let mut ella = app.user("ella").await;
  let db = &app.state.db;
  examples::sync(db, "ella", &[example("(one)")])
    .await
    .expect("sync");
  let id = "Waves123";
  assert_eq!(get(&mut ella, id).await["published_revision"], 1);

  // A draft of Ella's is replaced by the example's files, published.
  save(&mut ella, id, "(draft)").await;
  let report = examples::sync(db, "ella", &[example("(two)")])
    .await
    .expect("sync");
  assert_eq!(report.updated, 1);
  let own = get(&mut ella, id).await;
  assert_eq!(own["head_revision"], 3);
  assert_eq!(own["published_revision"], 3);
  assert_eq!(source_of(&own), "(two)");
  let mut anyone = app.client();
  assert_eq!(source_of(&get(&mut anyone, id).await), "(two)");

  // A draft identical to the example only needs publishing.
  set_visibility(&mut ella, id, "unlisted").await;
  save(&mut ella, id, "(three)").await;
  let report = examples::sync(db, "ella", &[example("(three)")])
    .await
    .expect("sync");
  assert_eq!(report.updated, 1);
  let own = get(&mut ella, id).await;
  assert_eq!(own["head_revision"], 4);
  assert_eq!(own["published_revision"], 4);
  assert_eq!(own["visibility"], "unlisted");
  let report = examples::sync(db, "ella", &[example("(three)")])
    .await
    .expect("sync");
  assert_eq!(report.unchanged, 1);
}

/// A database at the first migration, before revisions existed.
async fn database_before_revisions(dir: &Path) -> SqlitePool {
  let migrations = dir.join("migrations");
  fs::create_dir_all(&migrations).expect("migrations dir");
  let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
  fs::copy(
    manifest.join("migrations/0001_init.sql"),
    migrations.join("0001_init.sql"),
  )
  .expect("copy the first migration");
  let options = SqliteConnectOptions::new()
    .filename(dir.join("old.db"))
    .create_if_missing(true)
    .foreign_keys(true);
  let pool = SqlitePool::connect_with(options).await.expect("open");
  Migrator::new(migrations.as_path())
    .await
    .expect("migrator")
    .run(&pool)
    .await
    .expect("first migration");
  pool
}

#[tokio::test]
async fn migration_makes_existing_files_revision_one() {
  let dir = TempDir::new().expect("temp dir");
  let old = database_before_revisions(dir.path()).await;
  sqlx::query(
    "INSERT INTO users (id, username, email, password_hash, created_at) \
     VALUES (1, 'alice', 'alice@example.com', 'x', 0)",
  )
  .execute(&old)
  .await
  .expect("user");
  for (id, visibility, updated_at) in [
    ("Private1", "private", 100),
    ("Unlisted", "unlisted", 200),
    ("Public12", "public", 300),
  ] {
    sqlx::query(
      "INSERT INTO sketches (id, owner_id, title, visibility, entry_file, \
       entry_name, created_at, updated_at) \
       VALUES (?, 1, ?, ?, 'b.easl', 'start', 0, ?)",
    )
    .bind(id)
    .bind(id)
    .bind(visibility)
    .bind(updated_at)
    .execute(&old)
    .await
    .expect("sketch");
    for (position, path) in ["b.easl", "a.easl"].iter().enumerate() {
      sqlx::query(
        "INSERT INTO sketch_files (sketch_id, path, content, position) \
         VALUES (?, ?, ?, ?)",
      )
      .bind(id)
      .bind(path)
      .bind(format!("({id} {path})"))
      .bind(position as i64)
      .execute(&old)
      .await
      .expect("file");
    }
  }
  old.close().await;

  let db = db::open(&dir.path().join("old.db")).await.expect("migrate");
  let owner = Viewer {
    id: Some(1),
    is_admin: false,
  };
  let anyone = Viewer::of(None);
  for (id, published) in
    [("Private1", false), ("Unlisted", true), ("Public12", true)]
  {
    let row = sketches::find(&db, id, &owner)
      .await
      .expect("find")
      .expect("a sketch");
    assert_eq!(row.head_revision, 1);
    assert_eq!(row.published_revision, published.then_some(1));
    assert_eq!(row.published_updated_at.is_some(), published);
    assert_eq!(row.head_saved_at, row.updated_at);
    let viewer = if published { &anyone } else { &owner };
    let sketch = row.detail(&db, viewer).await.expect("detail");
    let paths: Vec<_> = sketch.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, ["b.easl", "a.easl"]);
    assert_eq!(sketch.files[0].content, format!("({id} b.easl)"));
    assert_eq!(sketch.summary.entry_file.as_deref(), Some("b.easl"));
    assert_eq!(sketch.summary.entry_name.as_deref(), Some("start"));
  }
  let old_table: Option<String> = sqlx::query_scalar(
    "SELECT name FROM sqlite_master WHERE name = 'sketch_files'",
  )
  .fetch_optional(&db)
  .await
  .expect("schema");
  assert!(old_table.is_none());
}
