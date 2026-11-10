//! Sketch tags: setting them, forks, explore's tag filter and most-used
//! tags, and search matching them.

mod common;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};

use common::{Client, TestApp};

fn id_of(value: &Value) -> String {
  value["id"].as_str().expect("id").to_string()
}

fn tags_of(sketch: &Value) -> Vec<String> {
  sketch["tags"]
    .as_array()
    .expect("tags")
    .iter()
    .map(|tag| tag.as_str().expect("tag").to_string())
    .collect()
}

/// Sets a sketch's tags, expecting success, and returns the sketch.
async fn tag(client: &mut Client<'_>, id: &str, tags: &[&str]) -> Value {
  let response = client
    .api(
      Method::PUT,
      &format!("/api/sketches/{id}"),
      json!({ "tags": tags }),
    )
    .await;
  assert_eq!(response.status, StatusCode::OK, "{}", response.text());
  response.json()
}

/// Creates a public sketch with tags, returning its id.
async fn tagged(client: &mut Client<'_>, title: &str, tags: &[&str]) -> String {
  let id = id_of(&client.create_sketch(title, "public").await);
  tag(client, &id, tags).await;
  id
}

/// The sketch titles on a page, in order.
fn card_titles(html: &str) -> Vec<String> {
  html
    .split("class=\"card-title\"><a href=\"")
    .skip(1)
    .filter_map(|rest| {
      let start = rest.find('>')? + 1;
      let end = rest.find("</a>")?;
      Some(rest[start..end].to_string())
    })
    .collect()
}

/// A popular-tag chip: its tag (without the `#`), count, link, and whether
/// it's the current tag.
#[derive(Debug, PartialEq)]
struct Chip {
  tag: String,
  count: i64,
  url: String,
  current: bool,
}

/// The popular-tag chips on an explore page's
/// [`page`](common::TestResponse::page) text.
fn chips(html: &str) -> Vec<Chip> {
  html
    .split("<a class=\"tag-chip\" href=\"")
    .skip(1)
    .map(|rest| {
      let url_end = rest.find('"').expect("url end");
      let open_end = rest.find('>').expect("tag end");
      let text_start = open_end + 2;
      let text_end = rest.find(" <span").expect("count");
      let count_start = rest.find("tag-count\">").expect("count") + 11;
      let count_end = count_start + rest[count_start..].find('<').expect("<");
      Chip {
        tag: rest[text_start..text_end].to_string(),
        count: rest[count_start..count_end].parse().expect("count"),
        url: rest[..url_end].replace("&amp;", "&"),
        current: rest[..open_end].contains("aria-current"),
      }
    })
    .collect()
}

