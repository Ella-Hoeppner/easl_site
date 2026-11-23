//! In-memory sliding-window rate limiting.
//!
//! Each (action, key) pair keeps the times of its recent events; an event is
//! allowed when, for each of the action's limits, fewer than the limit's
//! count fall inside its window (so an action can have an hourly and a daily
//! limit). State lives in this process only, which suits a single-server
//! deployment.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::config::Limit;
use crate::error::AppError;

/// What is being limited; separates the counters of different actions that
/// share a key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
  /// Signing in, or submitting a password-reset form, by client address.
  Auth,
  /// Signing up, by client address.
  Signup,
  /// Asking for a password-reset email, by client address.
  ResetRequest,
  /// A password-reset email, by recipient account.
  ResetEmail,
  /// A verification email after sign-up, by account.
  VerificationEmail,
  /// Checking the current password in settings, by account.
  PasswordCheck,
  Comment,
  SketchCreate,
  PlaylistCreate,
  Report,
  /// A sketch view, counted once per viewer and sketch per window.
  View,
}

/// How many checks pass between sweeps of stale entries.
const SWEEP_INTERVAL: u64 = 1024;

/// The recent events of one (action, key) pair, and the window they're
/// counted over.
struct Window {
  events: VecDeque<Instant>,
  length: Duration,
}

#[derive(Default)]
struct State {
  windows: HashMap<(Action, String), Window>,
  checks: u64,
}

/// The rate limiter shared by all requests.
#[derive(Default)]
pub struct RateLimiter {
  state: Mutex<State>,
}

impl RateLimiter {
  pub fn new() -> Self {
    Self::default()
  }

  /// Drops every entry with no event inside its window, so a key (often a
  /// client's address) is kept only while a limit needs it. The server calls
  /// this hourly; checks also sweep every [`SWEEP_INTERVAL`] calls.
  pub fn forget_expired(&self) {
    if let Ok(mut state) = self.state.lock() {
      sweep(&mut state.windows, Instant::now());
    }
  }

  /// Records an event for `(action, key)` if `limit` allows it, returning
  /// whether it did.
  pub fn allow(&self, action: Action, key: &str, limit: Limit) -> bool {
    self.allow_all(action, key, &[limit])
  }

  /// Records an event for `(action, key)` if every one of `limits` allows
  /// it, returning whether it did. An event refused by one limit counts
  /// against none.
  pub fn allow_all(&self, action: Action, key: &str, limits: &[Limit]) -> bool {
    let now = Instant::now();
    let Ok(mut state) = self.state.lock() else {
      // A poisoned lock means a panic elsewhere; failing open keeps the
      // site usable.
      return true;
    };
    state.checks += 1;
    if state.checks.is_multiple_of(SWEEP_INTERVAL) {
      sweep(&mut state.windows, now);
    }
    let longest = limits
      .iter()
      .map(|limit| limit.window)
      .max()
      .unwrap_or_default();
    let window = state
      .windows
      .entry((action, key.to_string()))
      .or_insert_with(|| Window {
        events: VecDeque::new(),
        length: longest,
      });
    window.length = longest;
    let events = &mut window.events;
    while events
      .front()
      .is_some_and(|&t| now.duration_since(t) >= longest)
    {
      events.pop_front();
    }
    let within = |length: Duration| {
      events
        .iter()
        .rev()
        .take_while(|&&t| now.duration_since(t) < length)
        .count()
    };
    if limits
      .iter()
      .any(|limit| within(limit.window) >= limit.count)
    {
      return false;
    }
    events.push_back(now);
    true
  }

  /// Like [`RateLimiter::allow`], failing with [`AppError::RateLimited`].
  pub fn check(
    &self,
    action: Action,
    key: &str,
    limit: Limit,
  ) -> Result<(), AppError> {
    self.check_all(action, key, &[limit])
  }

  /// Like [`RateLimiter::allow_all`], failing with
  /// [`AppError::RateLimited`].
  pub fn check_all(
    &self,
    action: Action,
    key: &str,
    limits: &[Limit],
  ) -> Result<(), AppError> {
    if self.allow_all(action, key, limits) {
      Ok(())
    } else {
      Err(AppError::RateLimited)
    }
  }
}

/// Drops entries with no event inside their window.
fn sweep(windows: &mut HashMap<(Action, String), Window>, now: Instant) {
  windows.retain(|_, window| {
    window
      .events
      .back()
      .is_some_and(|&t| now.duration_since(t) < window.length)
  });
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn forgetting_expired_entries() {
    let limiter = RateLimiter::new();
    let expired = Limit::new(1, 0);
    let current = Limit::new(1, 60 * 60);
    assert!(limiter.allow(Action::View, "gone", expired));
    assert!(limiter.allow(Action::View, "kept", current));
    limiter.forget_expired();
    let state = limiter.state.lock().unwrap();
    assert!(
      !state
        .windows
        .contains_key(&(Action::View, "gone".to_string()))
    );
    assert!(
      state
        .windows
        .contains_key(&(Action::View, "kept".to_string()))
    );
  }

  #[test]
  fn limits_per_action_and_key() {
    let limiter = RateLimiter::new();
    let limit = Limit::new(2, 60);
    assert!(limiter.allow(Action::Auth, "a", limit));
    assert!(limiter.allow(Action::Auth, "a", limit));
    assert!(!limiter.allow(Action::Auth, "a", limit));
    assert!(limiter.allow(Action::Auth, "b", limit));
    assert!(limiter.allow(Action::Comment, "a", limit));
  }

  #[test]
  fn every_limit_applies() {
    let limiter = RateLimiter::new();
    let limits = [Limit::new(2, 0), Limit::new(3, 60)];
    // The first limit's window is empty, so only the second one binds.
    for _ in 0..3 {
      assert!(limiter.allow_all(Action::ResetEmail, "a", &limits));
    }
    assert!(!limiter.allow_all(Action::ResetEmail, "a", &limits));
    let limits = [Limit::new(1, 60), Limit::new(5, 3600)];
    assert!(limiter.allow_all(Action::SketchCreate, "a", &limits));
    assert!(!limiter.allow_all(Action::SketchCreate, "a", &limits));
    // A refused event counts against neither limit.
    let limits = [Limit::new(5, 60), Limit::new(1, 3600)];
    assert!(limiter.allow_all(Action::Signup, "a", &limits));
    for _ in 0..3 {
      assert!(!limiter.allow_all(Action::Signup, "a", &limits));
    }
    assert!(limiter.allow_all(Action::Signup, "a", &[Limit::new(2, 60)]));
  }

  #[test]
  fn windows_expire() {
    let limiter = RateLimiter::new();
    let limit = Limit::new(1, 0);
    assert!(limiter.allow(Action::View, "a", limit));
    assert!(limiter.allow(Action::View, "a", limit));
  }
}
