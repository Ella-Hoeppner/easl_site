//! Validation of user input. Each function returns the cleaned value, or a
//! [`AppError::BadRequest`] whose message can be shown as is.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::error::AppError;

/// Usernames that would collide with site paths or impersonate the site.
const RESERVED_USERNAMES: &[&str] = &[
  "about",
  "account",
  "admin",
  "administrator",
  "api",
  "assets",
  "blog",
  "docs",
  "easl",
  "easl-lang",
  "easl-web",
  "editor",
  "explore",
  "forgot",
  "guide",
  "help",
  "login",
  "logout",
  "me",
  "mod",
  "moderator",
  "new",
  "p",
  "playground",
  "playlist",
  "playlists",
  "reference",
  "reset",
  "root",
  "s",
  "settings",
  "signin",
  "signout",
  "signup",
  "sketch",
  "sketches",
  "static",
  "support",
  "system",
  "u",
  "user",
  "users",
  "verify",
  "www",
];

/// The longest accepted request body, in bytes.
pub const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;

/// The longest accepted thumbnail, in bytes.
pub const MAX_THUMBNAIL_BYTES: usize = 300 * 1024;

const MAX_FILES: usize = 32;
const MAX_PATH_CHARS: usize = 128;
const MAX_FILE_BYTES: usize = 200 * 1024;
const MAX_TOTAL_FILE_BYTES: usize = 1024 * 1024;

/// The most tags a sketch can have.
pub const MAX_TAGS: usize = 8;

/// The longest a tag can be, in characters.
const MAX_TAG_CHARS: usize = 32;

fn bad(message: impl Into<String>) -> AppError {
  AppError::bad_request(message)
}

/// A username: 3–24 characters of `[A-Za-z0-9_-]`, not reserved.
pub fn username(raw: &str) -> Result<String, AppError> {
  let name = raw.trim();
  let length = name.chars().count();
  if !(3..=24).contains(&length) {
    return Err(bad("Usernames are 3 to 24 characters long."));
  }
  if !name
    .bytes()
    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
  {
    return Err(bad(
      "Usernames can only contain letters, digits, '_', and '-'.",
    ));
  }
  if RESERVED_USERNAMES.contains(&name.to_ascii_lowercase().as_str()) {
    return Err(bad("That username isn't available."));
  }
  Ok(name.to_string())
}