#[tokio::test]
async fn setting_tags() {
  let app = TestApp::new().await;
  let mut alice = app.user("alice").await;
  let created = alice
    .api(
      Method::POST,
      "/api/sketches",
      json!({
        "title": "Tagged at birth",
        "files": [{"path": "main.easl", "content": "(defn main [])"}],
        "tags": ["#Noise", "2D"],
      }),
    )
    .await;
  assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
  assert_eq!(tags_of(&created.json()), ["noise", "2d"]);

  let sketch = alice.create_sketch("Waves", "public").await;
  let id = id_of(&sketch);
  let path = format!("/api/sketches/{id}");
  assert_eq!(tags_of(&sketch), Vec::<String>::new());

  // Normalized, kept in the order given.
  let sketch =
    tag(&mut alice, &id, &["Zeta", " #alpha", "Ray  Marching"]).await;
  assert_eq!(tags_of(&sketch), ["zeta", "alpha", "ray-marching"]);
  let head = sketch["head_revision"].clone();
  // Tags aren't part of the program: no new revision, no draft, and
  // everyone sees them at once.
  assert_eq!(sketch["has_draft"], false);
  let mut bob = app.user("bob").await;
  let seen = bob.api(Method::GET, &path, Value::Null).await.json();
  assert_eq!(tags_of(&seen), ["zeta", "alpha", "ray-marching"]);

  // A PUT without tags keeps them; one with tags replaces them.
  let renamed = alice
    .api(Method::PUT, &path, json!({"title": "Waves 2"}))
    .await
    .json();
  assert_eq!(tags_of(&renamed), ["zeta", "alpha", "ray-marching"]);
  let replaced = tag(&mut alice, &id, &["ray-marching", "beta"]).await;
  assert_eq!(tags_of(&replaced), ["ray-marching", "beta"]);
  assert_eq!(replaced["head_revision"], head);

  // Only the owner sets them.
  let forbidden = bob.api(Method::PUT, &path, json!({"tags": ["mine"]})).await;
  assert_eq!(forbidden.status, StatusCode::FORBIDDEN);

  // Bad tags are refused, naming the tag and the rule, and change nothing.
  let invalid = [
    (json!(["fine", "c++"]), "\"c++\""),
    (json!(["-edge"]), "not starting or ending with '-'"),
    (json!([""]), "1 to 32"),
    (json!(["a".repeat(33)]), "1 to 32"),
    (json!(["Noise", "#noise"]), "\"noise\" appears twice"),
    (
      json!(["a", "b", "c", "d", "e", "f", "g", "h", "i"]),
      "at most 8 tags",
    ),
  ];
  for (tags, message) in invalid {
    let response = alice.api(Method::PUT, &path, json!({"tags": tags})).await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST, "{tags}");
    let error = response.json()["error"]
      .as_str()
      .expect("error")
      .to_string();
    assert!(error.contains(message), "{error}");
  }
  let unchanged = alice.api(Method::GET, &path, Value::Null).await.json();
  assert_eq!(tags_of(&unchanged), ["ray-marching", "beta"]);

  // The same tags again change nothing, not even `updated_at`.
  sqlx::query("UPDATE sketches SET updated_at = 5 WHERE id = ?")
    .bind(&id)
    .execute(&app.state.db)
    .await
    .expect("age");
  let same = tag(&mut alice, &id, &["Ray Marching", "beta"]).await;
  assert_eq!(same["updated_at"], 5);
  let different = tag(&mut alice, &id, &["beta", "ray-marching"]).await;
  assert_ne!(different["updated_at"], 5);
  assert_eq!(tags_of(&different), ["beta", "ray-marching"]);
  let cleared = tag(&mut alice, &id, &[]).await;
  assert!(tags_of(&cleared).is_empty());

  // Deleting the sketch deletes its tags.
  let other = tagged(&mut alice, "Doomed", &["gone"]).await;
  alice
    .api(
      Method::DELETE,
      &format!("/api/sketches/{other}"),
      Value::Null,
    )
    .await;
  let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sketch_tags")
    .fetch_one(&app.state.db)
    .await
    .expect("count");
  assert_eq!(left, 2, "only the first sketch's tags");
}

#[tokio::test]
async fn forks_copy_tags_and_summaries_show_them() {
  let app = TestApp::new().await;
  let mut alice = app.user("alice").await;
  let id = tagged(&mut alice, "Original", &["sdf", "3d"]).await;
  let mut bob = app.user("bob").await;
  let fork = bob
    .api(
      Method::POST,
      &format!("/api/sketches/{id}/fork"),
      Value::Null,
    )
    .await;
  assert_eq!(fork.status, StatusCode::CREATED);
  let fork = fork.json();
  assert_eq!(tags_of(&fork), ["sdf", "3d"]);
  // The fork's tags are its own.
  tag(&mut bob, &id_of(&fork), &["mine"]).await;
  let original = alice
    .api(Method::GET, &format!("/api/sketches/{id}"), Value::Null)
    .await
    .json();
  assert_eq!(tags_of(&original), ["sdf", "3d"]);

  // Summaries (a playlist's items) carry them too.
  let playlist = alice
    .api(
      Method::POST,
      "/api/playlists",
      json!({"title": "Mine", "visibility": "public"}),
    )
    .await
    .json();
  let playlist_path = format!("/api/playlists/{}", id_of(&playlist));
  alice
    .api(
      Method::POST,
      &format!("{playlist_path}/items"),
      json!({"sketch_id": id}),
    )
    .await;
  let listed = alice.api(Method::GET, &playlist_path, Value::Null).await;
  assert_eq!(listed.status, StatusCode::OK, "{}", listed.text());
  assert_eq!(tags_of(&listed.json()["items"][0]), ["sdf", "3d"]);
}

