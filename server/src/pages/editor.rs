//! The editor pages (`/new`, `/s/{id}`), which both render `editor.html`.
//!
//! Besides the base context (which includes `docs_origin`), the template
//! gets:
//!
//! - `page_json`: JSON for a `<script type="application/json">` element,
//!   already safe there (`<`, `>`, `&`, and the line separators are escaped):
//!   `{"mode": "new" | "sketch", "user", "sketch", "playlist",
//!   "editor_origin", "docs_origin", "csrf_ok"}`
//! - `asset_version` and `monaco_version`, from the editor manifest
//! - `page_title`, and `og` = `{title, description, image, url}` for link
//!   previews (`image` is absolute or none)

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use minijinja::{Value, context};
use serde::{Deserialize, Serialize};

use super::{PageContext, html_response, render_html};
use crate::auth::Viewer;
use crate::config::Limit;
use crate::error::{AppError, PageResult};
use crate::playlists::{self, PlaylistPosition};
use crate::ratelimit::Action;
use crate::security::ClientIp;
use crate::sketches::{self, Sketch};
use crate::state::AppState;
use crate::users::Me;

/// The site's description, for link previews of pages without their own.
const SITE_DESCRIPTION: &str = "Write easl in your browser and run it live: \
  shaders, compute, and the CPU code that drives them, all in one program.";

/// A view is counted at most once per viewer and sketch in this window.
const VIEW_WINDOW: Limit = Limit::new(1, 60 * 60);

/// The data the editor's script reads.
#[derive(Serialize)]
struct PageData<'a> {
  mode: &'static str,
  user: Option<Me>,
  sketch: Option<Sketch>,
  playlist: Option<PlaylistPosition>,
  editor_origin: &'a str,
  docs_origin: &'a str,
  csrf_ok: bool,
}

impl<'a> PageData<'a> {
  /// The data of a page in `mode`, with nothing loaded yet.
  fn new(state: &'a AppState, page: &PageContext, mode: &'static str) -> Self {
    PageData {
      mode,
      user: page.user.as_ref().map(|u| u.me()),
      sketch: None,
      playlist: None,
      editor_origin: &state.config.editor_origin,
      docs_origin: &state.config.docs_origin,
      csrf_ok: true,
    }
  }
}

/// Link-preview metadata.
#[derive(Serialize)]
struct OpenGraph {
  title: String,
  description: String,
  image: Option<String>,
  url: String,
}

/// JSON that is safe inside a `<script>` element: nothing in it can close
/// the element or start a comment, and JavaScript line separators are
/// escaped.
fn script_safe_json(data: &impl Serialize) -> serde_json::Result<String> {
  Ok(
    serde_json::to_string(data)?
      .replace('<', "\\u003c")
      .replace('>', "\\u003e")
      .replace('&', "\\u0026")
      .replace('\u{2028}', "\\u2028")
      .replace('\u{2029}', "\\u2029"),
  )
}

/// A description cut to a link-preview length, on a word boundary.
fn preview_text(text: &str) -> String {
  const MAX: usize = 200;
  let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
  if text.chars().count() <= MAX {
    return text;
  }
  let cut: String = text.chars().take(MAX).collect();
  let cut = cut.rsplit_once(' ').map_or(cut.as_str(), |(head, _)| head);
  format!("{cut}…")
}

fn render_editor(
  state: &AppState,
  page: &PageContext,
  data: &PageData,
  page_title: &str,
  og: OpenGraph,
) -> PageResult {
  let manifest = state.site.manifest();
  let page_json = script_safe_json(data).map_err(AppError::from_error)?;
  let context = context! {
    page_json => Value::from_safe_string(page_json),
    asset_version => manifest.version,
    monaco_version => manifest.monaco_version,
    page_title,
    og,
  };
  let body = render_html(
    state,
    page.user.as_ref(),
    &page.path,
    "editor.html",
    context,
  )?;
  Ok(html_response(StatusCode::OK, body, false))
}

/// `GET /new`: an empty editor for a new sketch.
pub async fn new_sketch(
  State(state): State<AppState>,
  page: PageContext,
) -> PageResult {
  let data = PageData::new(&state, &page, "new");
  let og = OpenGraph {
    title: "New sketch — easl".into(),
    description: SITE_DESCRIPTION.into(),
    image: None,
    url: state.config.editor_url("/new"),
  };
  render_editor(&state, &page, &data, "New sketch — easl", og)
}

#[derive(Deserialize)]
pub struct SketchQuery {
  playlist: Option<String>,
}

/// `GET /s/{id}?playlist=<pid>`: a sketch, counting a view when the viewer
/// isn't its owner (once an hour per viewer).
pub async fn sketch(
  State(state): State<AppState>,
  page: PageContext,
  ip: ClientIp,
  Path(id): Path<String>,
  Query(query): Query<SketchQuery>,
) -> PageResult {
  let viewer = Viewer::of(page.user.as_ref());
  let mut row = sketches::find_visible(&state.db, &id, &viewer).await?;
  if !viewer.is(row.owner_id) {
    let viewer_key = match viewer.id {
      Some(user_id) => format!("u{user_id}:{id}"),
      None => format!("{}:{id}", ip.0),
    };
    if state.limiter.allow(Action::View, &viewer_key, VIEW_WINDOW) {
      sketches::add_view(&state.db, &id).await?;
      row.view_count += 1;
    }
  }
  let playlist = match &query.playlist {
    Some(playlist_id) => {
      playlists::position_of(&state.db, &viewer, playlist_id, &id).await?
    }
    None => None,
  };
  let sketch = row.detail(&state.db, &viewer).await?;
  let description = if sketch.description.trim().is_empty() {
    format!("A sketch by {} on easl.", sketch.summary.owner.display_name)
  } else {
    preview_text(&sketch.description)
  };
  let og = OpenGraph {
    title: format!("{} — easl", sketch.summary.title),
    description,
    image: Some(state.config.editor_url(&sketch.summary.thumbnail_url)),
    url: state.config.editor_url(&sketch.summary.url),
  };
  let page_title = format!("{} — easl", sketch.summary.title);
  let data = PageData {
    sketch: Some(sketch),
    playlist,
    ..PageData::new(&state, &page, "sketch")
  };
  render_editor(&state, &page, &data, &page_title, og)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn json_cannot_close_the_script() {
    let json = script_safe_json(&"</script><!-- & \u{2028}").unwrap();
    assert!(!json.contains('<'));
    assert!(!json.contains('>'));
    assert!(!json.contains('&'));
    assert!(!json.contains('\u{2028}'));
  }

  #[test]
  fn preview_text_cuts_on_words() {
    let long = "word ".repeat(100);
    let cut = preview_text(&long);
    assert!(cut.ends_with("word…"));
    assert!(cut.chars().count() <= 201);
  }
}