/// Whether a string could be a username, for routing lookups.
pub fn is_username_shaped(name: &str) -> bool {
  (1..=24).contains(&name.len())
    && name
      .bytes()
      .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// A password of 10–256 characters.
pub fn password(raw: &str) -> Result<&str, AppError> {
  let length = raw.chars().count();
  if length < 10 {
    return Err(bad("Passwords need at least 10 characters."));
  }
  if length > 256 {
    return Err(bad("Passwords can be at most 256 characters."));
  }
  Ok(raw)
}

/// An email address: one `@` with something on both sides, a dot in the
/// domain, no whitespace, at most 254 characters.
pub fn email(raw: &str) -> Result<String, AppError> {
  let address = raw.trim();
  let invalid = || bad("Enter a valid email address.");
  if address.len() > 254 || address.chars().any(char::is_whitespace) {
    return Err(invalid());
  }
  let (local, domain) = address.split_once('@').ok_or_else(invalid)?;
  if local.is_empty()
    || domain.contains('@')
    || !domain.contains('.')
    || domain.starts_with('.')
    || domain.ends_with('.')
    || address
      .chars()
      .any(|c| c.is_control() || c == '<' || c == '>')
  {
    return Err(invalid());
  }
  Ok(address.to_string())
}

/// Optional text of at most `max` characters, trimmed; `what` names it in
/// the error.
pub fn optional_text(
  raw: &str,
  max: usize,
  what: &str,
) -> Result<String, AppError> {
  let text = raw.trim();
  if text.chars().count() > max {
    return Err(bad(format!("{what} can be at most {max} characters.")));
  }
  Ok(text.to_string())
}

/// Required text of 1 to `max` characters, trimmed.
pub fn required_text(
  raw: &str,
  max: usize,
  what: &str,
) -> Result<String, AppError> {
  let text = optional_text(raw, max, what)?;
  if text.is_empty() {
    return Err(bad(format!("{what} can't be empty.")));
  }
  Ok(text)
}

pub fn display_name(raw: &str) -> Result<String, AppError> {
  optional_text(raw, 50, "Display names")
}

pub fn bio(raw: &str) -> Result<String, AppError> {
  optional_text(raw, 500, "Bios")
}

pub fn sketch_title(raw: &str) -> Result<String, AppError> {
  required_text(raw, 100, "Titles")
}

pub fn sketch_description(raw: &str) -> Result<String, AppError> {
  optional_text(raw, 5000, "Descriptions")
}

pub fn comment_body(raw: &str) -> Result<String, AppError> {
  required_text(raw, 2000, "Comments")
}

pub fn playlist_title(raw: &str) -> Result<String, AppError> {
  required_text(raw, 100, "Playlist titles")
}

pub fn playlist_description(raw: &str) -> Result<String, AppError> {
  optional_text(raw, 2000, "Playlist descriptions")
}

pub fn report_reason(raw: &str) -> Result<String, AppError> {
  optional_text(raw, 1000, "Report reasons")
}

/// A tag as typed, put in its stored form: trimmed, lowercased, without a
/// leading `#`, with each run of whitespace, underscores, and `-` turned
/// into one `-`. The result isn't necessarily a valid tag ([`tag`] checks).
pub fn normalize_tag(raw: &str) -> String {
  let text = raw.trim();
  let text = text.strip_prefix('#').unwrap_or(text).trim_start();
  let mut tag = String::with_capacity(text.len());
  let mut in_gap = false;
  for c in text.chars() {
    if c.is_whitespace() || c == '_' || c == '-' {
      if !in_gap {
        tag.push('-');
      }
      in_gap = true;
    } else {
      tag.extend(c.to_lowercase());
      in_gap = false;
    }
  }
  tag
}

/// A tag, normalized ([`normalize_tag`]): 1–32 characters of lowercase
/// ASCII letters, digits, and `-`, not starting or ending with `-`.
pub fn tag(raw: &str) -> Result<String, AppError> {
  let tag = normalize_tag(raw);
  let valid = (1..=MAX_TAG_CHARS).contains(&tag.chars().count())
    && tag
      .bytes()
      .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    && !tag.starts_with('-')
    && !tag.ends_with('-');
  if valid {
    Ok(tag)
  } else {
    Err(bad(format!(
      "Invalid tag {raw:?}: tags are 1 to {MAX_TAG_CHARS} lowercase \
       letters, digits, and '-', not starting or ending with '-'."
    )))
  }
}

/// A sketch's tags: each valid ([`tag`]), at most 8, distinct after
/// normalization, in the order given.
pub fn tags(raw: &[String]) -> Result<Vec<String>, AppError> {
  let mut tags = Vec::with_capacity(raw.len());
  for text in raw {
    let tag = tag(text)?;
    if tags.contains(&tag) {
      return Err(bad(format!("The tag {tag:?} appears twice.")));
    }
    tags.push(tag);
  }
  if tags.len() > MAX_TAGS {
    return Err(bad(format!("A sketch can have at most {MAX_TAGS} tags.")));
  }
  Ok(tags)
}

/// One file of a sketch, as sent by clients and stored.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct SketchFile {
  pub path: String,
  pub content: String,
}

/// A file path: relative, `/`-separated segments of `[A-Za-z0-9._-]+`, no
/// `.` or `..` segments, ending in `.easl`, at most 128 characters.
pub fn file_path(path: &str) -> Result<(), AppError> {
  let invalid = |why: &str| bad(format!("Invalid file path {path:?}: {why}."));
  if path.is_empty() || path.len() > MAX_PATH_CHARS {
    return Err(invalid("paths are 1 to 128 characters long"));
  }
  if !path.ends_with(".easl") {
    return Err(invalid("files must end in .easl"));
  }
  for segment in path.split('/') {
    if segment.is_empty() {
      return Err(invalid("empty path segment"));
    }
    if segment == "." || segment == ".." {
      return Err(invalid("'.' and '..' aren't allowed"));
    }
    if !segment
      .bytes()
      .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
    {
      return Err(invalid(
        "use only letters, digits, '.', '_', '-', and '/' between folders",
      ));
    }
  }
  Ok(())
}

