//! Runtime configuration for the server.

use std::path::PathBuf;
use std::time::Duration;

/// How the server runs: where its files are, its two public origins, and
/// the development and proxy switches.
///
/// The app (the editor, explore, profiles, accounts, the API) lives on the
/// editor origin; the docs (the static site) on the docs origin. Requests
/// are routed by their `Host`: the editor origin's hostname gets the app,
/// any other host the docs.
#[derive(Clone, Debug)]
pub struct Config {
  /// The generated static site: the docs, and the editor's and web
  /// runtime's files.
  pub site_dir: PathBuf,
  /// The app's public origin (`https://editor.easl-lang.org`), without a
  /// trailing slash.
  pub editor_origin: String,
  /// The docs' public origin (`https://easl-lang.org`), without a trailing
  /// slash.
  pub docs_origin: String,
  /// Development mode: dev origins are accepted, cookies aren't `Secure`,
  /// templates and assets reload from disk, and email links are shown on
  /// the page.
  pub dev: bool,
  /// Whether to take the client address from `X-Forwarded-For`'s last hop
  /// (set when running behind a reverse proxy that appends it).
  pub trust_proxy: bool,
  /// Rate limits for abuse-prone actions.
  pub limits: RateLimits,
  /// How much revision history sketches and accounts may keep.
  pub history: HistoryBudget,
  /// How many emails the site sends a day (UTC), at most.
  pub daily_email_budget: i64,
  /// The directory holding the database, whose filesystem the free-disk
  /// guard ([`crate::disk`]) measures.
  pub data_dir: PathBuf,
  /// The least free space, in bytes, the database's filesystem must have
  /// for the site to store sketch content or thumbnails (0 turns the guard
  /// off).
  pub min_free_disk: u64,
  /// The GeoIP database (DB-IP's IP to Country Lite, in MaxMind's format)
  /// that visits' countries are looked up in; without it, visits are
  /// recorded with no country. See [`crate::visits`].
  pub geoip_db: Option<PathBuf>,
}

impl Config {
  /// A configuration with default limits; the origins are normalized to
  /// have no trailing slash.
  pub fn new(
    site_dir: PathBuf,
    editor_origin: &str,
    docs_origin: &str,
    dev: bool,
  ) -> Self {
    Config {
      site_dir,
      editor_origin: editor_origin.trim_end_matches('/').to_string(),
      docs_origin: docs_origin.trim_end_matches('/').to_string(),
      dev,
      trust_proxy: false,
      limits: RateLimits::default(),
      history: HistoryBudget::default(),
      daily_email_budget: DAILY_EMAIL_BUDGET,
      data_dir: PathBuf::from("."),
      min_free_disk: MIN_FREE_DISK_BYTES,
      geoip_db: None,
    }
  }

  /// Whether a request's `Host` (with or without a port, in any case) is
  /// the editor's: its hostname is the editor origin's.
  pub fn is_editor_host(&self, host: &str) -> bool {
    hostname(host)
      .eq_ignore_ascii_case(hostname(origin_host(&self.editor_origin)))
  }

  /// Whether a request's `Origin` header names the app: the editor origin,
  /// or in dev any `http://localhost:<port>`, `http://127.0.0.1:<port>`, or
  /// `http://editor.localhost:<port>`.
  pub fn is_allowed_origin(&self, origin: &str) -> bool {
    if origin == self.editor_origin {
      return true;
    }
    self.dev
      && DEV_ORIGIN_PREFIXES.iter().any(|prefix| {
        origin.strip_prefix(prefix).is_some_and(|port| {
          !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit())
        })
      })
  }

  /// Whether cookies carry the `Secure` attribute.
  pub fn secure_cookies(&self) -> bool {
    !self.dev
  }

  /// An absolute URL for an app path (which starts with `/`), on the
  /// editor origin.
  pub fn editor_url(&self, path: &str) -> String {
    format!("{}{}", self.editor_origin, path)
  }

  /// An absolute URL for a docs path (which starts with `/`), on the docs
  /// origin.
  pub fn docs_url(&self, path: &str) -> String {
    format!("{}{}", self.docs_origin, path)
  }
}

/// The request origins dev mode also accepts, each followed by a port.
const DEV_ORIGIN_PREFIXES: &[&str] = &[
  "http://localhost:",
  "http://127.0.0.1:",
  "http://editor.localhost:",
];

/// The host (and port) of an origin: `https://a.b:8000` → `a.b:8000`.
fn origin_host(origin: &str) -> &str {
  let rest = origin.split_once("://").map_or(origin, |(_, rest)| rest);
  rest.split('/').next().unwrap_or(rest)
}

