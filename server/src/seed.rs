//! `easl-site-server seed`: demo data for development.
//!
//! Creates the users `ella` (an admin, who owns the examples), `alice`,
//! `bob`, and `carol`, all with the password `password123` and verified
//! emails; syncs the site's examples (`editor/examples.json`, or two
//! built-in programs without one) to `ella`, tagging the ones without
//! tags; and gives the others sketches of their own (forks of the examples
//! and short programs, tagged), likes, comments, and public playlists.
//!
//! Users that already exist are left alone, the sync is idempotent, tags
//! are only given to examples without any, and the social data is only made
//! while `alice` has no public sketches, so seeding again changes nothing.

use std::path::Path;

use anyhow::Context;
use sqlx::SqlitePool;

use crate::auth::hash_password_blocking;
use crate::comments;
use crate::config::HistoryBudget;
use crate::db;
use crate::examples::{self, Example};
use crate::playlists;
use crate::revisions::Contents;
use crate::sketches::{self, NewSketch};
use crate::tags;
use crate::users;
use crate::validate::SketchFile;
use crate::visibility::Visibility;

const PASSWORD: &str = "password123";

/// The examples' owner.
const OWNER: &str = "ella";

/// A demo user: username, display name, bio, and whether they're an admin.
type DemoUser = (&'static str, &'static str, &'static str, bool);

/// The demo users, the examples' owner first.
const USERS: &[DemoUser] = &[
  (
    OWNER,
    "Ella",
    "I make easl. The docs' examples live here.",
    true,
  ),
  (
    "alice",
    "Alice Liddell",
    "Raymarching enthusiast. Mostly SDFs and soft shadows.\n\
     More at https://example.com/alice",
    false,
  ),
  (
    "bob",
    "Bob",
    "I make compute shaders do things they shouldn't.",
    false,
  ),
  ("carol", "", "", false),
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

fn main_file(source: &str) -> Vec<SketchFile> {
  vec![SketchFile {
    path: "main.easl".into(),
    content: source.into(),
  }]
}

/// The examples used when the site has none.
fn built_in_examples() -> Vec<Example> {
  vec![
    Example {
      id: "gradient".into(),
      sketch_id: "SeedGrad".into(),
      title: "Gradient".into(),
      description: "Each pixel colored by where it is on the screen.".into(),
      files: main_file(GRADIENT),
    },
    Example {
      id: "pulse".into(),
      sketch_id: "SeedPuls".into(),
      title: "Pulse".into(),
      description: "Colors that move with the time.".into(),
      files: main_file(PULSE),
    },
  ]
}

/// The id of a demo user, creating them if they don't exist.
async fn ensure_user(
  db: &SqlitePool,
  (username, display_name, bio, admin): DemoUser,
  hash: &str,
) -> anyhow::Result<i64> {
  if let Some(user) = users::by_username(db, username).await? {
    return Ok(user.id);
  }
  let email = format!("{username}@example.com");
  let user = users::create(db, username, &email, hash)
    .await
    .map_err(|e| e.into_anyhow().context(format!("creating {username}")))?;
  users::update_profile(db, user.id, display_name, bio).await?;
  users::mark_verified(db, user.id).await?;
  if admin {
    users::set_admin(db, username, true).await?;
  }
  Ok(user.id)
}

/// The tags of an example, by its id (its old playground name).
fn example_tags(id: &str) -> &'static [&'static str] {
  match id {
    "landing" => &["generative", "2d", "animation"],
    "game-of-life" => &["simulation", "compute", "cellular-automata"],
    "wavetable" => &["audio", "synth"],
    "raymarch-sphere" => &["raymarching", "walkthrough", "3d", "sdf"],
    "raymarch-cpu-raymarch" => &["raymarching", "walkthrough", "3d", "cpu"],
    "raymarch-rotating-union" => {
      &["raymarching", "walkthrough", "3d", "sdf", "animation"]
    }
    "raymarch-materials" => &["raymarching", "walkthrough", "3d", "lighting"],
    "gradient" => &["2d", "color", "beginner"],
    "pulse" => &["2d", "color", "animation"],
    _ => &[],
  }
}

fn strings(tags: &[&str]) -> Vec<String> {
  tags.iter().map(|tag| tag.to_string()).collect()
}

/// Tags the examples that have no tags yet.
async fn tag_examples(
  db: &SqlitePool,
  examples: &[Example],
) -> anyhow::Result<()> {
  let mut conn = db.acquire().await?;
  for example in examples {
    let tags = example_tags(&example.id);
    if !tags.is_empty()
      && tags::of(&mut conn, &example.sketch_id).await?.is_empty()
    {
      tags::set(&mut conn, &example.sketch_id, &strings(tags)).await?;
    }
  }
  Ok(())
}

/// A public sketch with one file.
fn public_sketch(
  title: &str,
  description: &str,
  files: Vec<SketchFile>,
  forked_from: Option<&str>,
  tags: &[&str],
) -> NewSketch {
  NewSketch {
    title: title.chars().take(100).collect(),
    description: description.into(),
    contents: Contents {
      entry_file: files.first().map(|f| f.path.clone()),
      files,
      entry_name: None,
    },
    visibility: Visibility::Public,
    forked_from: forked_from.map(str::to_string),
    tags: strings(tags),
  }
}

async fn create_sketch(
  db: &SqlitePool,
  owner: i64,
  sketch: &NewSketch,
) -> anyhow::Result<String> {
  sketches::create(db, owner, sketch, &HistoryBudget::default())
    .await
    .map_err(|e| e.into_anyhow().context("creating a sketch"))
}

/// Gives each non-owner demo user a fork of an example (with its tags, as
/// forks have) and a program of their own, returning the new sketches'
/// ids.
async fn others_sketches(
  db: &SqlitePool,
  others: &[i64],
  examples: &[Example],
) -> anyhow::Result<Vec<String>> {
  let originals: [(&str, &str, &str, &[&str]); 3] = [
    (
      "Screen-space gradient",
      "My first easl sketch.",
      GRADIENT,
      &["2d", "gradient", "beginner"],
    ),
    (
      "Pulse",
      "Sines all the way down.",
      PULSE,
      &["2d", "animation", "trigonometry"],
    ),
    ("Slow gradient", "", GRADIENT, &["gradient", "minimal"]),
  ];
  let mut ids = Vec::new();
  for (i, owner) in others.iter().enumerate() {
    let example = &examples[i % examples.len()];
    let fork = public_sketch(
      &format!("My take on {}", example.title),
      "Forked from the example; next I'll change the colors.",
      example.files.clone(),
      Some(&example.sketch_id),
      example_tags(&example.id),
    );
    ids.push(create_sketch(db, *owner, &fork).await?);
    let (title, description, source, tags) = originals[i % originals.len()];
    let original =
      public_sketch(title, description, main_file(source), None, tags);
    ids.push(create_sketch(db, *owner, &original).await?);
  }
  Ok(ids)
}

/// Likes, comments, and two public playlists among the demo sketches.
async fn social(
  db: &SqlitePool,
  others: &[i64],
  example_ids: &[String],
  other_ids: &[String],
) -> anyhow::Result<()> {
  let all: Vec<&String> = example_ids.iter().chain(other_ids).collect();
  for (i, sketch_id) in all.iter().enumerate() {
    for (j, user_id) in others.iter().enumerate() {
      if (i + j) % 3 != 0 {
        sketches::set_liked(db, *user_id, sketch_id, true).await?;
      }
    }
    if i % 2 == 0 {
      let author = others[i / 2 % others.len()];
      let body = COMMENTS[i / 2 % COMMENTS.len()];
      comments::create(db, sketch_id, author, body).await?;
    }
  }
  let lists: [(i64, &str, &str, Vec<&String>); 2] = [
    (
      others[0],
      "Favorites",
      "A few sketches worth watching.",
      example_ids
        .iter()
        .rev()
        .take(4)
        .chain(&other_ids[2..3])
        .collect(),
    ),
    (
      others[1],
      "Things that move",
      "Sketches with a sense of time.",
      example_ids.iter().take(3).chain(&other_ids[4..]).collect(),
    ),
  ];
  for (owner, title, description, items) in lists {
    let playlist =
      playlists::create(db, owner, title, description, Visibility::Public)
        .await
        .map_err(|e| e.into_anyhow().context("creating a playlist"))?;
    for sketch_id in items {
      playlists::add_item(db, &playlist, sketch_id)
        .await
        .map_err(|e| e.into_anyhow().context("filling a playlist"))?;
    }
  }
  Ok(())
}

/// Adds the demo data that isn't there yet.
pub async fn seed(db: &SqlitePool, site_dir: &Path) -> anyhow::Result<()> {
  let hash = hash_password_blocking(PASSWORD)?;
  let mut user_ids = Vec::new();
  for user in USERS {
    user_ids.push(ensure_user(db, *user, &hash).await?);
  }
  let examples = match examples::load(site_dir)? {
    Some(examples) if !examples.is_empty() => examples,
    _ => {
      println!("The site has no editor/examples.json; using two built-ins.");
      built_in_examples()
    }
  };
  let report = examples::sync(db, OWNER, &examples).await?;
  println!("Examples: {report}.");
  tag_examples(db, &examples).await?;

  let others = &user_ids[1..];
  let (alice_sketches, _) = sketches::listed_counts(db, others[0]).await?;
  if alice_sketches > 0 {
    println!("The other demo users already have sketches; leaving them.");
  } else {
    let example_ids: Vec<String> =
      examples.iter().map(|e| e.sketch_id.clone()).collect();
    let other_ids = others_sketches(db, others, &examples).await?;
    social(db, others, &example_ids, &other_ids).await?;
    println!(
      "Made {} sketches for alice, bob, and carol.",
      other_ids.len()
    );
  }
  println!(
    "Sign in as {} with the password {PASSWORD} ({OWNER} is an admin).",
    USERS
      .iter()
      .map(|(name, ..)| *name)
      .collect::<Vec<_>>()
      .join(", ")
  );
  Ok(())
}

/// Creates the demo data in the database at `db_path`.
pub async fn run(db_path: &Path, site_dir: &Path) -> anyhow::Result<()> {
  let db = db::open(db_path).await.context("opening the database")?;
  seed(&db, site_dir).await
}