/// A sketch's set of files: 1–32 valid, distinct paths, each file at most
/// 200 KB, all together at most 1 MB.
pub fn sketch_files(files: &[SketchFile]) -> Result<(), AppError> {
  if files.is_empty() {
    return Err(bad("A sketch needs at least one file."));
  }
  if files.len() > MAX_FILES {
    return Err(bad(format!("A sketch can have at most {MAX_FILES} files.")));
  }
  let mut seen = HashSet::new();
  let mut total = 0;
  for file in files {
    file_path(&file.path)?;
    if !seen.insert(file.path.as_str()) {
      return Err(bad(format!("The file {:?} appears twice.", file.path)));
    }
    if file.content.len() > MAX_FILE_BYTES {
      return Err(AppError::TooLarge(format!(
        "The file {:?} is larger than 200 KB.",
        file.path
      )));
    }
    total += file.content.len();
  }
  if total > MAX_TOTAL_FILE_BYTES {
    return Err(AppError::TooLarge(
      "A sketch's files can total at most 1 MB.".into(),
    ));
  }
  Ok(())
}

/// The file a sketch opens with: one of its files' paths.
pub fn entry_file(
  entry: Option<&str>,
  files: &[SketchFile],
) -> Result<(), AppError> {
  match entry {
    Some(path) if !files.iter().any(|f| f.path == path) => Err(bad(format!(
      "The entry file {path:?} isn't one of the sketch's files."
    ))),
    _ => Ok(()),
  }
}

/// An entry function name: 1–100 characters without whitespace,
/// parentheses, brackets, quotes, or control characters. The server doesn't
/// parse easl, so this only rules out what can't be an identifier.
pub fn entry_name(name: &str) -> Result<(), AppError> {
  let plausible = !name.is_empty()
    && name.chars().count() <= 100
    && !name.chars().any(|c| {
      c.is_whitespace()
        || c.is_control()
        || matches!(c, '(' | ')' | '[' | ']' | '{' | '}' | '"' | ';')
    });
  if plausible {
    Ok(())
  } else {
    Err(bad(format!("{name:?} isn't a valid entry name.")))
  }
}

/// Whether bytes start the way an image format's files do.
type MagicCheck = fn(&[u8]) -> bool;

/// Image formats accepted as thumbnails.
const THUMBNAIL_FORMATS: &[(&str, MagicCheck)] = &[
  ("image/png", |b| b.starts_with(b"\x89PNG\r\n\x1a\n")),
  ("image/jpeg", |b| b.starts_with(&[0xFF, 0xD8, 0xFF])),
  ("image/webp", |b| {
    b.len() >= 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP"
  }),
];

/// A thumbnail upload: a PNG, JPEG, or WebP of at most 300 KB whose bytes
/// match its declared type. Returns the type to store.
pub fn thumbnail(
  content_type: Option<&str>,
  data: &[u8],
) -> Result<&'static str, AppError> {
  if data.len() > MAX_THUMBNAIL_BYTES {
    return Err(AppError::TooLarge(
      "Thumbnails can be at most 300 KB.".into(),
    ));
  }
  let declared = content_type
    .and_then(|t| t.split(';').next())
    .map(str::trim)
    .unwrap_or("");
  let (mime, matches) = THUMBNAIL_FORMATS
    .iter()
    .find(|(mime, _)| mime.eq_ignore_ascii_case(declared))
    .ok_or_else(|| bad("Thumbnails must be PNG, JPEG, or WebP images."))?;
  if !matches(data) {
    return Err(bad(format!("The thumbnail isn't a valid {mime} image.")));
  }
  Ok(mime)
}

