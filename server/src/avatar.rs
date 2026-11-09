//! Generated images: user avatars and placeholder sketch thumbnails, as
//! SVG drawn deterministically from a seed string.

use sha2::{Digest, Sha256};

use crate::text::escape_html;

/// A stream of pseudo-random numbers derived from a seed string.
struct Seeded {
  bytes: [u8; 32],
  next: usize,
}

impl Seeded {
  fn new(seed: &str) -> Self {
    Seeded {
      bytes: Sha256::digest(seed.as_bytes()).into(),
      next: 0,
    }
  }

  /// A number in `0.0..1.0`.
  fn unit(&mut self) -> f64 {
    let byte = self.bytes[self.next % self.bytes.len()];
    self.next += 1;
    f64::from(byte) / 256.0
  }

  /// A number in `low..high`.
  fn range(&mut self, low: f64, high: f64) -> f64 {
    low + self.unit() * (high - low)
  }
}

/// An HSL color string.
fn hsl(hue: f64, saturation: f64, lightness: f64) -> String {
  format!(
    "hsl({:.0}, {:.0}%, {:.0}%)",
    hue.rem_euclid(360.0),
    saturation,
    lightness
  )
}

/// A palette of three related hues from a seed: a base hue and two
/// neighbors on an analogous or split-complementary spread.
fn palette(rng: &mut Seeded) -> [f64; 3] {
  let base = rng.range(0.0, 360.0);
  let spread = rng.range(35.0, 150.0);
  [base, base + spread, base - spread * rng.range(0.4, 1.0)]
}

/// The avatar for a username: a soft gradient field with glowing blobs and
/// faint contour rings, like a tiny distance-field render.
pub fn avatar_svg(username: &str) -> String {
  let mut rng = Seeded::new(&format!("avatar:{}", username.to_lowercase()));
  let [h0, h1, h2] = palette(&mut rng);
  let angle = rng.range(0.0, 360.0);
  let blobs: String = [h1, h2, h0 + 180.0]
    .iter()
    .enumerate()
    .map(|(i, &hue)| {
      let (x, y, r) =
        (rng.range(8.0, 56.0), rng.range(8.0, 56.0), rng.range(18.0, 34.0));
      format!(
        "<radialGradient id=\"b{i}\"><stop offset=\"0\" stop-color=\"{}\" \
         stop-opacity=\"0.95\"/><stop offset=\"1\" stop-color=\"{}\" \
         stop-opacity=\"0\"/></radialGradient>\
         <circle cx=\"{x:.1}\" cy=\"{y:.1}\" r=\"{r:.1}\" fill=\"url(#b{i})\"/>",
        hsl(hue, 85.0, 62.0),
        hsl(hue, 85.0, 62.0),
      )
    })
    .collect();
  let (cx, cy) = (rng.range(16.0, 48.0), rng.range(16.0, 48.0));
  let rings: String = (1..=4)
    .map(|i| {
      format!(
        "<circle cx=\"{cx:.1}\" cy=\"{cy:.1}\" r=\"{}\" fill=\"none\" \
         stroke=\"white\" stroke-opacity=\"{:.2}\" stroke-width=\"1\"/>",
        i * 9,
        0.22 - f64::from(i) * 0.04
      )
    })
    .collect();
  format!(
    "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 64 64\" \
     width=\"64\" height=\"64\">\
     <defs><linearGradient id=\"g\" gradientTransform=\"rotate({angle:.0} \
     0.5 0.5)\"><stop offset=\"0\" stop-color=\"{}\"/><stop offset=\"1\" \
     stop-color=\"{}\"/></linearGradient></defs>\
     <rect width=\"64\" height=\"64\" fill=\"url(#g)\"/>{blobs}{rings}</svg>",
    hsl(h0, 55.0, 30.0),
    hsl(h2, 60.0, 18.0),
  )
}

/// The thumbnail shown for a sketch without an uploaded one: a gradient
/// from the sketch's id with its title.
pub fn placeholder_thumbnail_svg(sketch_id: &str, title: &str) -> String {
  let mut rng = Seeded::new(&format!("thumb:{sketch_id}"));
  let [h0, h1, h2] = palette(&mut rng);
  let glows: String = [h1, h2]
    .iter()
    .enumerate()
    .map(|(i, &hue)| {
      let (x, y, r) = (
        rng.range(80.0, 560.0),
        rng.range(40.0, 320.0),
        rng.range(160.0, 300.0),
      );
      format!(
        "<radialGradient id=\"b{i}\"><stop offset=\"0\" stop-color=\"{}\" \
         stop-opacity=\"0.85\"/><stop offset=\"1\" stop-color=\"{}\" \
         stop-opacity=\"0\"/></radialGradient>\
         <circle cx=\"{x:.0}\" cy=\"{y:.0}\" r=\"{r:.0}\" \
         fill=\"url(#b{i})\"/>",
        hsl(hue, 80.0, 58.0),
        hsl(hue, 80.0, 58.0),
      )
    })
    .collect();
  let (cx, cy) = (rng.range(120.0, 520.0), rng.range(80.0, 280.0));
  let rings: String = (1..=7)
    .map(|i| {
      format!(
        "<circle cx=\"{cx:.0}\" cy=\"{cy:.0}\" r=\"{}\" fill=\"none\" \
         stroke=\"white\" stroke-opacity=\"0.07\" stroke-width=\"1.5\"/>",
        i * 38
      )
    })
    .collect();
  let title = truncate_chars(title, 34);
  format!(
    "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 640 360\" \
     width=\"640\" height=\"360\">\
     <defs><linearGradient id=\"g\" x1=\"0\" y1=\"0\" x2=\"1\" y2=\"1\">\
     <stop offset=\"0\" stop-color=\"{}\"/><stop offset=\"1\" \
     stop-color=\"{}\"/></linearGradient>\
     <linearGradient id=\"shade\" x1=\"0\" y1=\"0\" x2=\"0\" y2=\"1\">\
     <stop offset=\"0.55\" stop-color=\"black\" stop-opacity=\"0\"/>\
     <stop offset=\"1\" stop-color=\"black\" stop-opacity=\"0.55\"/>\
     </linearGradient></defs>\
     <rect width=\"640\" height=\"360\" fill=\"url(#g)\"/>{glows}{rings}\
     <rect width=\"640\" height=\"360\" fill=\"url(#shade)\"/>\
     <text x=\"28\" y=\"326\" fill=\"white\" fill-opacity=\"0.92\" \
     font-family=\"ui-monospace, SFMono-Regular, Menlo, Consolas, monospace\" \
     font-size=\"28\" font-weight=\"600\">{}</text></svg>",
    hsl(h0, 50.0, 22.0),
    hsl(h2, 55.0, 12.0),
    escape_html(&title),
  )
}

fn truncate_chars(text: &str, max: usize) -> String {
  if text.chars().count() <= max {
    text.to_string()
  } else {
    let mut short: String = text.chars().take(max - 1).collect();
    short.push('…');
    short
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn avatars_are_deterministic_and_case_insensitive() {
    assert_eq!(avatar_svg("Alice"), avatar_svg("alice"));
    assert_ne!(avatar_svg("alice"), avatar_svg("bob"));
  }

  #[test]
  fn placeholder_titles_are_escaped() {
    let svg = placeholder_thumbnail_svg("abcdefgh", "<b>&</b>");
    assert!(svg.contains("&lt;b&gt;&amp;&lt;/b&gt;"));
    assert!(!svg.contains("<b>"));
  }
}
