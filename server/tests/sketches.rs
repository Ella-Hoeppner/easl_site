//! Sketches: CRUD, visibility, validation, forks, thumbnails, likes.

mod common;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};

use common::TestApp;

fn id_of(sketch: &Value) -> String {
  sketch["id"].as_str().expect("id").to_string()
}

#[tokio::test]
async fn create_read_update_delete() {
  let app = TestApp::new().await;
  let mut alice = app.user("alice").await;
  let created = alice
    .api(
      Method::POST,
      "/api/sketches",
      json!({
        "title": "  Waves  ",
        "description": "Sine waves",
        "files": [
          {"path": "main.easl", "content": "(main)"},
          {"path": "lib/noise.easl", "content": "(noise)"},
        ],
        "entry_file": "main.easl",
        "entry_name": "main",
      }),
    )
    .await;
  assert_eq!(created.status, StatusCode::CREATED);
  let sketch = created.json();
  let id = id_of(&sketch);
  assert_eq!(id.len(), 8);
  assert_eq!(sketch["title"], "Waves");
  assert_eq!(sketch["visibility"], "private");
  assert_eq!(sketch["owner"]["username"], "alice");
  assert_eq!(sketch["files"][1]["path"], "lib/noise.easl");
  assert_eq!(sketch["can_edit"], true);
  assert_eq!(sketch["liked"], false);
  assert!(sketch["published_at"].is_null());
  assert!(sketch["forked_from"].is_null());
  assert_eq!(sketch["url"], format!("/s/{id}"));
  assert!(
    sketch["thumbnail_url"]
      .as_str()
      .expect("thumb")
      .starts_with(&format!("/s/{id}/thumb?v="))
  );

  let path = format!("/api/sketches/{id}");
  let updated = alice
    .api(
      Method::PUT,
      &path,
      json!({
        "title": "Waves 2",
        "files": [{"path": "app.easl", "content": "(app)"}],
        "entry_file": "app.easl",
        "entry_name": null,
        "visibility": "public",
      }),
    )
    .await;
  assert_eq!(updated.status, StatusCode::OK, "{}", updated.text());
  let sketch = updated.json();
  assert_eq!(sketch["title"], "Waves 2");
  assert_eq!(sketch["description"], "Sine waves");
  assert_eq!(sketch["files"].as_array().expect("files").len(), 1);
  assert_eq!(sketch["entry_file"], "app.easl");
  assert!(sketch["entry_name"].is_null());
  assert!(sketch["published_at"].is_number());

  // Replacing the files without the entry file is an error.
  let dangling = alice
    .api(
      Method::PUT,
      &path,
      json!({"files": [{"path": "other.easl", "content": ""}]}),
    )
    .await;
  assert_eq!(dangling.status, StatusCode::BAD_REQUEST);

  let mut bob = app.user("bob").await;
  let forbidden = bob.api(Method::PUT, &path, json!({"title": "mine"})).await;
  assert_eq!(forbidden.status, StatusCode::FORBIDDEN);
  let not_deleted = bob.api(Method::DELETE, &path, Value::Null).await;
  assert_eq!(not_deleted.status, StatusCode::FORBIDDEN);

  let deleted = alice.api(Method::DELETE, &path, Value::Null).await;
  assert_eq!(deleted.status, StatusCode::NO_CONTENT);
  let gone = alice.api(Method::GET, &path, Value::Null).await;
  assert_eq!(gone.status, StatusCode::NOT_FOUND);
  assert_eq!(gone.json()["error"], "Not found.");
}

