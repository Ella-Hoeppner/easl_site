//! Comments and playlists.

mod common;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};

use common::TestApp;

fn id_of(value: &Value) -> String {
  value["id"].as_str().expect("id").to_string()
}

fn ids(items: &Value) -> Vec<String> {
  items.as_array().expect("array").iter().map(id_of).collect()
}

#[tokio::test]
async fn comments_permissions_and_counts() {
  let app = TestApp::new().await;
  let mut alice = app.user("alice").await;
  let id = id_of(&alice.create_sketch("Commented", "public").await);
  let comments = format!("/api/sketches/{id}/comments");
  let mut bob = app.user("bob").await;
  let mut carol = app.user("carol").await;
  let mut unverified = app.unverified_user("dave").await;

  let refused = unverified
    .api(Method::POST, &comments, json!({"body": "hi"}))
    .await;
  assert_eq!(refused.status, StatusCode::FORBIDDEN);
  let anonymous = app
    .client()
    .api(Method::POST, &comments, json!({"body": "x"}))
    .await;
  assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);
  for body in ["", "   ", &"x".repeat(2001)] {
    let bad = bob
      .api(Method::POST, &comments, json!({"body": body}))
      .await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
  }

  let first = bob
    .api(Method::POST, &comments, json!({"body": "  first!  "}))
    .await;
  assert_eq!(first.status, StatusCode::CREATED);
  let first = first.json();
  assert_eq!(first["body"], "first!");
  assert_eq!(first["author"]["username"], "bob");
  assert_eq!(first["can_delete"], true);
  let second = carol
    .api(Method::POST, &comments, json!({"body": "second"}))
    .await
    .json();

  let list = app
    .client()
    .api(Method::GET, &comments, Value::Null)
    .await
    .json();
  let list = list["comments"].as_array().expect("comments").clone();
  assert_eq!(list.len(), 2);
  assert_eq!(list[0]["body"], "first!");
  assert_eq!(list[0]["can_delete"], false);
  let count = |sketch: Value| sketch["comment_count"].as_i64().expect("count");
  let path = format!("/api/sketches/{id}");
  assert_eq!(
    count(bob.api(Method::GET, &path, Value::Null).await.json()),
    2
  );

  // Carol can't delete Bob's comment; the sketch's owner can.
  let first_path = format!("/api/comments/{}", first["id"]);
  let denied = carol.api(Method::DELETE, &first_path, Value::Null).await;
  assert_eq!(denied.status, StatusCode::FORBIDDEN);
  let deleted = alice.api(Method::DELETE, &first_path, Value::Null).await;
  assert_eq!(deleted.status, StatusCode::NO_CONTENT);
  let again = alice.api(Method::DELETE, &first_path, Value::Null).await;
  assert_eq!(again.status, StatusCode::NOT_FOUND);
  // Authors can delete their own.
  let own = carol
    .api(
      Method::DELETE,
      &format!("/api/comments/{}", second["id"]),
      Value::Null,
    )
    .await;
  assert_eq!(own.status, StatusCode::NO_CONTENT);

  let list = bob.api(Method::GET, &comments, Value::Null).await.json();
  assert!(list["comments"].as_array().expect("comments").is_empty());
  assert_eq!(
    count(bob.api(Method::GET, &path, Value::Null).await.json()),
    0
  );

  // Comments on a private sketch are as hidden as the sketch.
  let private = id_of(&alice.create_sketch("Private", "private").await);
  let on_private = bob
    .api(
      Method::POST,
      &format!("/api/sketches/{private}/comments"),
      json!({"body": "x"}),
    )
    .await;
  assert_eq!(on_private.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn admins_can_delete_any_comment() {
  let app = TestApp::new().await;
  let mut alice = app.user("alice").await;
  let id = id_of(&alice.create_sketch("S", "public").await);
  let comment = alice
    .api(
      Method::POST,
      &format!("/api/sketches/{id}/comments"),
      json!({"body": "x"}),
    )
    .await
    .json();
  let mut admin = app.admin("dana").await;
  let response = admin
    .api(
      Method::DELETE,
      &format!("/api/comments/{}", comment["id"]),
      Value::Null,
    )
    .await;
  assert_eq!(response.status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn playlists_crud_and_ordering() {
  let app = TestApp::new().await;
  let mut alice = app.user("alice").await;
  let a = id_of(&alice.create_sketch("A", "public").await);
  let b = id_of(&alice.create_sketch("B", "public").await);
  let c = id_of(&alice.create_sketch("C", "unlisted").await);

  let created = alice
    .api(
      Method::POST,
      "/api/playlists",
      json!({"title": "Mix", "description": "Good stuff", "sketch_id": a}),
    )
    .await;
  assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
  let playlist = created.json();
  let pid = id_of(&playlist);
  assert_eq!(playlist["visibility"], "private");
  assert_eq!(playlist["can_edit"], true);
  assert_eq!(playlist["owner"]["username"], "alice");
  assert_eq!(ids(&playlist["items"]), vec![a.clone()]);

  let items = format!("/api/playlists/{pid}/items");
  for id in [&b, &c, &b] {
    let added = alice
      .api(Method::POST, &items, json!({"sketch_id": id}))
      .await;
    assert_eq!(added.status, StatusCode::OK);
  }
  let playlist = alice
    .api(Method::GET, &format!("/api/playlists/{pid}"), Value::Null)
    .await
    .json();
  assert_eq!(
    ids(&playlist["items"]),
    vec![a.clone(), b.clone(), c.clone()]
  );

  let order = format!("/api/playlists/{pid}/order");
  let reordered = alice
    .api(Method::PUT, &order, json!({"sketch_ids": [c, a, b]}))
    .await
    .json();
  assert_eq!(
    ids(&reordered["items"]),
    vec![c.clone(), a.clone(), b.clone()]
  );
  // A partial order puts the rest after it.
  let partial = alice
    .api(Method::PUT, &order, json!({"sketch_ids": [b]}))
    .await
    .json();
  assert_eq!(
    ids(&partial["items"]),
    vec![b.clone(), c.clone(), a.clone()]
  );
  let unknown = alice
    .api(Method::PUT, &order, json!({"sketch_ids": ["zzzzzzzz"]}))
    .await;
  assert_eq!(unknown.status, StatusCode::BAD_REQUEST);
  let duplicate = alice
    .api(Method::PUT, &order, json!({"sketch_ids": [a, a]}))
    .await;
  assert_eq!(duplicate.status, StatusCode::BAD_REQUEST);

  let removed = alice
    .api(Method::DELETE, &format!("{items}/{c}"), Value::Null)
    .await;
  assert_eq!(removed.status, StatusCode::NO_CONTENT);
  let missing = alice
    .api(Method::DELETE, &format!("{items}/{c}"), Value::Null)
    .await;
  assert_eq!(missing.status, StatusCode::NOT_FOUND);

  let updated = alice
    .api(
      Method::PUT,
      &format!("/api/playlists/{pid}"),
      json!({"title": "Renamed", "visibility": "public"}),
    )
    .await
    .json();
  assert_eq!(updated["title"], "Renamed");
  assert_eq!(updated["description"], "Good stuff");
  assert_eq!(updated["visibility"], "public");

  let mine = alice
    .api(
      Method::GET,
      &format!("/api/me/playlists?sketch={a}"),
      Value::Null,
    )
    .await
    .json();
  assert_eq!(mine["playlists"][0]["contains"], true);
  assert_eq!(mine["playlists"][0]["item_count"], 2);
  let other = alice
    .api(
      Method::GET,
      &format!("/api/me/playlists?sketch={c}"),
      Value::Null,
    )
    .await
    .json();
  assert_eq!(other["playlists"][0]["contains"], false);

  let page = app.client().get(&format!("/p/{pid}")).await;
  assert_eq!(page.status, StatusCode::OK);
  let text = page.page();
  assert!(text.contains("Renamed"));
  assert!(text.contains(&format!("/s/{b}?playlist={pid}")));

  let deleted = alice
    .api(
      Method::DELETE,
      &format!("/api/playlists/{pid}"),
      Value::Null,
    )
    .await;
  assert_eq!(deleted.status, StatusCode::NO_CONTENT);
  assert_eq!(
    app.client().get(&format!("/p/{pid}")).await.status,
    StatusCode::NOT_FOUND
  );
}

#[tokio::test]
async fn playlist_permissions_and_visibility() {
  let app = TestApp::new().await;
  let mut alice = app.user("alice").await;
  let public_sketch = id_of(&alice.create_sketch("Shown", "public").await);
  let private_sketch = id_of(&alice.create_sketch("Secret", "private").await);
  let pid = id_of(
    &alice
      .api(
        Method::POST,
        "/api/playlists",
        json!({"title": "Alice's", "visibility": "unlisted"}),
      )
      .await
      .json(),
  );
  for id in [&public_sketch, &private_sketch] {
    alice
      .api(
        Method::POST,
        &format!("/api/playlists/{pid}/items"),
        json!({"sketch_id": id}),
      )
      .await;
  }

  let mut bob = app.user("bob").await;
  // Bob can view the unlisted playlist but only sees the public sketch.
  let viewed = bob
    .api(Method::GET, &format!("/api/playlists/{pid}"), Value::Null)
    .await
    .json();
  assert_eq!(ids(&viewed["items"]), vec![public_sketch.clone()]);
  assert_eq!(viewed["can_edit"], false);
  assert!(
    !bob
      .get(&format!("/p/{pid}"))
      .await
      .text()
      .contains("Secret")
  );
  // It isn't listed on Alice's profile for him.
  assert!(
    !bob
      .get("/u/alice?tab=playlists")
      .await
      .text()
      .contains("Alice")
  );
  assert!(
    alice
      .get("/u/alice?tab=playlists")
      .await
      .page()
      .contains("Alice's")
  );

  // He can't change it.
  let edits = [
    (
      Method::PUT,
      format!("/api/playlists/{pid}"),
      json!({"title": "Bob's"}),
    ),
    (
      Method::POST,
      format!("/api/playlists/{pid}/items"),
      json!({"sketch_id": public_sketch}),
    ),
    (
      Method::PUT,
      format!("/api/playlists/{pid}/order"),
      json!({"sketch_ids": []}),
    ),
    (Method::DELETE, format!("/api/playlists/{pid}"), Value::Null),
  ];
  for (method, path, body) in edits {
    let response = bob.api(method, &path, body).await;
    assert_eq!(response.status, StatusCode::FORBIDDEN, "{path}");
  }
  // Nor add Alice's private sketch to his own playlist.
  let his = id_of(
    &bob
      .api(Method::POST, "/api/playlists", json!({"title": "Bob's"}))
      .await
      .json(),
  );
  let sneaky = bob
    .api(
      Method::POST,
      &format!("/api/playlists/{his}/items"),
      json!({"sketch_id": private_sketch}),
    )
    .await;
  assert_eq!(sneaky.status, StatusCode::NOT_FOUND);

  // Private playlists are 404s to others.
  alice
    .api(
      Method::PUT,
      &format!("/api/playlists/{pid}"),
      json!({"visibility": "private"}),
    )
    .await;
  let hidden = bob
    .api(Method::GET, &format!("/api/playlists/{pid}"), Value::Null)
    .await;
  assert_eq!(hidden.status, StatusCode::NOT_FOUND);
  assert_eq!(
    bob.get(&format!("/p/{pid}")).await.status,
    StatusCode::NOT_FOUND
  );

  // Publishing needs a verified email.
  let mut carol = app.unverified_user("carol").await;
  let refused = carol
    .api(
      Method::POST,
      "/api/playlists",
      json!({"title": "x", "visibility": "public"}),
    )
    .await;
  assert_eq!(refused.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn playlist_navigation_on_sketch_pages() {
  let app = TestApp::new().await;
  let mut alice = app.user("alice").await;
  let a = id_of(&alice.create_sketch("A", "public").await);
  let hidden = id_of(&alice.create_sketch("Hidden", "private").await);
  let b = id_of(&alice.create_sketch("B", "public").await);
  let pid = id_of(
    &alice
      .api(
        Method::POST,
        "/api/playlists",
        json!({"title": "Tour", "visibility": "public", "sketch_id": a}),
      )
      .await
      .json(),
  );
  for id in [&hidden, &b] {
    alice
      .api(
        Method::POST,
        &format!("/api/playlists/{pid}/items"),
        json!({"sketch_id": id}),
      )
      .await;
  }
  let mut bob = app.user("bob").await;
  let data = bob.get(&format!("/s/{b}?playlist={pid}")).await.page_json();
  // Bob's view skips the private sketch.
  assert_eq!(
    data["playlist"],
    json!({"id": pid, "title": "Tour", "index": 1, "count": 2, "prev": a, "next": null})
  );
  let owner = alice
    .get(&format!("/s/{b}?playlist={pid}"))
    .await
    .page_json();
  assert_eq!(owner["playlist"]["index"], 2);
  assert_eq!(owner["playlist"]["prev"], hidden.as_str());

  // A playlist that doesn't hold the sketch, or doesn't exist, is ignored.
  let other = bob
    .get(&format!("/s/{b}?playlist=nope1234"))
    .await
    .page_json();
  assert!(other["playlist"].is_null());
}
