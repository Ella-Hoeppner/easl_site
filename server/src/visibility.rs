//! Who can see sketches and playlists.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::auth::Viewer;

/// How widely a sketch or playlist is shared.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
  /// Only the owner (and admins) can see it.
  Private,
  /// Anyone with the link can see it; it's never listed.
  Unlisted,
  /// Anyone can see it, and it's listed in explore and on profiles.
  Public,
}

impl Visibility {
  pub fn as_str(self) -> &'static str {
    match self {
      Visibility::Private => "private",
      Visibility::Unlisted => "unlisted",
      Visibility::Public => "public",
    }
  }

  pub fn parse(text: &str) -> Option<Self> {
    match text {
      "private" => Some(Visibility::Private),
      "unlisted" => Some(Visibility::Unlisted),
      "public" => Some(Visibility::Public),
      _ => None,
    }
  }
}

impl fmt::Display for Visibility {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str(self.as_str())
  }
}

impl TryFrom<String> for Visibility {
  type Error = String;

  fn try_from(text: String) -> Result<Self, String> {
    Visibility::parse(&text).ok_or_else(|| format!("bad visibility {text:?}"))
  }
}

/// What decides whether a viewer can see something.
#[derive(Clone, Copy, Debug)]
pub struct Access {
  pub owner_id: i64,
  pub visibility: Visibility,
  /// Hidden by a moderator.
  pub hidden: bool,
  pub owner_banned: bool,
}

impl Access {
  /// Owners and admins see everything; anyone else sees what isn't
  /// private, hidden, or owned by a banned user.
  pub fn visible_to(&self, viewer: &Viewer) -> bool {
    if viewer.is(self.owner_id) || viewer.is_admin {
      return true;
    }
    !self.hidden && !self.owner_banned && self.visibility != Visibility::Private
  }
}

/// SQL for "a sketch `s` with owner `u` is listed publicly" (public, not
/// hidden, owner not banned).
pub const LISTED_SKETCH_SQL: &str = "(s.visibility = 'public' AND \
  s.hidden_at IS NULL AND u.banned_at IS NULL)";

/// SQL for [`Access::visible_to`] on a sketch `s` with owner `u`; binds the
/// viewer's id (or NULL) and whether they're an admin.
pub const VISIBLE_SKETCH_SQL: &str = "(s.owner_id IS ? OR ? OR \
  (s.visibility != 'private' AND s.hidden_at IS NULL AND \
  u.banned_at IS NULL))";
