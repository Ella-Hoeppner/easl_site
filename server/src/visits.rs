//! Visit counts: one row per page view, on the docs host and the editor
//! host, holding the time, the site, the visitor's approximate country, and
//! a daily visitor hash, never the visitor's address.
//!
//! Every page loads a small script that sends `POST /visit` (a beacon), so
//! only browsers that run the page's scripts are counted, which leaves out
//! most crawlers. The handler looks up the address's country in a local
//! GeoIP database ([`Config::geoip_db`]) and hashes the address and user
//! agent with a random salt that lives only in memory and is replaced each
//! UTC day. Within a day the same browser gets the same hash, so unique
//! visitors can be counted; once the salt is gone the hashes can't be tied
//! back to an address, or to the same visitor on another day.
//!
//! [`Config::geoip_db`]: crate::config::Config::geoip_db

use std::fs;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::{Mutex, RwLock};
use std::time::SystemTime;

use axum::extract::State;
use axum::http::header::{ORIGIN, USER_AGENT};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{MethodRouter, post};
use maxminddb::{Reader, geoip2};
use rand::Rng;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

use crate::config::Config;
use crate::db;
use crate::ratelimit::Action;
use crate::security::ClientIp;
use crate::state::AppState;

/// The path pages send their beacon to, on both hosts.
pub const VISIT_PATH: &str = "/visit";

/// One day, in seconds.
const DAY: i64 = 24 * 60 * 60;

/// Words in a user agent that mark a crawler or a headless browser.
const BOT_MARKERS: &[&str] = &["bot", "crawl", "spider", "slurp", "headless"];

/// Which of the two sites a page view was on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Site {
  Docs,
  Editor,
}

impl Site {
  pub fn as_str(self) -> &'static str {
    match self {
      Site::Docs => "docs",
      Site::Editor => "editor",
    }
  }
}

/// The salt for one UTC day.
struct Salt {
  day: i64,
  bytes: [u8; 32],
}

/// The GeoIP database, and the modification time of the file it was read
/// from.
struct Loaded {
  modified: SystemTime,
  reader: Reader<Vec<u8>>,
}

/// Visit counting's state: today's salt and the GeoIP database.
pub struct Visits {
  salt: Mutex<Option<Salt>>,
  geoip_path: Option<PathBuf>,
  geoip: RwLock<Option<Loaded>>,
}

impl Visits {
  /// Visit counting with the GeoIP database at `geoip_path`, if any. A
  /// missing or unreadable file is logged, and visits are recorded without
  /// a country until [`Visits::reload_geoip`] finds a good one.
  pub fn new(geoip_path: Option<PathBuf>) -> Self {
    let visits = Visits {
      salt: Mutex::new(None),
      geoip_path,
      geoip: RwLock::new(None),
    };
    visits.reload_geoip();
    visits
  }

  /// Reads the GeoIP database again if its file changed since it was last
  /// read (or appeared), keeping the one in memory when the new file can't
  /// be read. The server calls this hourly, so a monthly update of the file
  /// needs no restart.
  pub fn reload_geoip(&self) {
    let Some(path) = &self.geoip_path else {
      return;
    };
    let modified = match fs::metadata(path).and_then(|m| m.modified()) {
      Ok(modified) => modified,
      Err(error) => {
        tracing::warn!(
          "visits are recorded without countries: can't read the GeoIP \
           database {}: {error}",
          path.display()
        );
        return;
      }
    };
    let current = self
      .geoip
      .read()
      .ok()
      .and_then(|loaded| loaded.as_ref().map(|loaded| loaded.modified));
    if current == Some(modified) {
      return;
    }
    match Reader::open_readfile(path) {
      Ok(reader) => {
        tracing::info!("read the GeoIP database {}", path.display());
        if let Ok(mut loaded) = self.geoip.write() {
          *loaded = Some(Loaded { modified, reader });
        }
      }
      Err(error) => tracing::error!(
        "reading the GeoIP database {}: {error}",
        path.display()
      ),
    }
  }

