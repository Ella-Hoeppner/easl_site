//! The SQLite connection pool and small shared database helpers.

use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use rand::Rng;
use sqlx::SqlitePool;
use sqlx::sqlite::{
  SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous,
};

/// Opens (creating if needed) the database at `path` in WAL mode with
/// foreign keys enforced, and applies any pending migrations.
pub async fn open(path: &Path) -> anyhow::Result<SqlitePool> {
  if let Some(parent) = path.parent()
    && !parent.as_os_str().is_empty()
  {
    fs::create_dir_all(parent).with_context(|| {
      format!("creating the database directory {}", parent.display())
    })?;
  }
  let options = SqliteConnectOptions::new()
    .filename(path)
    .create_if_missing(true)
    .journal_mode(SqliteJournalMode::Wal)
    .synchronous(SqliteSynchronous::Normal)
    .foreign_keys(true)
    .busy_timeout(Duration::from_secs(5));
  let pool = SqlitePoolOptions::new()
    .max_connections(4)
    .connect_with(options)
    .await
    .with_context(|| format!("opening the database {}", path.display()))?;
  sqlx::migrate!("./migrations")
    .run(&pool)
    .await
    .context("migrating the database")?;
  Ok(pool)
}

/// The current time in unix seconds.
pub fn now() -> i64 {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .map(|d| d.as_secs() as i64)
    .unwrap_or(0)
}

const BASE62: &[u8] =
  b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// A random 8-character base62 id, for sketches and playlists.
pub fn new_public_id() -> String {
  let mut rng = rand::rng();
  (0..8)
    .map(|_| BASE62[rng.random_range(0..BASE62.len())] as char)
    .collect()
}

/// Whether `id` has the shape of a public id (8 base62 characters).
pub fn is_public_id(id: &str) -> bool {
  id.len() == 8 && id.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// Whether an error is a violation of a UNIQUE or PRIMARY KEY constraint.
pub fn is_unique_violation(error: &sqlx::Error) -> bool {
  matches!(error, sqlx::Error::Database(e) if e.is_unique_violation())
}

/// How many times inserting a row with a fresh random id is retried when
/// the id collides with an existing one.
pub const ID_ATTEMPTS: usize = 5;