#[tokio::test]
async fn sign_in_required_to_create() {
  let app = TestApp::new().await;
  let response = app
    .client()
    .api(
      Method::POST,
      "/api/sketches",
      json!({"title": "x", "files": [{"path": "a.easl", "content": ""}]}),
    )
    .await;
  assert_eq!(response.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn publishing_needs_a_verified_email() {
  let app = TestApp::new().await;
  let mut carol = app.unverified_user("carol").await;
  let files = json!([{"path": "a.easl", "content": ""}]);
  let public = carol
    .api(
      Method::POST,
      "/api/sketches",
      json!({"title": "x", "files": files, "visibility": "public"}),
    )
    .await;
  assert_eq!(public.status, StatusCode::FORBIDDEN);
  assert!(
    public.json()["error"]
      .as_str()
      .expect("error")
      .contains("Verify")
  );
  let sketch = carol.create_sketch("x", "unlisted").await;
  let made_public = carol
    .api(
      Method::PUT,
      &format!("/api/sketches/{}", id_of(&sketch)),
      json!({"visibility": "public"}),
    )
    .await;
  assert_eq!(made_public.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn visibility_rules() {
  let app = TestApp::new().await;
  let mut alice = app.user("alice").await;
  let private = id_of(&alice.create_sketch("Private one", "private").await);
  let unlisted = id_of(&alice.create_sketch("Unlisted one", "unlisted").await);
  let public = id_of(&alice.create_sketch("Public one", "public").await);
  let mut bob = app.user("bob").await;
  let mut anonymous = app.client();

  for client in [&mut bob, &mut anonymous] {
    let get = |id: &str| format!("/api/sketches/{id}");
    assert_eq!(
      client
        .api(Method::GET, &get(&private), Value::Null)
        .await
        .status,
      StatusCode::NOT_FOUND
    );
    assert_eq!(
      client.get(&format!("/s/{private}")).await.status,
      StatusCode::NOT_FOUND
    );
    assert_eq!(
      client.get(&format!("/s/{private}/thumb")).await.status,
      StatusCode::NOT_FOUND
    );
    assert_eq!(
      client
        .api(Method::GET, &get(&unlisted), Value::Null)
        .await
        .status,
      StatusCode::OK
    );
    assert_eq!(
      client
        .api(Method::GET, &get(&public), Value::Null)
        .await
        .status,
      StatusCode::OK
    );
    let explore = client.get("/explore").await.text();
    assert!(explore.contains("Public one"));
    assert!(!explore.contains("Unlisted one"));
    assert!(!explore.contains("Private one"));
    let profile = client.get("/u/alice").await.text();
    assert!(profile.contains("Public one"));
    assert!(!profile.contains("Unlisted one"));
    assert!(!profile.contains("Private one"));
  }

  // The owner sees everything on their profile, labeled.
  let own = alice.get("/u/alice").await.text();
  assert!(own.contains("Private one"));
  assert!(own.contains("Unlisted one"));
  assert!(own.contains("badge-private"));
  assert!(own.contains("badge-unlisted"));
  assert_eq!(
    alice
      .api(
        Method::GET,
        &format!("/api/sketches/{private}"),
        Value::Null
      )
      .await
      .status,
    StatusCode::OK
  );

  // Admins can see private sketches.
  let mut admin = app.admin("dana").await;
  assert_eq!(
    admin
      .api(
        Method::GET,
        &format!("/api/sketches/{private}"),
        Value::Null
      )
      .await
      .status,
    StatusCode::OK
  );
}

#[tokio::test]
async fn hidden_sketches() {
  let app = TestApp::new().await;
  let mut alice = app.user("alice").await;
  let id = id_of(&alice.create_sketch("Spam", "public").await);
  let mut admin = app.admin("dana").await;
  let mut bob = app.user("bob").await;

  let not_admin = bob
    .api(
      Method::POST,
      &format!("/api/admin/sketches/{id}/hide"),
      Value::Null,
    )
    .await;
  assert_eq!(not_admin.status, StatusCode::FORBIDDEN);

  let hidden = admin
    .api(
      Method::POST,
      &format!("/api/admin/sketches/{id}/hide"),
      Value::Null,
    )
    .await;
  assert_eq!(hidden.status, StatusCode::NO_CONTENT);
  let path = format!("/api/sketches/{id}");
  assert_eq!(
    bob.api(Method::GET, &path, Value::Null).await.status,
    StatusCode::NOT_FOUND
  );
  assert!(!bob.get("/explore").await.text().contains("Spam"));
  let own = alice.api(Method::GET, &path, Value::Null).await.json();
  assert_eq!(own["hidden"], true);
  assert_eq!(
    admin.api(Method::GET, &path, Value::Null).await.status,
    StatusCode::OK
  );

  admin
    .api(
      Method::POST,
      &format!("/api/admin/sketches/{id}/unhide"),
      Value::Null,
    )
    .await;
  assert_eq!(
    bob.api(Method::GET, &path, Value::Null).await.status,
    StatusCode::OK
  );
  assert!(bob.get("/explore").await.text().contains("Spam"));
}

#[tokio::test]
async fn file_validation() {
  let app = TestApp::new().await;
  let mut alice = app.user("alice").await;
  let too_big = "x".repeat(200 * 1024 + 1);
  let cases = vec![
    (json!([]), StatusCode::BAD_REQUEST),
    (json!([{"path": "/main.easl", "content": ""}]), StatusCode::BAD_REQUEST),
    (json!([{"path": "../main.easl", "content": ""}]), StatusCode::BAD_REQUEST),
    (json!([{"path": "a/./b.easl", "content": ""}]), StatusCode::BAD_REQUEST),
    (json!([{"path": "main.txt", "content": ""}]), StatusCode::BAD_REQUEST),
    (json!([{"path": "ma in.easl", "content": ""}]), StatusCode::BAD_REQUEST),
    (
      json!([{"path": "a.easl", "content": ""}, {"path": "a.easl", "content": ""}]),
      StatusCode::BAD_REQUEST,
    ),
    (
      json!([{"path": "a.easl", "content": too_big}]),
      StatusCode::PAYLOAD_TOO_LARGE,
    ),
    (
      Value::Array(
        (0..33)
          .map(|i| json!({"path": format!("f{i}.easl"), "content": ""}))
          .collect(),
      ),
      StatusCode::BAD_REQUEST,
    ),
    (
      Value::Array(
        (0..6)
          .map(|i| json!({"path": format!("f{i}.easl"), "content": "y".repeat(190 * 1024)}))
          .collect(),
      ),
      StatusCode::PAYLOAD_TOO_LARGE,
    ),
  ];
  for (files, status) in cases {
    let response = alice
      .api(
        Method::POST,
        "/api/sketches",
        json!({"title": "t", "files": files}),
      )
      .await;
    assert_eq!(response.status, status, "{}", response.text());
    assert!(response.json()["error"].is_string());
  }
  let bad_entry = alice
    .api(
      Method::POST,
      "/api/sketches",
      json!({
        "title": "t",
        "files": [{"path": "a.easl", "content": ""}],
        "entry_file": "b.easl",
      }),
    )
    .await;
  assert_eq!(bad_entry.status, StatusCode::BAD_REQUEST);
  let bad_name = alice
    .api(
      Method::POST,
      "/api/sketches",
      json!({
        "title": "t",
        "files": [{"path": "a.easl", "content": ""}],
        "entry_name": "(evil)",
      }),
    )
    .await;
  assert_eq!(bad_name.status, StatusCode::BAD_REQUEST);
  for title in ["", "   ", &"x".repeat(101)] {
    let response = alice
      .api(
        Method::POST,
        "/api/sketches",
        json!({"title": title, "files": [{"path": "a.easl", "content": ""}]}),
      )
      .await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST, "{title:?}");
  }
  let not_json = alice
    .raw(
      Method::POST,
      "/api/sketches",
      "application/json",
      b"{".to_vec(),
    )
    .await;
  assert_eq!(not_json.status, StatusCode::BAD_REQUEST);
  assert!(not_json.json()["error"].is_string());
  let bad_visibility = alice
    .api(
      Method::POST,
      "/api/sketches",
      json!({"title": "t", "visibility": "secret", "files": [{"path": "a.easl", "content": ""}]}),
    )
    .await;
  assert!(bad_visibility.status.is_client_error());
}

#[tokio::test]
async fn forks() {
  let app = TestApp::new().await;
  let mut alice = app.user("alice").await;
  let original = alice
    .api(
      Method::POST,
      "/api/sketches",
      json!({
        "title": "Original",
        "visibility": "public",
        "files": [{"path": "main.easl", "content": "(original)"}],
        "entry_file": "main.easl",
        "entry_name": "start",
      }),
    )
    .await
    .json();
  let id = id_of(&original);
  let mut bob = app.user("bob").await;
  let fork = bob
    .api(
      Method::POST,
      &format!("/api/sketches/{id}/fork"),
      Value::Null,
    )
    .await;
  assert_eq!(fork.status, StatusCode::CREATED, "{}", fork.text());
  let fork = fork.json();
  assert_eq!(fork["title"], "Fork of Original");
  assert_eq!(fork["visibility"], "private");
  assert_eq!(fork["owner"]["username"], "bob");
  assert_eq!(fork["files"][0]["content"], "(original)");
  assert_eq!(fork["entry_name"], "start");
  assert_eq!(fork["forked_from"]["id"], id.as_str());
  assert_eq!(fork["forked_from"]["title"], "Original");
  assert_eq!(fork["forked_from"]["owner_username"], "alice");

  let custom = bob
    .api(
      Method::POST,
      &format!("/api/sketches/{id}/fork"),
      json!({"title": "Mine", "files": [{"path": "x.easl", "content": "(x)"}], "entry_file": "x.easl"}),
    )
    .await
    .json();
  assert_eq!(custom["title"], "Mine");
  assert_eq!(custom["files"][0]["path"], "x.easl");

  // A private sketch can't be forked by others, and once the original goes
  // private, forks no longer reveal it.
  alice
    .api(
      Method::PUT,
      &format!("/api/sketches/{id}"),
      json!({"visibility": "private"}),
    )
    .await;
  let refused = bob
    .api(
      Method::POST,
      &format!("/api/sketches/{id}/fork"),
      Value::Null,
    )
    .await;
  assert_eq!(refused.status, StatusCode::NOT_FOUND);
  let fork_now = bob
    .api(
      Method::GET,
      &format!("/api/sketches/{}", id_of(&fork)),
      Value::Null,
    )
    .await
    .json();
  assert!(fork_now["forked_from"].is_null());

  // Deleting the original keeps the fork.
  alice
    .api(Method::DELETE, &format!("/api/sketches/{id}"), Value::Null)
    .await;
  let survived = bob
    .api(
      Method::GET,
      &format!("/api/sketches/{}", id_of(&fork)),
      Value::Null,
    )
    .await;
  assert_eq!(survived.status, StatusCode::OK);
}

#[tokio::test]
async fn thumbnails() {
  let app = TestApp::new().await;
  let mut alice = app.user("alice").await;
  let id = id_of(&alice.create_sketch("Thumb <test>", "public").await);
  let path = format!("/api/sketches/{id}/thumbnail");

  let placeholder = alice.get(&format!("/s/{id}/thumb")).await;
  assert_eq!(placeholder.status, StatusCode::OK);
  assert_eq!(
    placeholder.header("content-type").as_deref(),
    Some("image/svg+xml")
  );
  assert!(placeholder.text().contains("Thumb &lt;test&gt;"));

  let png = b"\x89PNG\r\n\x1a\nrest of the image".to_vec();
  let wrong_type = alice
    .raw(Method::PUT, &path, "image/jpeg", png.clone())
    .await;
  assert_eq!(wrong_type.status, StatusCode::BAD_REQUEST);
  let gif = alice
    .raw(Method::PUT, &path, "image/gif", b"GIF89a....".to_vec())
    .await;
  assert_eq!(gif.status, StatusCode::BAD_REQUEST);
  let svg = alice
    .raw(Method::PUT, &path, "image/svg+xml", b"<svg/>".to_vec())
    .await;
  assert_eq!(svg.status, StatusCode::BAD_REQUEST);
  let mut big = png.clone();
  big.resize(300 * 1024 + 1, 0);
  let too_big = alice.raw(Method::PUT, &path, "image/png", big).await;
  assert_eq!(too_big.status, StatusCode::PAYLOAD_TOO_LARGE);
  let mut bob = app.user("bob").await;
  let not_owner = bob.raw(Method::PUT, &path, "image/png", png.clone()).await;
  assert_eq!(not_owner.status, StatusCode::FORBIDDEN);

  let ok = alice
    .raw(Method::PUT, &path, "image/png", png.clone())
    .await;
  assert_eq!(ok.status, StatusCode::NO_CONTENT);
  let sketch = alice
    .api(Method::GET, &format!("/api/sketches/{id}"), Value::Null)
    .await
    .json();
  let url = sketch["thumbnail_url"].as_str().expect("url").to_string();
  let served = bob.get(&url).await;
  assert_eq!(served.status, StatusCode::OK);
  assert_eq!(served.header("content-type").as_deref(), Some("image/png"));
  assert_eq!(served.body, png);
  assert!(
    served
      .header("cache-control")
      .expect("cache")
      .contains("immutable")
  );
  assert_eq!(
    served.header("x-content-type-options").as_deref(),
    Some("nosniff")
  );

  let webp = b"RIFF\x10\0\0\0WEBPVP8 ".to_vec();
  assert_eq!(
    alice
      .raw(Method::PUT, &path, "image/webp", webp)
      .await
      .status,
    StatusCode::NO_CONTENT
  );
  let jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0, 0];
  assert_eq!(
    alice
      .raw(Method::PUT, &path, "image/jpeg", jpeg)
      .await
      .status,
    StatusCode::NO_CONTENT
  );
}

#[tokio::test]
async fn likes() {
  let app = TestApp::new().await;
  let mut alice = app.user("alice").await;
  let id = id_of(&alice.create_sketch("Likeable", "public").await);
  let private = id_of(&alice.create_sketch("Private", "private").await);
  let mut bob = app.user("bob").await;
  let like = format!("/api/sketches/{id}/like");

  let liked = bob.api(Method::POST, &like, Value::Null).await.json();
  assert_eq!(liked, json!({"liked": true, "like_count": 1}));
  // Liking twice counts once.
  let again = bob.api(Method::POST, &like, Value::Null).await.json();
  assert_eq!(again["like_count"], 1);
  let alice_likes = alice.api(Method::POST, &like, Value::Null).await.json();
  assert_eq!(alice_likes["like_count"], 2);
  let sketch = bob
    .api(Method::GET, &format!("/api/sketches/{id}"), Value::Null)
    .await
    .json();
  assert_eq!(sketch["liked"], true);
  assert_eq!(sketch["like_count"], 2);

  let unliked = bob.api(Method::DELETE, &like, Value::Null).await.json();
  assert_eq!(unliked, json!({"liked": false, "like_count": 1}));
  let unliked = bob.api(Method::DELETE, &like, Value::Null).await.json();
  assert_eq!(unliked["like_count"], 1);

  let anonymous = app.client().api(Method::POST, &like, Value::Null).await;
  assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);
  let hidden = bob
    .api(
      Method::POST,
      &format!("/api/sketches/{private}/like"),
      Value::Null,
    )
    .await;
  assert_eq!(hidden.status, StatusCode::NOT_FOUND);

  // Likes show on the liker's profile.
  bob.api(Method::POST, &like, Value::Null).await;
  let tab = app.client().get("/u/bob?tab=likes").await.text();
  assert!(tab.contains("Likeable"));
}

#[tokio::test]
async fn me_sketches_and_editor_page() {
  let app = TestApp::new().await;
  let mut alice = app.user("alice").await;
  let first = id_of(&alice.create_sketch("First", "private").await);
  let second =
    id_of(&alice.create_sketch("Second </script><b>", "public").await);
  let list = alice
    .api(Method::GET, "/api/me/sketches?limit=1", Value::Null)
    .await
    .json();
  let sketches = list["sketches"].as_array().expect("sketches");
  assert_eq!(sketches.len(), 1);
  assert_eq!(sketches[0]["id"], second.as_str());
  assert!(sketches[0]["thumbnail_url"].is_string());

  let page = alice.get(&format!("/s/{second}")).await;
  assert_eq!(page.status, StatusCode::OK);
  assert!(!page.text().contains("</script><b>"));
  let data = page.page_json();
  assert_eq!(data["mode"], "sketch");
  assert_eq!(data["user"]["username"], "alice");
  assert_eq!(data["sketch"]["title"], "Second </script><b>");
  assert_eq!(data["sketch"]["files"][0]["path"], "main.easl");
  assert_eq!(data["csrf_ok"], true);
  assert!(data["playlist"].is_null());
  let text = page.text();
  assert!(text.contains("monaco-editor@0.99.0"));
  assert!(text.contains("?v=abc123"));
  assert!(page.page().contains("http://test.local/s/"));
  assert_eq!(page.header("cache-control").as_deref(), Some("no-store"));

  // The owner's views don't count; others' count once an hour.
  let mut bob = app.user("bob").await;
  bob.get(&format!("/s/{second}")).await;
  bob.get(&format!("/s/{second}")).await;
  let sketch = alice
    .api(Method::GET, &format!("/api/sketches/{second}"), Value::Null)
    .await
    .json();
  assert_eq!(sketch["view_count"], 1);

  assert_eq!(
    bob.get(&format!("/s/{first}")).await.status,
    StatusCode::NOT_FOUND
  );
}
