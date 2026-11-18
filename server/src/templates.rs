//! HTML templates (`server/templates/`), rendered with minijinja.
//!
//! Release builds embed the templates in the binary. In dev they're loaded
//! from disk for every render, so edits show on refresh. Templates ending in
//! `.html` are autoescaped. Besides the page's own context, every template
//! can use:
//!
//! - `static(path)`: the versioned URL of an app asset (`/static/app.css?v=…`)
//! - `value|rich`: plain user text as HTML (escaped, line breaks, links)
//! - `timestamp|date` ("March 4, 2026"), `|month` ("March 2026"), `|ago`
//!   ("3 days ago")

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use minijinja::{Environment, Value, path_loader};

use crate::assets::Assets;
use crate::db::now;
use crate::text::{format_ago, format_date, format_month, rich_text};

/// The embedded templates: name and source.
const TEMPLATES: &[(&str, &str)] = &[
  ("base.html", include_str!("../templates/base.html")),
  ("macros.html", include_str!("../templates/macros.html")),
  ("explore.html", include_str!("../templates/explore.html")),
  ("profile.html", include_str!("../templates/profile.html")),
  ("playlist.html", include_str!("../templates/playlist.html")),
  (
    "playlist_new.html",
    include_str!("../templates/playlist_new.html"),
  ),
  ("login.html", include_str!("../templates/login.html")),
  ("signup.html", include_str!("../templates/signup.html")),
  (
    "check_email.html",
    include_str!("../templates/check_email.html"),
  ),
  ("forgot.html", include_str!("../templates/forgot.html")),
  ("reset.html", include_str!("../templates/reset.html")),
  ("message.html", include_str!("../templates/message.html")),
  ("settings.html", include_str!("../templates/settings.html")),
  ("admin.html", include_str!("../templates/admin.html")),
  (
    "admin_macros.html",
    include_str!("../templates/admin_macros.html"),
  ),
  (
    "admin_user.html",
    include_str!("../templates/admin_user.html"),
  ),
  ("error.html", include_str!("../templates/error.html")),
  ("editor.html", include_str!("../templates/editor.html")),
];

/// The directory dev mode loads templates from.
fn dev_dir() -> PathBuf {
  PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("templates")
}

/// The template set.
pub struct Templates {
  /// The embedded environment; `None` in dev, which builds one per render.
  embedded: Option<Environment<'static>>,
  assets: Arc<Assets>,
}

impl Templates {
  pub fn new(dev: bool, assets: Arc<Assets>) -> anyhow::Result<Self> {
    let embedded = if dev {
      None
    } else {
      let mut env = environment(assets.clone());
      for (name, source) in TEMPLATES {
        env
          .add_template(name, source)
          .with_context(|| format!("parsing the template {name}"))?;
      }
      Some(env)
    };
    Ok(Templates { embedded, assets })
  }

  /// Renders a template with a context.
  pub fn render(&self, name: &str, context: Value) -> anyhow::Result<String> {
    let rendered = match &self.embedded {
      Some(env) => env.get_template(name)?.render(context),
      None => {
        let mut env = environment(self.assets.clone());
        env.set_loader(path_loader(dev_dir()));
        env.get_template(name)?.render(context)
      }
    };
    rendered.with_context(|| format!("rendering the template {name}"))
  }
}

/// An environment with the site's filters and functions.
fn environment(assets: Arc<Assets>) -> Environment<'static> {
  let mut env = Environment::new();
  env.add_function("static", move |path: String| assets.url(&path));
  env.add_filter("rich", |text: String| {
    Value::from_safe_string(rich_text(&text))
  });
  env.add_filter("date", |timestamp: i64| format_date(timestamp));
  env.add_filter("month", |timestamp: i64| format_month(timestamp));
  env.add_filter("ago", |timestamp: i64| format_ago(timestamp, now()));
  env
}
