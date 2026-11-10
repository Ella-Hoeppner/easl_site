//! The docs' example programs as sketches.
//!
//! The editor build lists the examples in `editor/examples.json`, each with
//! the fixed sketch id the docs link to. [`sync`] makes each one an
//! ordinary public sketch owned by one user (Ella's account) at that id,
//! creating it or bringing its title, description, and files up to date.
//! New files are saved as a new revision and published at once. It never
//! deletes anything and never touches likes, comments, thumbnails, tags,
//! or visibility.

use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use anyhow::{Context, anyhow, bail};
use serde::Deserialize;
use sqlx::SqlitePool;

use crate::auth::Viewer;
use crate::db::is_public_id;
use crate::error::AppError;
use crate::revisions::Contents;
use crate::site;
use crate::sketches::{self, NewSketch, SketchUpdate};
use crate::users;
use crate::validate::{self, SketchFile};
use crate::visibility::Visibility;

/// One example, as `editor/examples.json` lists it.
#[derive(Clone, Debug, Deserialize)]
pub struct Example {
  /// The example's name, which old `/playground?example=<id>` links use.
  pub id: String,
  /// The id of the sketch holding it (8 base62 characters).
  pub sketch_id: String,
  pub title: String,
  #[serde(default)]
  pub description: String,
  pub files: Vec<SketchFile>,
}

#[derive(Deserialize)]
struct ExamplesFile {
  examples: Vec<Example>,
}

/// The site's examples, or `None` when it has no `editor/examples.json`.
pub fn load(site_dir: &Path) -> anyhow::Result<Option<Vec<Example>>> {
  let path = site::examples_path(site_dir);
  let text = match fs::read_to_string(&path) {
    Ok(text) => text,
    Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
    Err(error) => {
      return Err(error).with_context(|| format!("reading {}", path.display()));
    }
  };
  let file: ExamplesFile = serde_json::from_str(&text)
    .with_context(|| format!("parsing {}", path.display()))?;
  Ok(Some(file.examples))
}

/// How many example sketches a sync created, updated, and left alone.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SyncReport {
  pub created: usize,
  pub updated: usize,
  pub unchanged: usize,
}

impl fmt::Display for SyncReport {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(
      f,
      "{} created, {} updated, {} unchanged",
      self.created, self.updated, self.unchanged
    )
  }
}

/// What a sync does to one example's sketch.
enum Change {
  Create(NewSketch),
  Update(SketchUpdate),
  Unchanged,
}

/// Makes every example a public sketch owned by `owner` at its
/// `sketch_id`, creating missing ones and updating ones whose title,
/// description, or files differ (bumping `updated_at` only then). An
/// example's files become its sketch's head and, unless the sketch was made
/// private, its published revision, replacing any draft.
///
/// Everything is checked before anything is written: it fails when the
/// owner doesn't exist, when an example is invalid (a bad sketch id, title,
/// or files) or shares its sketch id with another, or when a sketch id
/// belongs to someone else's sketch.
pub async fn sync(
  db: &SqlitePool,
  owner: &str,
  examples: &[Example],
) -> anyhow::Result<SyncReport> {
  let owner = users::by_username(db, owner)
    .await?
    .ok_or_else(|| anyhow!("there's no user named {owner:?}"))?;
  let mut seen = HashSet::new();
  let mut changes = Vec::new();
  for example in examples {
    if !seen.insert(example.sketch_id.as_str()) {
      bail!(
        "two examples have the sketch id {:?}; each needs its own",
        example.sketch_id
      );
    }
    let change = plan(db, owner.id, &owner.username, example)
      .await
      .with_context(|| format!("the example {:?}", example.id))?;
    changes.push((example.sketch_id.as_str(), change));
  }
  let mut report = SyncReport::default();
  for (id, change) in changes {
    match change {
      Change::Create(sketch) => {
        sketches::insert(db, id, owner.id, &sketch).await?;
        report.created += 1;
      }
      Change::Update(update) => {
        sketches::update(db, id, &update).await?;
        report.updated += 1;
      }
      Change::Unchanged => report.unchanged += 1,
    }
  }
  Ok(report)
}

/// Validates an example and works out what its sketch needs.
async fn plan(
  db: &SqlitePool,
  owner_id: i64,
  owner_name: &str,
  example: &Example,
) -> anyhow::Result<Change> {
  if !is_public_id(&example.sketch_id) {
    bail!(
      "the sketch id {:?} isn't 8 letters and digits",
      example.sketch_id
    );
  }
  let invalid = AppError::into_anyhow;
  let title = validate::sketch_title(&example.title).map_err(invalid)?;
  let description =
    validate::sketch_description(&example.description).map_err(invalid)?;
  validate::sketch_files(&example.files).map_err(invalid)?;
  let first_file = example.files[0].path.clone();
  let anyone = Viewer::of(None);
  let Some(row) = sketches::find(db, &example.sketch_id, &anyone).await? else {
    return Ok(Change::Create(NewSketch {
      title,
      description,
      contents: Contents {
        files: example.files.clone(),
        entry_file: Some(first_file),
        entry_name: None,
      },
      visibility: Visibility::Public,
      forked_from: None,
      tags: Vec::new(),
    }));
  };
  if row.owner_id != owner_id {
    bail!(
      "the sketch {} belongs to {}, not {owner_name}",
      row.id,
      row.owner_username
    );
  }
  let head = row.head_contents(db).await?;
  // The entry file stays as it is while it's still one of the files.
  let entry_file = match &head.entry_file {
    Some(path) if example.files.iter().any(|f| &f.path == path) => path.clone(),
    _ => first_file,
  };
  let contents = Contents {
    files: example.files.clone(),
    entry_file: Some(entry_file),
    entry_name: head.entry_name.clone(),
  };
  let published = row.visibility == Visibility::Private
    || row.published_revision == Some(row.head_revision);
  let changed = contents != head
    || !published
    || title != row.title
    || description != row.description;
  if !changed {
    return Ok(Change::Unchanged);
  }
  Ok(Change::Update(SketchUpdate {
    title,
    description,
    contents: Some(contents),
    visibility: row.visibility,
    publish: true,
    tags: None,
  }))
}
