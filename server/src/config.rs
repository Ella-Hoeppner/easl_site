//! Runtime configuration for the server.

use std::path::PathBuf;
use std::time::Duration;

/// How the server runs: where its files are, its public origin, and the
/// development and proxy switches.
#[derive(Clone, Debug)]
pub struct Config {
  /// The generated static site, served at `/`.
  pub site_dir: PathBuf,
  /// The public origin (`https://easl-lang.org`), without a trailing slash.
  pub origin: String,
  /// Development mode: dev origins are accepted, cookies aren't `Secure`,
  /// templates and assets reload from disk, and email links are shown on
  /// the page.
  pub dev: bool,
  /// Whether to take the client address from `X-Forwarded-For`'s last hop
  /// (set when running behind a reverse proxy that appends it).
  pub trust_proxy: bool,
  /// Rate limits for abuse-prone actions.
  pub limits: RateLimits,
}

impl Config {
  /// A configuration with default limits; `origin` is normalized to have no
  /// trailing slash.
  pub fn new(site_dir: PathBuf, origin: &str, dev: bool) -> Self {
    Config {
      site_dir,
      origin: origin.trim_end_matches('/').to_string(),
      dev,
      trust_proxy: false,
      limits: RateLimits::default(),
    }
  }

  /// Whether a request's `Origin` header names this site: the configured
  /// origin, or in dev any `http://localhost:<port>` or
  /// `http://127.0.0.1:<port>`.
  pub fn is_allowed_origin(&self, origin: &str) -> bool {
    if origin == self.origin {
      return true;
    }
    self.dev
      && ["http://localhost:", "http://127.0.0.1:"]
        .iter()
        .any(|prefix| {
          origin.strip_prefix(prefix).is_some_and(|port| {
            !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit())
          })
        })
  }

  /// Whether cookies carry the `Secure` attribute.
  pub fn secure_cookies(&self) -> bool {
    !self.dev
  }

  /// An absolute URL for a site path (which starts with `/`).
  pub fn absolute_url(&self, path: &str) -> String {
    format!("{}{}", self.origin, path)
  }
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

/// The rate limits the server enforces.
#[derive(Clone, Copy, Debug)]
pub struct RateLimits {
  /// Sign-in, sign-up, and password-reset requests, per client address.
  pub auth: Limit,
  /// Comments posted, per user.
  pub comments: Limit,
  /// Sketches created (including forks), per user.
  pub sketch_creates: Limit,
  /// Reports filed, per user.
  pub reports: Limit,
  /// Verification emails re-sent, per user.
  pub verification_emails: Limit,
}

impl Default for RateLimits {
  fn default() -> Self {
    RateLimits {
      auth: Limit::new(10, 10 * 60),
      comments: Limit::new(10, 60),
      sketch_creates: Limit::new(30, 60 * 60),
      reports: Limit::new(10, 60 * 60),
      verification_emails: Limit::new(5, 60 * 60),
    }
  }
}
