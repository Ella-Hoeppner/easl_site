//! `easl-site-server seed`: demo data for development.
//!
//! Creates the users `alice`, `bob`, and `carol` (password `password123`,
//! emails verified), a public sketch for each playground example (from the
//! site's `playground/examples.json`, or two built-in programs without
//! one), and some comments, likes, and a playlist. Does nothing when the
//! database already has users.

use std::path::{Path, PathBuf};

use anyhow::Context;
use sqlx::SqlitePool;

use crate::auth::hash_password_blocking;
use crate::comments;
use crate::db;
use crate::playlists;
use crate::site::{Example, SiteFiles};
use crate::sketches::{self, NewSketch};
use crate::users;
use crate::validate::SketchFile;
use crate::visibility::Visibility;

const PASSWORD: &str = "password123";

const USERS: &[(&str, &str, &str)] = &[
  (
    "alice",
    "Alice Liddell",
    "Raymarching enthusiast. Mostly SDFs and soft shadows.\n\
     More at https://example.com/alice",
  ),
  (
    "bob",
    "Bob",
    "I make compute shaders do things they shouldn't.",
  ),
  ("carol", "", ""),
];

const GRADIENT: &str = "(defn vert []: vec4f
  (match (vertex-index)
    0u (vec4f -1. -1. 0. 1.)
    1u (vec4f -1. 3. 0. 1.)
    _ (vec4f 3. -1. 0. 1.)))

(defn frag []: vec4f
  (vec4f (/ (.xy (position))
            (vec2f (window-resolution)))
         0.
         1.))

@cpu
(defn main []
  (spawn-window (fn []
                  (dispatch-render-shaders vert frag 3))))
";

const PULSE: &str = "(defn vert []: vec4f
  (match (vertex-index)
    0u (vec4f -1. -1. 0. 1.)
    1u (vec4f -1. 3. 0. 1.)
    _ (vec4f 3. -1. 0. 1.)))

(defn frag []: vec4f
  (let [uv (/ (.xy (position))
              (vec2f (window-resolution)))
        t (window-time)]
    (vec4f (* 0.5 (+ 1. (sin (+ t (* 6. uv.x)))))
           uv.y
           (* 0.5 (+ 1. (cos t)))
           1.)))

@cpu
(defn main []
  (spawn-window (fn []
                  (dispatch-render-shaders vert frag 3))))
";

const COMMENTS: &[&str] = &[
  "This is lovely! How did you get the colors so smooth?",
  "Forked it and tried a different palette, works great.",
  "The way it moves is hypnotic.",
  "Nice. See https://iquilezles.org/articles/ for more tricks like this.",
];

fn built_in_examples() -> Vec<Example> {
  vec![
    Example {
      id: "gradient".into(),
      menu: None,
      title: "Gradient".into(),
      source: GRADIENT.into(),
    },
    Example {
      id: "pulse".into(),
      menu: None,
      title: "Pulse".into(),
      source: PULSE.into(),
    },
  ]
}

/// Fills an empty database with demo data.
pub async fn seed(db: &SqlitePool, site: &SiteFiles) -> anyhow::Result<()> {
  if users::count(db).await? > 0 {
    println!("The database already has users; not seeding.");
    return Ok(());
  }
  let hash = hash_password_blocking(PASSWORD)?;
  let mut user_ids = Vec::new();
  for (username, display_name, bio) in USERS {
    let email = format!("{username}@example.com");
    let user = users::create(db, username, &email, &hash)
      .await
      .map_err(|e| e.into_anyhow().context(format!("creating {username}")))?;
    users::update_profile(db, user.id, display_name, bio).await?;
    users::mark_verified(db, user.id).await?;
    user_ids.push(user.id);
  }

  let mut examples = site.examples();
  if examples.is_empty() {
    examples = built_in_examples();
  }
  let mut sketch_ids = Vec::new();
  for (i, example) in examples.iter().enumerate() {
    let title = match &example.menu {
      Some(menu) => format!("{menu}: {}", example.title),
      None => example.title.clone(),
    };
    let sketch = NewSketch {
      title: title.chars().take(100).collect(),
      description: format!(
        "The \"{}\" example from the playground.",
        example.title
      ),
      files: vec![SketchFile {
        path: "main.easl".into(),
        content: example.source.clone(),
      }],
      entry_file: Some("main.easl".into()),
      entry_name: None,
      visibility: Visibility::Public,
      forked_from: None,
    };
    let owner = user_ids[i % user_ids.len()];
    let id = sketches::create(db, owner, &sketch)
      .await
      .map_err(|e| e.into_anyhow().context("creating a sketch"))?;
    sketch_ids.push(id);
  }

  for (i, sketch_id) in sketch_ids.iter().enumerate() {
    for (j, user_id) in user_ids.iter().enumerate() {
      if (i + j) % 3 != 0 {
        sketches::set_liked(db, *user_id, sketch_id, true).await?;
      }
    }
    if i % 2 == 0 {
      let author = user_ids[(i + 1) % user_ids.len()];
      let body = COMMENTS[i / 2 % COMMENTS.len()];
      comments::create(db, sketch_id, author, body).await?;
    }
  }

  let playlist = playlists::create(
    db,
    user_ids[0],
    "Favorites",
    "A few sketches worth watching.",
    Visibility::Public,
  )
  .await
  .map_err(|e| e.into_anyhow().context("creating a playlist"))?;
  for sketch_id in sketch_ids.iter().rev().take(5) {
    playlists::add_item(db, &playlist, sketch_id)
      .await
      .map_err(|e| e.into_anyhow().context("filling the playlist"))?;
  }
  println!(
    "Seeded {} users and {} sketches. Sign in as alice, bob, or carol with \
     the password {PASSWORD}.",
    user_ids.len(),
    sketch_ids.len()
  );
  Ok(())
}

/// Creates the demo data in the database at `db_path`.
pub async fn run(db_path: &Path, site_dir: PathBuf) -> anyhow::Result<()> {
  let db = db::open(db_path).await.context("opening the database")?;
  seed(&db, &SiteFiles::new(site_dir)).await
}