  /// Drops the salt once its day is over, so it's gone even when no visit
  /// comes along to replace it. The server calls this hourly.
  pub fn forget_old_salt(&self) {
    let today = db::now().div_euclid(DAY);
    if let Ok(mut salt) = self.salt.lock()
      && salt.as_ref().is_some_and(|salt| salt.day != today)
    {
      *salt = None;
    }
  }

  /// The visitor hash for an address and user agent at time `at`: the
  /// first 8 bytes of a SHA-256 of that day's salt, the address, and the
  /// user agent, in hex.
  pub fn visitor(&self, at: i64, ip: &str, user_agent: &str) -> String {
    let day = at.div_euclid(DAY);
    let salt = {
      let Ok(mut salt) = self.salt.lock() else {
        return String::new();
      };
      match salt.as_ref() {
        Some(salt) if salt.day == day => salt.bytes,
        _ => {
          let fresh = Salt {
            day,
            bytes: rand::rng().random(),
          };
          let bytes = fresh.bytes;
          *salt = Some(fresh);
          bytes
        }
      }
    };
    let mut hash = Sha256::new();
    hash.update(salt);
    hash.update(ip.as_bytes());
    hash.update([0]);
    hash.update(user_agent.as_bytes());
    hash.finalize()[..8]
      .iter()
      .map(|b| format!("{b:02x}"))
      .collect()
  }

  /// The ISO 3166-1 code of the country an address is in, when the GeoIP
  /// database knows it.
  pub fn country(&self, ip: &str) -> Option<String> {
    let ip: IpAddr = ip.parse().ok()?;
    let loaded = self.geoip.read().ok()?;
    let result = loaded.as_ref()?.reader.lookup(ip).ok()?;
    let record: geoip2::Country = result.decode().ok()??;
    record.country.iso_code.map(str::to_string)
  }
}

/// Whether a user agent is missing or names a crawler or headless browser.
pub fn is_bot(user_agent: &str) -> bool {
  let user_agent = user_agent.to_ascii_lowercase();
  user_agent.is_empty()
    || BOT_MARKERS.iter().any(|marker| user_agent.contains(marker))
}

/// Whether a beacon's `Origin` is one of the site's own pages: the docs
/// origin or an origin the app accepts.
fn is_own_page(config: &Config, origin: &str) -> bool {
  origin == config.docs_origin || config.is_allowed_origin(origin)
}

/// `POST /visit` on `site`'s host.
pub fn route(site: Site) -> MethodRouter<AppState> {
  post(
    move |state: State<AppState>, ip: ClientIp, headers: HeaderMap| {
      record_visit(state, ip, headers, site)
    },
  )
}

/// Records a page view and answers 204, or answers 204 having recorded
/// nothing: for a request from another site's page, a crawler, an address
/// past its limit, or while the disk is low on space. Counting is best
/// effort, so nothing about it is ever an error for the page.
async fn record_visit(
  State(state): State<AppState>,
  ClientIp(ip): ClientIp,
  headers: HeaderMap,
  site: Site,
) -> StatusCode {
  let header = |name| {
    headers
      .get(name)
      .and_then(|value| value.to_str().ok())
      .unwrap_or("")
  };
  let user_agent = header(USER_AGENT);
  let counted = is_own_page(&state.config, header(ORIGIN))
    && !is_bot(user_agent)
    && state
      .limiter
      .allow(Action::Visit, &ip, state.config.limits.visits)
    && state.disk.check().is_ok();
  if !counted {
    return StatusCode::NO_CONTENT;
  }
  let at = db::now();
  let visitor = state.visits.visitor(at, &ip, user_agent);
  let country = state.visits.country(&ip);
  if let Err(error) =
    insert(&state.db, at, site, country.as_deref(), &visitor).await
  {
    tracing::error!("recording a visit: {error}");
  }
  StatusCode::NO_CONTENT
}

/// Stores one page view.
pub async fn insert(
  db: &SqlitePool,
  at: i64,
  site: Site,
  country: Option<&str>,
  visitor: &str,
) -> sqlx::Result<()> {
  sqlx::query(
    "INSERT INTO visits (at, site, country, visitor) VALUES (?, ?, ?, ?)",
  )
  .bind(at)
  .bind(site.as_str())
  .bind(country)
  .bind(visitor)
  .execute(db)
  .await?;
  Ok(())
}

