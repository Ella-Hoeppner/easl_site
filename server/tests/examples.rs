//! Syncing the docs' examples to sketches, and seeding demo data.

mod common;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};

use common::TestApp;
use easl_site_server::examples::{self, Example, SyncReport};
use easl_site_server::seed;
use easl_site_server::users;
use easl_site_server::validate::{self, SketchFile};

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
  let mut ella = app.user("ella").await;
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

  // Tags the owner gave survive syncs.
  let tagged = ella
    .api(
      Method::PUT,
      "/api/sketches/Ab3dE9xQ",
      json!({"tags": ["generative", "2d"]}),
    )
    .await;
  assert_eq!(tagged.status, StatusCode::OK);

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
  assert_eq!(landing["tags"], json!(["generative", "2d"]));

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

/// The Waves123 sketch, as `client` sees it.
async fn waves(client: &mut common::Client<'_>) -> Value {
  client
    .api(Method::GET, "/api/sketches/Waves123", Value::Null)
    .await
    .json()
}

#[tokio::test]
async fn edits_on_the_site_survive_until_the_docs_change_them() {
  let app = TestApp::new().await;
  let mut ella = app.user("ella").await;
  let db = &app.state.db;
  let mut examples =
    vec![example("waves", "Waves123", "Waves", "(defn main [])")];
  examples::sync(db, "ella", &examples).await.expect("sync");

  // Ella edits the title, the description, and the code (a draft).
  let edited = ella
    .api(
      Method::PUT,
      "/api/sketches/Waves123",
      json!({
        "title": "Wavy",
        "description": "Draw on the canvas!",
        "files": [{"path": "main.easl", "content": "(defn main [] 2)"}],
      }),
    )
    .await;
  assert_eq!(edited.status, StatusCode::OK);
  let source = |sketch: &Value| sketch["files"][0]["content"].clone();

  // A sync of unchanged docs (a restart) keeps all of it, draft included.
  let again = examples::sync(db, "ella", &examples).await.expect("sync");
  assert_eq!(again, report(0, 0, 1));
  let own = waves(&mut ella).await;
  assert_eq!(own["title"], "Wavy");
  assert_eq!(own["description"], "Draw on the canvas!");
  assert_eq!(source(&own), "(defn main [] 2)");
  assert_eq!(own["has_draft"], true);

  // The docs changing the description replaces only the description.
  examples[0].description = "Waves, from the docs.".into();
  let changed = examples::sync(db, "ella", &examples).await.expect("sync");
  assert_eq!(changed, report(0, 1, 0));
  let own = waves(&mut ella).await;
  assert_eq!(own["title"], "Wavy");
  assert_eq!(own["description"], "Waves, from the docs.");
  assert_eq!(source(&own), "(defn main [] 2)");
  assert_eq!(own["has_draft"], true);

  // The docs changing the code replaces the draft, published.
  examples[0].files[0].content = "(defn main [] 3)".into();
  examples::sync(db, "ella", &examples).await.expect("sync");
  let own = waves(&mut ella).await;
  assert_eq!(source(&own), "(defn main [] 3)");
  assert_eq!(own["has_draft"], false);
  assert_eq!(own["title"], "Wavy");
}

#[tokio::test]
async fn sketches_synced_before_syncs_were_remembered_are_adopted() {
  let app = TestApp::new().await;
  let mut ella = app.user("ella").await;
  let db = &app.state.db;
  let mut examples =
    vec![example("waves", "Waves123", "Waves", "(defn main [])")];
  examples::sync(db, "ella", &examples).await.expect("sync");
  ella
    .api(
      Method::PUT,
      "/api/sketches/Waves123",
      json!({"description": "Draw on the canvas!"}),
    )
    .await;
  // As on a database synced before `example_syncs` existed.
  sqlx::query("DELETE FROM example_syncs")
    .execute(db)
    .await
    .expect("forget");
  examples[0].title = "Waves (new)".into();
  let adopted = examples::sync(db, "ella", &examples).await.expect("sync");
  assert_eq!(adopted, report(0, 0, 1));
  let own = ella
    .api(Method::GET, "/api/sketches/Waves123", Value::Null)
    .await
    .json();
  assert_eq!(own["title"], "Waves");
  assert_eq!(own["description"], "Draw on the canvas!");
  // From then on, the docs' changes come through.
  examples[0].title = "Waves (newer)".into();
  examples::sync(db, "ella", &examples).await.expect("sync");
  let own = ella
    .api(Method::GET, "/api/sketches/Waves123", Value::Null)
    .await
    .json();
  assert_eq!(own["title"], "Waves (newer)");
  assert_eq!(own["description"], "Draw on the canvas!");
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
    let tags: Vec<String> = sqlx::query_scalar("SELECT tag FROM sketch_tags")
      .fetch_all(db)
      .await
      .expect("tags");
    (sketches, playlists, tags.len())
  };
  let first = counts().await;
  assert_eq!(first.0, 8, "2 built-in examples, 6 others");
  assert_eq!(first.1, 2, "2 playlists");
  // Every sketch is tagged, with valid tags.
  let untagged: i64 = sqlx::query_scalar(
    "SELECT COUNT(*) FROM sketches s \
     WHERE NOT EXISTS (SELECT 1 FROM sketch_tags t WHERE t.sketch_id = s.id)",
  )
  .fetch_one(db)
  .await
  .expect("count");
  assert_eq!(untagged, 0);
  let all_tags: Vec<String> = sqlx::query_scalar("SELECT tag FROM sketch_tags")
    .fetch_all(db)
    .await
    .expect("tags");
  for tag in &all_tags {
    assert_eq!(validate::tag(tag).ok().as_ref(), Some(tag));
  }
  seed::seed(db, &site).await.expect("seed again");
  assert_eq!(counts().await, first);

  let mut client = app.client();
  assert_eq!(client.get("/s/SeedGrad").await.status, StatusCode::OK);
  let gradient = client
    .api(Method::GET, "/api/sketches/SeedGrad", Value::Null)
    .await
    .json();
  assert_eq!(gradient["tags"], json!(["2d", "color", "beginner"]));
  // Explore shows the tags.
  let explore = client.get("/").await.page();
  assert!(
    explore.contains("class=\"tag-chip\" href=\"/?sort=new&amp;tag=2d\"")
  );
  let profile = client.get("/u/alice?tab=playlists").await.page();
  assert!(profile.contains("Favorites"));
}
