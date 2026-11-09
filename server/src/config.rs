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