/// One UTC day's page views and unique visitors.
#[derive(Debug, sqlx::FromRow)]
pub struct DayCount {
  /// The day's start, in unix seconds.
  pub day: i64,
  pub visits: i64,
  pub visitors: i64,
  pub docs_visits: i64,
  pub editor_visits: i64,
}

/// One country's page views and visitors over a span of days, where a
/// visitor is counted once a day.
#[derive(Debug, sqlx::FromRow)]
pub struct CountryCount {
  /// The ISO 3166-1 code, or `None` for addresses with no known country.
  pub country: Option<String>,
  pub visits: i64,
  pub visitors: i64,
}

/// Each day's counts since `since` (unix seconds), oldest first. Days with
/// no visits are left out.
pub async fn by_day(
  db: &SqlitePool,
  since: i64,
) -> sqlx::Result<Vec<DayCount>> {
  sqlx::query_as(
    "SELECT at / ?1 * ?1 AS day, COUNT(*) AS visits,
       COUNT(DISTINCT visitor) AS visitors,
       COUNT(*) FILTER (WHERE site = 'docs') AS docs_visits,
       COUNT(*) FILTER (WHERE site = 'editor') AS editor_visits
     FROM visits WHERE at >= ?2 GROUP BY day ORDER BY day",
  )
  .bind(DAY)
  .bind(since)
  .fetch_all(db)
  .await
}

/// Each country's counts since `since` (unix seconds), most visits first.
/// Hashes differ from day to day, so counting distinct ones counts each
/// visitor once a day.
pub async fn by_country(
  db: &SqlitePool,
  since: i64,
) -> sqlx::Result<Vec<CountryCount>> {
  sqlx::query_as(
    "SELECT country, COUNT(*) AS visits, COUNT(DISTINCT visitor) AS visitors
     FROM visits WHERE at >= ? GROUP BY country
     ORDER BY visits DESC, country",
  )
  .bind(since)
  .fetch_all(db)
  .await
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn visitor_hashes_last_a_day() {
    let visits = Visits::new(None);
    let ua = "Mozilla/5.0";
    let morning = 20_000 * DAY + 60;
    let evening = morning + DAY - 120;
    let a = visits.visitor(morning, "1.2.3.4", ua);
    assert_eq!(a.len(), 16);
    assert_eq!(visits.visitor(evening, "1.2.3.4", ua), a);
    assert_ne!(visits.visitor(evening, "1.2.3.5", ua), a);
    assert_ne!(visits.visitor(evening, "1.2.3.4", "Other/1.0"), a);
    // The next day has a new salt, so the same visitor hashes differently.
    assert_ne!(visits.visitor(morning + DAY, "1.2.3.4", ua), a);
  }

  #[test]
  fn old_salts_are_forgotten() {
    let visits = Visits::new(None);
    visits.visitor(db::now() - 2 * DAY, "1.2.3.4", "x");
    visits.forget_old_salt();
    assert!(visits.salt.lock().unwrap().is_none());
    visits.visitor(db::now(), "1.2.3.4", "x");
    visits.forget_old_salt();
    assert!(visits.salt.lock().unwrap().is_some());
  }

  #[test]
  fn bots() {
    for agent in [
      "",
      "Mozilla/5.0 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)",
      "Mozilla/5.0 HeadlessChrome/120.0",
      "Baiduspider",
    ] {
      assert!(is_bot(agent), "{agent}");
    }
    assert!(!is_bot(
      "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 \
       (KHTML, like Gecko) Version/18.0 Safari/605.1.15"
    ));
  }

  #[test]
  fn no_geoip_database_means_no_country() {
    let visits = Visits::new(Some(PathBuf::from("/nonexistent.mmdb")));
    assert_eq!(visits.country("8.8.8.8"), None);
    assert_eq!(Visits::new(None).country("not an address"), None);
  }
}
