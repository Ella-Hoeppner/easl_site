//! In-memory sliding-window rate limiting.
//!
//! Each (action, key) pair keeps the times of its recent events; an event is
//! allowed when fewer than the limit's count fall inside its window. State
//! lives in this process only, which suits a single-server deployment.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::config::Limit;
use crate::error::AppError;

/// What is being limited; separates the counters of different actions that
/// share a key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
  Auth,
  Comment,
  SketchCreate,
  Report,
  VerificationEmail,
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

  /// Records an event for `(action, key)` if `limit` allows it, returning
  /// whether it did.
  pub fn allow(&self, action: Action, key: &str, limit: Limit) -> bool {
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
    let window = state
      .windows
      .entry((action, key.to_string()))
      .or_insert_with(|| Window {
        events: VecDeque::new(),
        length: limit.window,
      });
    window.length = limit.window;
    let events = &mut window.events;
    while events
      .front()
      .is_some_and(|&t| now.duration_since(t) >= limit.window)
    {
      events.pop_front();
    }
    if events.len() >= limit.count {
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
    if self.allow(action, key, limit) {
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
  fn windows_expire() {
    let limiter = RateLimiter::new();
    let limit = Limit::new(1, 0);
    assert!(limiter.allow(Action::View, "a", limit));
    assert!(limiter.allow(Action::View, "a", limit));
  }
}
