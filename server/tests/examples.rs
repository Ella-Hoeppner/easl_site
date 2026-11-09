//! Syncing the docs' examples to sketches, and seeding demo data.

mod common;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};

use common::TestApp;
use easl_site_server::examples::{self, Example, SyncReport};
use easl_site_server::seed;
use easl_site_server::users;
use easl_site_server::validate::SketchFile;

fn example(id: &str, sketch_id: &str, title: &str, source: &str) -> Example {
  Example {
    id: id.into(),
    sketch_id: sketch_id.into(),
    title: title.into(),
    description: format!("The {title} example."),
    files: vec![SketchFile {
      path: "main.easl".into(),
      content: source.into(),
    }],
  }
}

fn report(created: usize, updated: usize, unchanged: usize) -> SyncReport {
  SyncReport {
    created,
    updated,
    unchanged,
  }
}

async fn updated_at(app: &TestApp, id: &str) -> i64 {
  sqlx::query_scalar("SELECT updated_at FROM sketches WHERE id = ?")
    .bind(id)
    .fetch_one(&app.state.db)
    .await
    .expect("updated_at")
}

#[tokio::test]
async fn sync_creates_then_updates_only_what_changed() {
  let app = TestApp::new().await;
  app.user("ella").await;
  let db = &app.state.db;
  let mut examples = vec![
    example("landing", "Ab3dE9xQ", "Landing page", "(defn main [])"),
    example("rings", "Rings123", "Rings", "(defn main [])"),
  ];
  let first = examples::sync(db, "ella", &examples).await.expect("sync");
  assert_eq!(first, report(2, 0, 0));

  let mut client = app.client();
  let sketch = client
    .api(Method::GET, "/api/sketches/Ab3dE9xQ", Value::Null)
    .await;
  assert_eq!(sketch.status, StatusCode::OK);
  let sketch = sketch.json();
  assert_eq!(sketch["owner"]["username"], "ella");
  assert_eq!(sketch["visibility"], "public");
  assert_eq!(sketch["title"], "Landing page");
  assert_eq!(sketch["description"], "The Landing page example.");
  assert_eq!(sketch["entry_file"], "main.easl");
  assert!(sketch["published_at"].is_i64());
  // Listed like any public sketch.
  assert!(client.get("/").await.page().contains("Landing page"));

  // Likes and comments survive syncs.
  let mut bob = app.user("bob").await;
  bob
    .api(Method::POST, "/api/sketches/Ab3dE9xQ/like", Value::Null)
    .await;
  bob
    .api(
      Method::POST,
      "/api/sketches/Ab3dE9xQ/comments",
      json!({"body": "nice"}),
    )
    .await;

  sqlx::query("UPDATE sketches SET updated_at = 5")
    .execute(db)
    .await
    .expect("age");
  let again = examples::sync(db, "ella", &examples).await.expect("sync");
  assert_eq!(again, report(0, 0, 2));
  assert_eq!(updated_at(&app, "Ab3dE9xQ").await, 5, "unchanged untouched");

  examples[0].title = "Landing".into();
  examples[1].files[0].content = "(defn main [] 1)".into();
  let changed = examples::sync(db, "ella", &examples).await.expect("sync");
  assert_eq!(changed, report(0, 2, 0));
  assert!(updated_at(&app, "Ab3dE9xQ").await > 5);
  let rings = client
    .api(Method::GET, "/api/sketches/Rings123", Value::Null)
    .await
    .json();
  assert_eq!(rings["files"][0]["content"], "(defn main [] 1)");
  let landing = client
    .api(Method::GET, "/api/sketches/Ab3dE9xQ", Value::Null)
    .await
    .json();
  assert_eq!(landing["title"], "Landing");
  assert_eq!(landing["like_count"], 1);
  assert_eq!(landing["comment_count"], 1);

  // Removing an example deletes nothing.
  let fewer = examples::sync(db, "ella", &examples[..1])
    .await
    .expect("sync");
  assert_eq!(fewer, report(0, 0, 1));
  let still = client.get("/s/Rings123").await;
  assert_eq!(still.status, StatusCode::OK);

  // A visibility the owner chose is kept.
  sqlx::query("UPDATE sketches SET visibility = 'unlisted' WHERE id = ?")
    .bind("Rings123")
    .execute(db)
    .await
    .expect("unlist");
  examples[1].title = "Rings again".into();
  examples::sync(db, "ella", &examples).await.expect("sync");
  let rings = client
    .api(Method::GET, "/api/sketches/Rings123", Value::Null)
    .await
    .json();
  assert_eq!(rings["visibility"], "unlisted");
  assert_eq!(rings["title"], "Rings again");
}

