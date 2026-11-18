//! Rendering user-written plain text (descriptions, bios, comments) as
//! HTML, and date formatting for templates.

use time::OffsetDateTime;
use time::macros::format_description;

/// Escapes text for HTML element content and attribute values.
pub fn escape_html(text: &str) -> String {
  let mut out = String::with_capacity(text.len());
  for c in text.chars() {
    match c {
      '&' => out.push_str("&amp;"),
      '<' => out.push_str("&lt;"),
      '>' => out.push_str("&gt;"),
      '"' => out.push_str("&quot;"),
      '\'' => out.push_str("&#39;"),
      _ => out.push(c),
    }
  }
  out
}

/// Plain text as HTML: every character escaped, line breaks as `<br>`, and
/// `http://` and `https://` URLs as links that don't pass ranking credit or
/// a window reference.
pub fn rich_text(text: &str) -> String {
  let mut out = String::with_capacity(text.len() + 16);
  let mut rest = text;
  while let Some(start) = find_url_start(rest) {
    push_text(&mut out, &rest[..start]);
    let length = url_length(&rest[start..]);
    let url = &rest[start..start + length];
    let escaped = escape_html(url);
    out.push_str(&format!(
      "<a href=\"{escaped}\" rel=\"nofollow ugc noopener\">{escaped}</a>"
    ));
    rest = &rest[start + length..];
  }
  push_text(&mut out, rest);
  out
}

fn push_text(out: &mut String, text: &str) {
  for (i, line) in text.split('\n').enumerate() {
    if i > 0 {
      out.push_str("<br>");
    }
    out.push_str(&escape_html(line.trim_end_matches('\r')));
  }
}

/// The byte offset of the next URL in `text`: an `http://` or `https://`
/// at the start of a word, followed by at least one URL character.
fn find_url_start(text: &str) -> Option<usize> {
  let mut offset = 0;
  while offset < text.len() {
    let found = ["http://", "https://"]
      .iter()
      .filter_map(|scheme| {
        text[offset..]
          .find(scheme)
          .map(|i| (offset + i, scheme.len()))
      })
      .min_by_key(|(i, _)| *i)?;
    let (start, scheme_len) = found;
    let at_word_start = text[..start]
      .chars()
      .next_back()
      .is_none_or(|c| !c.is_alphanumeric());
    if at_word_start && url_length(&text[start..]) > scheme_len {
      return Some(start);
    }
    offset = start + scheme_len;
  }
  None
}

/// How long the URL at the start of `text` is: up to whitespace or a
/// character that can't appear in one, without trailing punctuation (and a
/// trailing `)` only when it closes a `(` inside the URL).
fn url_length(text: &str) -> usize {
  let end = text
    .find(|c: char| {
      c.is_whitespace() || c.is_control() || matches!(c, '<' | '>' | '"')
    })
    .unwrap_or(text.len());
  let mut url = &text[..end];
  loop {
    let trimmed = url.trim_end_matches(['.', ',', ';', ':', '!', '?', '\'']);
    let trimmed = if trimmed.ends_with(')')
      && trimmed.matches('(').count() < trimmed.matches(')').count()
    {
      &trimmed[..trimmed.len() - 1]
    } else {
      trimmed
    };
    if trimmed.len() == url.len() {
      return url.len();
    }
    url = trimmed;
  }
}

/// Percent-encodes text for a URL query value: everything but unreserved
/// characters (`A-Z a-z 0-9 - . _ ~`) and `/`.
pub fn url_encode(text: &str) -> String {
  let mut out = String::with_capacity(text.len());
  for byte in text.bytes() {
    if byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte) {
      out.push(byte as char);
    } else {
      out.push_str(&format!("%{byte:02X}"));
    }
  }
  out
}

/// The first `max_chars` characters of `text`, with `…` added when it's
/// longer.
pub fn excerpt(text: &str, max_chars: usize) -> String {
  match text.char_indices().nth(max_chars) {
    Some((end, _)) => format!("{}…", &text[..end]),
    None => text.to_string(),
  }
}

/// A unix timestamp as a calendar date ("March 4, 2026").
pub fn format_date(timestamp: i64) -> String {
  OffsetDateTime::from_unix_timestamp(timestamp)
    .ok()
    .and_then(|t| {
      t.format(format_description!(
        "[month repr:long] [day padding:none], [year]"
      ))
      .ok()
    })
    .unwrap_or_default()
}

/// A unix timestamp as a month and year ("March 2026").
pub fn format_month(timestamp: i64) -> String {
  OffsetDateTime::from_unix_timestamp(timestamp)
    .ok()
    .and_then(|t| {
      t.format(format_description!("[month repr:long] [year]"))
        .ok()
    })
    .unwrap_or_default()
}

/// How long ago a unix timestamp was, relative to `now` ("5 minutes ago",
/// "3 days ago"), falling back to the date after a month.
pub fn format_ago(timestamp: i64, now: i64) -> String {
  let seconds = (now - timestamp).max(0);
  let plural = |n: i64, unit: &str| {
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
  };
  match seconds {
    0..60 => "just now".into(),
    60..3600 => plural(seconds / 60, "minute"),
    3600..86400 => plural(seconds / 3600, "hour"),
    86400..2_592_000 => plural(seconds / 86400, "day"),
    _ => format_date(timestamp),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn escapes_everything() {
    assert_eq!(
      rich_text("<script>alert('x')</script>"),
      "&lt;script&gt;alert(&#39;x&#39;)&lt;/script&gt;"
    );
  }

  #[test]
  fn line_breaks() {
    assert_eq!(rich_text("a\r\nb\nc"), "a<br>b<br>c");
  }

  #[test]
  fn links_urls() {
    assert_eq!(
      rich_text("see https://a.org/x?y=1&z=2."),
      "see <a href=\"https://a.org/x?y=1&amp;z=2\" \
       rel=\"nofollow ugc noopener\">https://a.org/x?y=1&amp;z=2</a>."
    );
    assert_eq!(
      rich_text("(http://a.org/b)"),
      "(<a href=\"http://a.org/b\" rel=\"nofollow ugc noopener\">\
       http://a.org/b</a>)"
    );
    assert!(
      rich_text("http://en.wikipedia.org/wiki/A_(b)")
        .contains("href=\"http://en.wikipedia.org/wiki/A_(b)\"")
    );
  }

  #[test]
  fn ignores_other_schemes_and_bare_schemes() {
    assert_eq!(rich_text("javascript:alert(1)"), "javascript:alert(1)");
    assert_eq!(rich_text("https:// x"), "https:// x");
    assert_eq!(rich_text("xhttps://a.org"), "xhttps://a.org");
  }

  #[test]
  fn link_attributes_cannot_be_broken_out_of() {
    let html = rich_text("https://a.org/\"onmouseover=alert(1)");
    assert!(!html.contains("\"onmouseover"));
  }

  #[test]
  fn url_encoding() {
    assert_eq!(url_encode("/s/a?b=c&d"), "/s/a%3Fb%3Dc%26d");
  }

  #[test]
  fn excerpts() {
    assert_eq!(excerpt("short", 5), "short");
    assert_eq!(excerpt("longer", 5), "longe…");
    assert_eq!(excerpt("ééé", 2), "éé…");
  }

  #[test]
  fn relative_times() {
    assert_eq!(format_ago(100, 130), "just now");
    assert_eq!(format_ago(0, 120), "2 minutes ago");
    assert_eq!(format_ago(0, 3600), "1 hour ago");
    assert_eq!(format_ago(0, 86400 * 3), "3 days ago");
  }
}