#[tokio::test]
async fn explore_by_tag() {
  let app = TestApp::new().await;
  let mut alice = app.user("alice").await;
  let old = tagged(&mut alice, "Old sphere", &["raymarching", "3d"]).await;
  let mid = tagged(&mut alice, "Middle tunnel", &["raymarching"]).await;
  tagged(&mut alice, "Newest noise", &["noise", "2d"]).await;
  for (id, published) in [(&old, 1000), (&mid, 2000)] {
    sqlx::query("UPDATE sketches SET published_at = ? WHERE id = ?")
      .bind(published)
      .bind(id)
      .execute(&app.state.db)
      .await
      .expect("update");
  }
  let mut bob = app.user("bob").await;
  bob
    .api(
      Method::POST,
      &format!("/api/sketches/{old}/like"),
      Value::Null,
    )
    .await;

  let mut client = app.client();
  let page = client.get("/?tag=raymarching").await;
  assert_eq!(page.status, StatusCode::OK);
  let html = page.page();
  assert_eq!(card_titles(&html), ["Middle tunnel", "Old sphere"]);
  assert!(html.contains("Tagged <span class=\"tag-name\">#raymarching</span>"));
  assert!(html.contains("<title>#raymarching — Explore — easl</title>"));
  // The way back keeps the sort.
  assert!(html.contains("<a href=\"/?sort=new\">Show all sketches</a>"));
  // Sorting, searching, and the search form keep the tag.
  assert!(html.contains("href=\"/?sort=popular&amp;tag=raymarching\""));
  assert!(html.contains("name=\"tag\" value=\"raymarching\""));
  assert_eq!(
    card_titles(&client.get("/?sort=popular&tag=raymarching").await.page()),
    ["Old sphere", "Middle tunnel"]
  );
  assert_eq!(
    card_titles(&client.get("/?tag=raymarching&q=tunnel").await.page()),
    ["Middle tunnel"]
  );
  let none = client.get("/?tag=raymarching&q=noise").await.page();
  assert!(card_titles(&none).is_empty());
  assert!(none.contains("No sketches tagged #raymarching match"));
  assert!(none.contains("href=\"/?sort=new&amp;tag=raymarching\""));
  // A tag in the address is read the way tags are saved.
  assert_eq!(
    card_titles(&client.get("/?tag=%23RayMarching").await.page()),
    ["Middle tunnel", "Old sphere"]
  );
  let missing = client.get("/?tag=nothing-here").await.page();
  assert!(missing.contains("No sketches are tagged #nothing-here."));
  // Cards link to their tags.
  assert!(html.contains("<a href=\"/?tag=raymarching\">#raymarching</a>"));

  // Pages keep the tag.
  for i in 0..25 {
    tagged(&mut alice, &format!("Bulk {i:02}"), &["bulk"]).await;
  }
  let first = client.get("/?tag=bulk").await.page();
  assert_eq!(card_titles(&first).len(), 24);
  assert!(first.contains("href=\"/?sort=new&amp;tag=bulk&amp;page=2\""));
  let second = client.get("/?sort=new&tag=bulk&page=2").await.page();
  assert_eq!(card_titles(&second).len(), 1);
  assert!(second.contains("href=\"/?sort=new&amp;tag=bulk\" rel=\"prev\""));
  let beyond = client.get("/?tag=bulk&page=3").await.page();
  assert!(beyond.contains("There’s nothing on this page."));
  assert!(
    beyond.contains("href=\"/?sort=new&amp;tag=bulk\">Back to the first")
  );
}