#[tokio::test]
async fn sync_refuses_bad_examples_before_writing_anything() {
  let app = TestApp::new().await;
  let db = &app.state.db;
  let good = example("a", "GoodOne1", "Good", "(defn main [])");
  let missing = examples::sync(db, "ella", std::slice::from_ref(&good)).await;
  let error = missing.expect_err("no owner");
  assert!(
    error.to_string().contains("no user named \"ella\""),
    "{error}"
  );

  app.user("ella").await;
  let mut bob = app.user("bob").await;
  let bobs = bob.create_sketch("Bob's", "public").await;
  let bobs_id = bobs["id"].as_str().expect("id");

  let taken = example("b", bobs_id, "Taken", "(defn main [])");
  let error = examples::sync(db, "ella", &[good.clone(), taken])
    .await
    .expect_err("someone else's id");
  assert!(format!("{error:#}").contains("belongs to bob"), "{error:#}");

  let bad_id = example("c", "short", "Bad id", "(defn main [])");
  assert!(examples::sync(db, "ella", &[bad_id]).await.is_err());
  let mut no_files = example("d", "NoFiles1", "No files", "");
  no_files.files.clear();
  assert!(examples::sync(db, "ella", &[no_files]).await.is_err());
  let twice = [good.clone(), example("e", "GoodOne1", "Again", "")];
  assert!(examples::sync(db, "ella", &twice).await.is_err());

  // None of those wrote the good example.
  let mut client = app.client();
  let status = client.get("/s/GoodOne1").await.status;
  assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn examples_load_from_the_site() {
  let app = TestApp::new().await;
  let site = &app.state.config.site_dir;
  assert!(examples::load(site).expect("load").is_none());
  app.write_examples(json!({
    "examples": [{
      "id": "landing",
      "sketch_id": "Ab3dE9xQ",
      "title": "Landing page",
      "description": "A landing.",
      "files": [{"path": "main.easl", "content": "(defn main [])"}],
    }],
  }));
  let loaded = examples::load(site).expect("load").expect("examples");
  assert_eq!(loaded.len(), 1);
  assert_eq!(loaded[0].sketch_id, "Ab3dE9xQ");
  app.write_examples(json!({"examples": "nope"}));
  assert!(examples::load(site).is_err());
}

#[tokio::test]
async fn seeding_is_repeatable() {
  let app = TestApp::new().await;
  let db = &app.state.db;
  let site = app.state.config.site_dir.clone();
  seed::seed(db, &site).await.expect("seed");
  let ella = users::by_username(db, "ella")
    .await
    .expect("db")
    .expect("ella");
  assert!(ella.is_admin);
  let counts = || async {
    let sketches: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sketches")
      .fetch_one(db)
      .await
      .expect("count");
    let playlists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM playlists")
      .fetch_one(db)
      .await
      .expect("count");
    (sketches, playlists)
  };
  let first = counts().await;
  assert_eq!(first, (8, 2), "2 built-in examples, 6 others, 2 playlists");
  seed::seed(db, &site).await.expect("seed again");
  assert_eq!(counts().await, first);

  let mut client = app.client();
  assert_eq!(client.get("/s/SeedGrad").await.status, StatusCode::OK);
  let profile = client.get("/u/alice?tab=playlists").await.page();
  assert!(profile.contains("Favorites"));
}