/// A host without its port: `a.b:8000` → `a.b`, `[::1]:80` → `[::1]`.
fn hostname(host: &str) -> &str {
  if host.starts_with('[') {
    return host
      .split_once(']')
      .map_or(host, |(name, _)| &host[..name.len() + 1]);
  }
  host.split(':').next().unwrap_or(host)
}

/// A number of events allowed within a sliding window.
#[derive(Clone, Copy, Debug)]
pub struct Limit {
  pub count: usize,
  pub window: Duration,
}

impl Limit {
  pub const fn new(count: usize, window_secs: u64) -> Self {
    Limit {
      count,
      window: Duration::from_secs(window_secs),
    }
  }
}

/// One hour, in seconds.
const HOUR: u64 = 60 * 60;

/// One day, in seconds.
const DAY: u64 = 24 * HOUR;

/// The rate limits the server enforces. Each is a separate bucket, so
/// hitting one never blocks another action.
#[derive(Clone, Copy, Debug)]
pub struct RateLimits {
  /// Sign-in attempts and password-reset form submissions, per client
  /// address.
  pub auth: Limit,
  /// Sign-ups (each sends a verification email), per client address.
  pub signups: Limit,
  /// Password-reset requests (`/forgot`), per client address.
  pub reset_requests: Limit,
  /// Password-reset emails, per recipient account, per hour and per day.
  pub reset_emails: [Limit; 2],
  /// Verification emails sent for an account after its sign-up (re-sent,
  /// or for a changed address), per account, per hour and per day.
  pub verification_emails: [Limit; 2],
  /// Checks of the current password in settings (changing the email or
  /// password, deleting the account), per account.
  pub password_checks: Limit,
  /// Comments posted, per user.
  pub comments: Limit,
  /// Sketches created (including forks), per user, per hour and per day.
  pub sketch_creates: [Limit; 2],
  /// Playlists created, per user.
  pub playlist_creates: Limit,
  /// Reports filed, per user.
  pub reports: Limit,
  /// Page views recorded, per client address; past it, views go
  /// uncounted.
  pub visits: Limit,
}

impl Default for RateLimits {
  fn default() -> Self {
    RateLimits {
      auth: Limit::new(10, 10 * 60),
      signups: Limit::new(5, HOUR),
      reset_requests: Limit::new(10, HOUR),
      reset_emails: [Limit::new(3, HOUR), Limit::new(6, DAY)],
      verification_emails: [Limit::new(3, HOUR), Limit::new(10, DAY)],
      password_checks: Limit::new(10, 10 * 60),
      comments: Limit::new(10, 60),
      sketch_creates: [Limit::new(30, HOUR), Limit::new(100, DAY)],
      playlist_creates: Limit::new(20, HOUR),
      reports: Limit::new(10, HOUR),
      visits: Limit::new(300, HOUR),
    }
  }
}

/// The site-wide number of emails sent a day, at most: below the 100 a day
/// of Resend's free tier, leaving room for test emails.
pub const DAILY_EMAIL_BUDGET: i64 = 80;

/// The most one sketch's revision history keeps, in bytes (as
/// [`crate::revisions::size`] measures revisions): a save that takes it
/// past this prunes the oldest revisions.
pub const SKETCH_HISTORY_BYTES: i64 = 10 * 1024 * 1024;

/// The most all of an account's sketches' histories may hold together, in
/// bytes, while its email address is unverified: a save or new sketch that
/// would take it past this is refused. Accounts are cheap to make, so an
/// unverified one gets little.
pub const UNVERIFIED_ACCOUNT_HISTORY_BYTES: i64 = 5 * 1024 * 1024;

/// The same for an account with a verified email address. Verifying takes
/// an email, and the site sends at most [`DAILY_EMAIL_BUDGET`] a day, so
/// verified accounts can't be made in bulk.
pub const VERIFIED_ACCOUNT_HISTORY_BYTES: i64 = 50 * 1024 * 1024;

/// How much revision history is kept; see [`crate::revisions`].
#[derive(Clone, Copy, Debug)]
pub struct HistoryBudget {
  /// The most one sketch's history keeps, in bytes.
  pub per_sketch: i64,
  /// The most an unverified account's sketches' histories hold together,
  /// in bytes.
  pub per_unverified_account: i64,
  /// The most a verified account's sketches' histories hold together, in
  /// bytes.
  pub per_verified_account: i64,
}

impl HistoryBudget {
  /// The most an account's sketches' histories hold together, by whether
  /// its email address is verified.
  pub fn per_account(&self, verified: bool) -> i64 {
    if verified {
      self.per_verified_account
    } else {
      self.per_unverified_account
    }
  }
}

