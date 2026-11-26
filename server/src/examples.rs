//! The docs' example programs as sketches.
//!
//! The editor build lists the examples in `editor/examples.json`, each with
//! the fixed sketch id the docs link to. [`sync`] makes each one an
//! ordinary public sketch owned by one user (Ella's account) at that id,
//! creating it, or bringing over what the docs changed since the last sync
//! (`example_syncs` remembers what they said then): a changed title or
//! description replaces the sketch's, and changed files are saved as a new
//! revision and published at once. What the docs didn't change stays as
//! edited on the site. It never deletes anything and never touches likes,
//! comments, thumbnails, tags, or visibility.

use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use anyhow::{Context, anyhow, bail};
use serde::Deserialize;
use sqlx::SqlitePool;

use crate::auth::Viewer;
use crate::config::HistoryBudget;
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
  /// An existing sketch that was never synced: its title, description, and
  /// files stay as they are, and what the docs say now is remembered as
  /// what they said at the last sync.
  Adopt,
  Unchanged,
}

/// What the docs said about an example at its last sync (a row of
/// `example_syncs`).
#[derive(sqlx::FromRow)]
struct Synced {
  title: String,
  description: String,
  files: String,
}

/// Makes every example a public sketch owned by `owner` at its
/// `sketch_id`, creating missing ones. For an existing one, whatever of its
/// title, description, and files the docs changed since the last sync
/// replaces the sketch's (bumping `updated_at` only then); the rest stays as
/// edited on the site. Changed files become the sketch's head and, unless
/// the sketch was made private, its published revision, replacing any
/// draft. A sketch the docs never synced (made before syncs were
/// remembered) is adopted as it is.
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
    changes.push((example, change));
  }
  let mut report = SyncReport::default();
  let budget = HistoryBudget::default();
  for (example, change) in changes {
    let id = example.sketch_id.as_str();
    match change {
      Change::Create(sketch) => {
        let inserted = sketches::insert(db, id, owner.id, &sketch, &budget)
          .await
          .map_err(AppError::into_anyhow)?;
        if !inserted {
          bail!("the sketch id {id:?} was taken while syncing");
        }
        remember(db, example).await?;
        report.created += 1;
      }
      Change::Update(update) => {
        sketches::update(db, id, &update, &budget)
          .await
          .map_err(AppError::into_anyhow)?;
        remember(db, example).await?;
        report.updated += 1;
      }
      Change::Adopt => {
        remember(db, example).await?;
        report.unchanged += 1;
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
  let synced: Option<Synced> = sqlx::query_as(
    "SELECT title, description, files FROM example_syncs WHERE sketch_id = ?",
  )
  .bind(&example.sketch_id)
  .fetch_optional(db)
  .await?;
  let Some(synced) = synced else {
    return Ok(Change::Adopt);
  };
  let title_changed = title != synced.title;
  let description_changed = description != synced.description;
  let files_changed = files_json(&example.files)? != synced.files;
  if !title_changed && !description_changed && !files_changed {
    return Ok(Change::Unchanged);
  }
  let contents = if files_changed {
    let head = row.head_contents(db).await?;
    // The entry file stays as it is while it's still one of the files.
    let entry_file = match &head.entry_file {
      Some(path) if example.files.iter().any(|f| &f.path == path) => {
        path.clone()
      }
      _ => first_file,
    };
    Some(Contents {
      files: example.files.clone(),
      entry_file: Some(entry_file),
      entry_name: head.entry_name.clone(),
    })
  } else {
    None
  };
  Ok(Change::Update(SketchUpdate {
    title: if title_changed { title } else { row.title },
    description: if description_changed {
      description
    } else {
      row.description
    },
    contents,
    visibility: row.visibility,
    publish: files_changed,
    tags: None,
  }))
}

/// An example's files as `example_syncs` keeps them.
fn files_json(files: &[SketchFile]) -> anyhow::Result<String> {
  Ok(serde_json::to_string(files)?)
}

/// Records what the docs say about `example` now, as what they said at its
/// last sync.
async fn remember(db: &SqlitePool, example: &Example) -> anyhow::Result<()> {
  let invalid = AppError::into_anyhow;
  let title = validate::sketch_title(&example.title).map_err(invalid)?;
  let description =
    validate::sketch_description(&example.description).map_err(invalid)?;
  sqlx::query(
    "INSERT INTO example_syncs (sketch_id, title, description, files) \
     VALUES (?, ?, ?, ?) ON CONFLICT (sketch_id) DO UPDATE SET \
     title = excluded.title, description = excluded.description, \
     files = excluded.files",
  )
  .bind(&example.sketch_id)
  .bind(title)
  .bind(description)
  .bind(files_json(&example.files)?)
  .execute(db)
  .await?;
  Ok(())
}