#[tokio::test]
async fn popular_tags_count_only_listed_sketches() {
  let app = TestApp::new().await;
  let mut alice = app.user("alice").await;
  tagged(&mut alice, "A", &["shared", "alpha"]).await;
  tagged(&mut alice, "B", &["shared", "beta"]).await;
  tagged(&mut alice, "C", &["shared", "beta"]).await;
  // None of these count: unlisted, private, hidden, or by a banned user.
  for visibility in ["unlisted", "private"] {
    let id = id_of(&alice.create_sketch(visibility, visibility).await);
    tag(&mut alice, &id, &["secret", "shared"]).await;
  }
  let hidden = tagged(&mut alice, "Hidden", &["secret", "shared"]).await;
  let mut admin = app.admin("dana").await;
  admin
    .api(
      Method::POST,
      &format!("/api/admin/sketches/{hidden}/hide"),
      Value::Null,
    )
    .await;
  let mut bob = app.user("bob").await;
  tagged(&mut bob, "Banned", &["secret", "shared"]).await;
  admin
    .api(Method::POST, "/api/admin/users/bob/ban", Value::Null)
    .await;

  let mut client = app.client();
  let html = client.get("/").await.page();
  let found: Vec<_> = chips(&html)
    .into_iter()
    .map(|chip| (chip.tag, chip.count))
    .collect();
  // Most used first, ties by name.
  assert_eq!(
    found,
    [
      ("shared".to_string(), 3),
      ("beta".to_string(), 2),
      ("alpha".to_string(), 1),
    ]
  );
  // The filter shows only what's listed, too, even to the owner.
  assert!(card_titles(&client.get("/?tag=secret").await.page()).is_empty());
  assert!(card_titles(&alice.get("/?tag=secret").await.page()).is_empty());
  assert_eq!(
    card_titles(&client.get("/?tag=shared").await.page()).len(),
    3
  );

  // Chips keep the sort and search; the current one is marked and clears
  // the tag.
  let html = client.get("/?sort=popular&q=b&tag=beta").await.page();
  let marked = chips(&html);
  let beta = marked.iter().find(|c| c.tag == "beta").expect("beta");
  assert!(beta.current);
  assert_eq!(beta.url, "/?sort=popular&q=b");
  let alpha = marked.iter().find(|c| c.tag == "alpha").expect("alpha");
  assert!(!alpha.current);
  assert_eq!(alpha.url, "/?sort=popular&q=b&tag=alpha");

  // At most 16, by count.
  for i in 0..17 {
    tagged(&mut alice, &format!("Many {i}"), &[&format!("t{i:02}")]).await;
  }
  let chips_now = chips(&client.get("/").await.page());
  assert_eq!(chips_now.len(), 16);
  assert_eq!(chips_now[0].tag, "shared");
  assert_eq!(chips_now[3].tag, "t00");
  assert_eq!(chips_now[15].tag, "t12");
}

#[tokio::test]
async fn search_matches_tags() {
  let app = TestApp::new().await;
  let mut alice = app.user("alice").await;
  tagged(&mut alice, "Sphere", &["ray-marching", "3d"]).await;
  tagged(&mut alice, "Plasma", &["2d"]).await;
  tagged(&mut alice, "Untagged", &[]).await;
  let mut client = app.client();
  let mut search = async |q: &str| card_titles(&client.get(q).await.page());
  // Substrings of tags match, and a query is compared as a tag.
  assert_eq!(search("/?q=marching").await, ["Sphere"]);
  assert_eq!(search("/?q=%23Ray%20Marching").await, ["Sphere"]);
  assert_eq!(search("/?q=ray_march").await, ["Sphere"]);
  assert_eq!(search("/?q=2D").await, ["Plasma"]);
  // A query that's nothing as a tag doesn't match every tagged sketch.
  assert!(search("/?q=%23").await.is_empty());
  // The other fields still match.
  assert_eq!(search("/?q=plas").await, ["Plasma"]);
}