/// A same-site path to return to after signing in: starts with a single
/// `/`, with no backslashes or control characters. Anything else becomes
/// `/`.
pub fn next_path(next: Option<&str>) -> String {
  match next {
    Some(path)
      if path.starts_with('/')
        && !path.starts_with("//")
        && !path.contains('\\')
        && !path.chars().any(char::is_control)
        && path.len() <= 2000 =>
    {
      path.to_string()
    }
    _ => "/".to_string(),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn file(path: &str) -> SketchFile {
    SketchFile {
      path: path.into(),
      content: String::new(),
    }
  }

  #[test]
  fn usernames() {
    assert!(username("ella_h-1").is_ok());
    assert!(username("ab").is_err());
    assert!(username("a".repeat(25).as_str()).is_err());
    assert!(username("bad name").is_err());
    assert!(username("Admin").is_err());
    assert!(username("émile").is_err());
  }

  #[test]
  fn emails() {
    assert!(email("a@b.co").is_ok());
    assert!(email(" a@b.co ").is_ok_and(|e| e == "a@b.co"));
    assert!(email("a@b").is_err());
    assert!(email("@b.co").is_err());
    assert!(email("a b@c.co").is_err());
    assert!(email("a@b@c.co").is_err());
  }

  #[test]
  fn paths() {
    assert!(file_path("main.easl").is_ok());
    assert!(file_path("lib/noise-2.easl").is_ok());
    assert!(file_path("/main.easl").is_err());
    assert!(file_path("../main.easl").is_err());
    assert!(file_path("a/./b.easl").is_err());
    assert!(file_path("a//b.easl").is_err());
    assert!(file_path("main.txt").is_err());
    assert!(file_path("ma in.easl").is_err());
    assert!(file_path(&format!("{}.easl", "a".repeat(124))).is_err());
  }

  #[test]
  fn file_sets() {
    assert!(sketch_files(&[]).is_err());
    assert!(sketch_files(&[file("a.easl"), file("a.easl")]).is_err());
    assert!(sketch_files(&[file("a.easl"), file("A.easl")]).is_ok());
    let many: Vec<_> = (0..33).map(|i| file(&format!("f{i}.easl"))).collect();
    assert!(sketch_files(&many).is_err());
  }

  #[test]
  fn entry_names() {
    assert!(entry_name("main").is_ok());
    assert!(entry_name("make-two-of").is_ok());
    assert!(entry_name("a b").is_err());
    assert!(entry_name("(x)").is_err());
    assert!(entry_name("").is_err());
  }

  #[test]
  fn next_paths() {
    assert_eq!(next_path(Some("/s/abc")), "/s/abc");
    assert_eq!(next_path(Some("//evil.com")), "/");
    assert_eq!(next_path(Some("https://evil.com")), "/");
    assert_eq!(next_path(Some("/\\evil.com")), "/");
    assert_eq!(next_path(None), "/");
  }

  #[test]
  fn tag_normalization() {
    assert_eq!(normalize_tag("  #Ray Marching "), "ray-marching");
    assert_eq!(normalize_tag("game_of  life"), "game-of-life");
    assert_eq!(normalize_tag("# 3D"), "3d");
    assert_eq!(normalize_tag("a _ b"), "a-b");
    assert_eq!(normalize_tag("a - b--c"), "a-b-c");
    assert_eq!(tag("#SDF").ok().as_deref(), Some("sdf"));
    assert_eq!(tag("2d-noise").ok().as_deref(), Some("2d-noise"));
  }

  #[test]
  fn tag_rules() {
    assert!(tag("").is_err());
    assert!(tag("#").is_err());
    assert!(tag("   ").is_err());
    assert!(tag("-edge").is_err());
    assert!(tag("edge-").is_err());
    assert!(tag("_edge").is_err());
    assert!(tag("c++").is_err());
    assert!(tag("émile").is_err());
    assert!(tag("a.b").is_err());
    assert!(tag(&"a".repeat(32)).is_ok());
    assert!(tag(&"a".repeat(33)).is_err());
    let message = tag("c++").expect_err("bad").message();
    assert!(message.contains("\"c++\""), "{message}");
    assert!(message.contains("lowercase letters"), "{message}");
  }

  #[test]
  fn tag_sets() {
    let strings = |tags: &[&str]| -> Vec<String> {
      tags.iter().map(|t| t.to_string()).collect()
    };
    assert_eq!(
      tags(&strings(&["Zeta", "#alpha", "mid dle"])).ok(),
      Some(strings(&["zeta", "alpha", "mid-dle"]))
    );
    assert!(tags(&[]).is_ok_and(|t| t.is_empty()));
    assert!(tags(&strings(&["Noise", "#noise"])).is_err());
    let eight: Vec<_> = (0..8).map(|i| format!("t{i}")).collect();
    assert!(tags(&eight).is_ok());
    let nine: Vec<_> = (0..9).map(|i| format!("t{i}")).collect();
    assert!(tags(&nine).is_err());
    assert!(tags(&strings(&["fine", "not fine!"])).is_err());
  }

  #[test]
  fn thumbnails() {
    let png = b"\x89PNG\r\n\x1a\n rest";
    assert_eq!(thumbnail(Some("image/png"), png).ok(), Some("image/png"));
    assert!(thumbnail(Some("image/jpeg"), png).is_err());
    assert!(thumbnail(Some("image/gif"), b"GIF89a").is_err());
    assert!(thumbnail(None, png).is_err());
  }
}