impl Default for HistoryBudget {
  fn default() -> Self {
    HistoryBudget {
      per_sketch: SKETCH_HISTORY_BYTES,
      per_unverified_account: UNVERIFIED_ACCOUNT_HISTORY_BYTES,
      per_verified_account: VERIFIED_ACCOUNT_HISTORY_BYTES,
    }
  }
}

/// The least free space the database's filesystem must have for the site
/// to store sketch content or thumbnails, in bytes.
pub const MIN_FREE_DISK_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Parses a size in bytes: a whole number with an optional binary unit
/// (`K`, `M`, `G`, or `T`, optionally followed by `B` or `iB`, in any
/// case), so `2G`, `2GB`, `2GiB`, and `2147483648` are the same.
pub fn parse_size(text: &str) -> Result<u64, String> {
  let text = text.trim();
  let digits = text
    .find(|c: char| !c.is_ascii_digit())
    .unwrap_or(text.len());
  let (number, unit) = text.split_at(digits);
  let number: u64 = number
    .parse()
    .map_err(|_| format!("{text:?} isn't a size like 2G or 500M"))?;
  let unit = unit.trim().to_ascii_lowercase();
  let unit = unit
    .strip_suffix("ib")
    .or_else(|| unit.strip_suffix('b'))
    .unwrap_or(&unit);
  let power = match unit {
    "" => 0,
    "k" => 1,
    "m" => 2,
    "g" => 3,
    "t" => 4,
    _ => return Err(format!("{text:?} has an unknown unit")),
  };
  number
    .checked_mul(1024u64.pow(power))
    .ok_or_else(|| format!("{text:?} is too large"))
}

#[cfg(test)]
mod tests {
  use super::*;

  fn config(dev: bool) -> Config {
    Config::new(
      PathBuf::from("site"),
      "https://editor.easl-lang.org/",
      "https://easl-lang.org",
      dev,
    )
  }

  #[test]
  fn editor_host_matches_on_hostname() {
    let config = config(false);
    assert_eq!(config.editor_origin, "https://editor.easl-lang.org");
    assert!(config.is_editor_host("editor.easl-lang.org"));
    assert!(config.is_editor_host("EDITOR.easl-lang.org:443"));
    assert!(!config.is_editor_host("easl-lang.org"));
    assert!(!config.is_editor_host("editor.easl-lang.org.evil.example"));
    assert!(!config.is_editor_host(""));
    let local =
      Config::new(PathBuf::new(), "http://[::1]:8000", "http://x", false);
    assert!(local.is_editor_host("[::1]:9000"));
    assert!(!local.is_editor_host("[::2]:8000"));
  }

  #[test]
  fn dev_origins_only_in_dev() {
    for origin in [
      "http://localhost:8100",
      "http://127.0.0.1:3000",
      "http://editor.localhost:8150",
    ] {
      assert!(config(true).is_allowed_origin(origin), "{origin}");
      assert!(!config(false).is_allowed_origin(origin), "{origin}");
    }
    assert!(config(false).is_allowed_origin("https://editor.easl-lang.org"));
    assert!(!config(false).is_allowed_origin("https://easl-lang.org"));
    assert!(!config(true).is_allowed_origin("http://localhost:"));
    assert!(!config(true).is_allowed_origin("http://localhost.evil:80"));
  }

  #[test]
  fn sizes() {
    assert_eq!(parse_size("2G"), Ok(MIN_FREE_DISK_BYTES));
    assert_eq!(parse_size("2GiB"), Ok(MIN_FREE_DISK_BYTES));
    assert_eq!(parse_size("2 gb"), Ok(MIN_FREE_DISK_BYTES));
    assert_eq!(parse_size("500M"), Ok(500 * 1024 * 1024));
    assert_eq!(parse_size("10k"), Ok(10 * 1024));
    assert_eq!(parse_size("123"), Ok(123));
    assert_eq!(parse_size("0"), Ok(0));
    assert!(parse_size("").is_err());
    assert!(parse_size("G").is_err());
    assert!(parse_size("2X").is_err());
    assert!(parse_size("-1").is_err());
    assert!(parse_size("99999999T").is_err());
  }

  #[test]
  fn absolute_urls() {
    let config = config(false);
    assert_eq!(
      config.editor_url("/s/x"),
      "https://editor.easl-lang.org/s/x"
    );
    assert_eq!(
      config.docs_url("/guide.html"),
      "https://easl-lang.org/guide.html"
    );
  }
}
