//! The state shared by every request.

use std::ops::Deref;
use std::sync::Arc;

use sqlx::SqlitePool;
use tokio::sync::Semaphore;

use crate::assets::Assets;
use crate::auth::hash_password_blocking;
use crate::config::Config;
use crate::disk::DiskGuard;
use crate::mail::Mailer;
use crate::ratelimit::RateLimiter;
use crate::site::SiteFiles;
use crate::templates::Templates;
use crate::tokens::new_token;
use crate::visits::Visits;

/// How many passwords may be hashed at once.
const CONCURRENT_HASHES: usize = 2;

/// A cheap handle to the shared state.
#[derive(Clone)]
pub struct AppState(Arc<Shared>);

/// The shared state.
pub struct Shared {
  pub db: SqlitePool,
  pub config: Config,
  pub templates: Templates,
  pub assets: Arc<Assets>,
  pub site: SiteFiles,
  pub mailer: Mailer,
  pub limiter: RateLimiter,
  /// Refuses to store sketch content while the disk is low on space.
  pub disk: DiskGuard,
  /// Bounds concurrent password hashing (each takes ~19 MB of memory).
  pub hashing: Semaphore,
  /// Today's visitor-hash salt and the GeoIP database.
  pub visits: Visits,
  /// A hash of a random password, checked when a sign-in names no
  /// account.
  pub dummy_password_hash: String,
}

impl AppState {
  pub fn new(
    config: Config,
    db: SqlitePool,
    mailer: Mailer,
  ) -> anyhow::Result<Self> {
    let assets = Arc::new(Assets::new(config.dev));
    let templates = Templates::new(config.dev, assets.clone())?;
    let site = SiteFiles::new(config.site_dir.clone());
    Ok(AppState(Arc::new(Shared {
      db,
      templates,
      assets,
      site,
      mailer,
      limiter: RateLimiter::new(),
      disk: DiskGuard::new(config.data_dir.clone(), config.min_free_disk),
      hashing: Semaphore::new(CONCURRENT_HASHES),
      visits: Visits::new(config.geoip_db.clone()),
      dummy_password_hash: hash_password_blocking(&new_token())?,
      config,
    })))
  }
}

impl Deref for AppState {
  type Target = Shared;

  fn deref(&self) -> &Shared {
    &self.0
  }
}
