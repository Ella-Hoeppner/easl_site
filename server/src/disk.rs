//! The free-disk guard: the last backstop against the database filling its
//! disk.
//!
//! The history budgets bound what each account stores; this bounds what all
//! of them do together. Before storing sketch content or a thumbnail (a new
//! sketch, a fork, a save of files, a thumbnail upload), handlers call
//! [`DiskGuard::check`], which fails with [`AppError::OutOfStorage`] while
//! the database's filesystem has less free space than
//! [`Config::min_free_disk`]. The free space is measured with `statvfs` at
//! most every few seconds, so checking costs a lock and a comparison.
//!
//! [`Config::min_free_disk`]: crate::config::Config::min_free_disk

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::error::AppError;

/// How long a measurement of the free space is used before measuring again.
const MEASURE_EVERY: Duration = Duration::from_secs(5);

/// One measurement of the free space.
struct Reading {
  at: Instant,
  /// The bytes available, or `None` when measuring failed.
  available: Option<u64>,
}

/// Refuses writes while the database's filesystem is low on space.
pub struct DiskGuard {
  /// A directory on the filesystem to measure (the database's).
  dir: PathBuf,
  /// The least free space writes need, in bytes; 0 turns the guard off.
  min_free: u64,
  last: Mutex<Option<Reading>>,
}

impl DiskGuard {
  pub fn new(dir: PathBuf, min_free: u64) -> Self {
    DiskGuard {
      dir,
      min_free,
      last: Mutex::new(None),
    }
  }

  /// Fails with [`AppError::OutOfStorage`] while the filesystem has less
  /// free space than the guard needs. When the space can't be measured it
  /// lets writes through (and logs the error), since refusing every save
  /// over a measuring problem would be worse.
  pub fn check(&self) -> Result<(), AppError> {
    if self.min_free == 0 {
      return Ok(());
    }
    match self.available() {
      Some(available) if available < self.min_free => {
        Err(AppError::OutOfStorage)
      }
      _ => Ok(()),
    }
  }

  /// The free space, measured at most every [`MEASURE_EVERY`]. A new
  /// measurement below the threshold logs a warning, so a full disk is
  /// logged every few seconds while it lasts, not on every refused request.
  fn available(&self) -> Option<u64> {
    let now = Instant::now();
    let Ok(mut last) = self.last.lock() else {
      return None;
    };
    if let Some(reading) = &*last
      && now.duration_since(reading.at) < MEASURE_EVERY
    {
      return reading.available;
    }
    let available = match fs4::available_space(&self.dir) {
      Ok(available) => {
        if available < self.min_free {
          tracing::warn!(
            "refusing to store sketch content: {} has {available} bytes \
             free, under the {} the site keeps free",
            self.dir.display(),
            self.min_free
          );
        }
        Some(available)
      }
      Err(error) => {
        tracing::error!(
          "couldn't measure the free space at {}: {error}",
          self.dir.display()
        );
        None
      }
    };
    *last = Some(Reading { at: now, available });
    available
  }
}

#[cfg(test)]
mod tests {
  use std::env;

  use super::*;

  #[test]
  fn refuses_below_the_threshold() {
    let dir = env::temp_dir();
    assert!(DiskGuard::new(dir.clone(), 0).check().is_ok());
    assert!(DiskGuard::new(dir.clone(), 1).check().is_ok());
    assert!(matches!(
      DiskGuard::new(dir.clone(), u64::MAX).check(),
      Err(AppError::OutOfStorage)
    ));
    // A filesystem that can't be measured doesn't block writes.
    let missing = dir.join("no such directory for easl tests");
    assert!(DiskGuard::new(missing, u64::MAX).check().is_ok());
  }
}
