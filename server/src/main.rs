//! The `easl-site-server` command: `serve`, `sync-examples`, `seed`, and
//! `admin`.

use std::env;
use std::future;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, bail};
use clap::{Parser, Subcommand};
use sqlx::SqlitePool;
use tokio::net::TcpListener;
use tokio::signal;
#[cfg(unix)]
use tokio::signal::unix::{self, SignalKind};
use tokio::time;
use tracing_subscriber::EnvFilter;

use easl_site_server::app;
use easl_site_server::auth;
use easl_site_server::config::Config;
use easl_site_server::db;
use easl_site_server::examples;
use easl_site_server::mail::Mailer;
use easl_site_server::moderation;
use easl_site_server::seed;
use easl_site_server::site;
use easl_site_server::state::AppState;
use easl_site_server::users;

#[derive(Parser)]
#[command(name = "easl-site-server", about = "The easl site's server")]
struct Cli {
  #[command(subcommand)]
  command: Command,
}

#[derive(Subcommand)]
enum Command {
  /// Serves the site.
  Serve {
    /// The address to listen on.
    #[arg(long, default_value = "127.0.0.1:8000")]
    bind: SocketAddr,
    /// The generated static site.
    #[arg(long, default_value = "../site")]
    site_dir: PathBuf,
    /// The SQLite database (created if missing).
    #[arg(long, default_value = "./data/easl-site.db")]
    db: PathBuf,
    /// The app's public origin, e.g. https://editor.easl-lang.org
    /// (default: http://editor.localhost:<bind port>).
    #[arg(long)]
    editor_origin: Option<String>,
    /// The docs' public origin, e.g. https://easl-lang.org (default:
    /// http://localhost:<bind port>).
    #[arg(long)]
    docs_origin: Option<String>,
    /// Development mode: dev origins accepted, cookies not Secure, templates
    /// reloaded, emails logged and their links shown on the page.
    #[arg(long)]
    dev: bool,
    /// Take client addresses from X-Forwarded-For's last hop (set this
    /// behind a reverse proxy, and only then).
    #[arg(long)]
    trust_proxy: bool,
    /// Sync the site's examples to this user's sketches at startup (if the
    /// user exists).
    #[arg(long)]
    sync_examples_owner: Option<String>,
  },
  /// Makes the site's examples (editor/examples.json) public sketches owned
  /// by a user, creating or updating them.
  SyncExamples {
    /// The user who owns the examples.
    #[arg(long, default_value = "ella")]
    owner: String,
    #[arg(long, default_value = "./data/easl-site.db")]
    db: PathBuf,
    #[arg(long, default_value = "../site")]
    site_dir: PathBuf,
  },
  /// Adds demo users (ella, alice, bob, carol), the examples, sketches,
  /// likes, comments, and playlists (for development).
  Seed {
    #[arg(long, default_value = "./data/easl-site.db")]
    db: PathBuf,
    #[arg(long, default_value = "../site")]
    site_dir: PathBuf,
  },
  /// Grants or revokes admin rights.
  Admin {
    #[command(subcommand)]
    action: AdminAction,
    #[arg(long, default_value = "./data/easl-site.db", global = true)]
    db: PathBuf,
  },
}

#[derive(Subcommand)]
enum AdminAction {
  /// Makes a user an admin.
  Grant { username: String },
  /// Takes a user's admin rights away.
  Revoke { username: String },
}

/// How often expired sessions and email tokens are deleted.
const CLEANUP_INTERVAL: Duration = Duration::from_secs(60 * 60);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
  tracing_subscriber::fmt()
    .with_env_filter(
      EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,tower_http=info,sqlx=warn")),
    )
    .init();
  match Cli::parse().command {
    Command::Serve {
      bind,
      site_dir,
      db,
      editor_origin,
      docs_origin,
      dev,
      trust_proxy,
      sync_examples_owner,
    } => {
      let port = bind.port();
      let editor_origin = editor_origin
        .unwrap_or_else(|| format!("http://editor.localhost:{port}"));
      let docs_origin =
        docs_origin.unwrap_or_else(|| format!("http://localhost:{port}"));
      let mut config = Config::new(site_dir, &editor_origin, &docs_origin, dev);
      config.trust_proxy = trust_proxy;
      serve(bind, &db, config, sync_examples_owner.as_deref()).await
    }
    Command::SyncExamples {
      owner,
      db,
      site_dir,
    } => sync_examples(&db, &site_dir, &owner).await,
    Command::Seed { db, site_dir } => seed::run(&db, &site_dir).await,
    Command::Admin { action, db } => admin(&db, action).await,
  }
}

/// The mailer from `SMTP_URL` and `MAIL_FROM`, or the log when they're
/// unset.
fn mailer(dev: bool) -> anyhow::Result<Mailer> {
  match (env::var("SMTP_URL"), env::var("MAIL_FROM")) {
    (Ok(url), Ok(from)) => Mailer::smtp(&url, &from),
    (Ok(_), Err(_)) => bail!("SMTP_URL is set but MAIL_FROM isn't"),
    _ => {
      if !dev {
        tracing::warn!(
          "SMTP_URL and MAIL_FROM aren't set, so emails are only logged"
        );
      }
      Ok(Mailer::Log)
    }
  }
}

async fn serve(
  bind: SocketAddr,
  db: &Path,
  config: Config,
  sync_examples_owner: Option<&str>,
) -> anyhow::Result<()> {
  if !config.site_dir.is_dir() {
    tracing::warn!(
      "the site directory {} doesn't exist; build the site with the \
       generator first",
      config.site_dir.display()
    );
  }
  let pool = db::open(db).await?;
  if let Some(owner) = sync_examples_owner {
    sync_at_startup(&pool, &config.site_dir, owner).await;
  }
  let state = AppState::new(config.clone(), pool.clone(), mailer(config.dev)?)?;
  tokio::spawn(async move {
    let mut interval = time::interval(CLEANUP_INTERVAL);
    loop {
      interval.tick().await;
      if let Err(error) = auth::delete_expired(&pool).await {
        tracing::error!("deleting expired sessions: {error}");
      }
    }
  });
  let listener = TcpListener::bind(bind)
    .await
    .with_context(|| format!("listening on {bind}"))?;
  tracing::info!(
    "serving {} on http://{bind}: the app as {}, the docs as {}{}",
    config.site_dir.display(),
    config.editor_origin,
    config.docs_origin,
    if config.dev { " (dev)" } else { "" }
  );
  axum::serve(
    listener,
    app::router(state).into_make_service_with_connect_info::<SocketAddr>(),
  )
  .with_graceful_shutdown(shutdown_signal())
  .await?;
  Ok(())
}

/// Syncs the examples for `serve`, logging problems rather than failing:
/// the server runs either way.
async fn sync_at_startup(db: &SqlitePool, site_dir: &Path, owner: &str) {
  match users::by_username(db, owner).await {
    Ok(Some(_)) => {}
    Ok(None) => {
      tracing::warn!("not syncing the examples: there's no user {owner:?}");
      return;
    }
    Err(error) => {
      tracing::error!("not syncing the examples: {error}");
      return;
    }
  }
  let result = match examples::load(site_dir) {
    Ok(Some(examples)) => examples::sync(db, owner, &examples).await,
    Ok(None) => {
      tracing::warn!(
        "not syncing the examples: the site has no editor/examples.json"
      );
      return;
    }
    Err(error) => Err(error),
  };
  match result {
    Ok(report) => tracing::info!("synced the examples to {owner}: {report}"),
    Err(error) => tracing::error!("syncing the examples: {error:#}"),
  }
}

/// `sync-examples`: syncs the site's examples to `owner`'s sketches.
async fn sync_examples(
  db: &Path,
  site_dir: &Path,
  owner: &str,
) -> anyhow::Result<()> {
  let Some(examples) = examples::load(site_dir)? else {
    println!(
      "{} doesn't exist; there's nothing to sync.",
      site::examples_path(site_dir).display()
    );
    return Ok(());
  };
  let pool = db::open(db).await?;
  let report = examples::sync(&pool, owner, &examples).await?;
  println!("Synced {} examples to {owner}: {report}.", examples.len());
  Ok(())
}

/// Resolves on Ctrl-C or SIGTERM.
async fn shutdown_signal() {
  let ctrl_c = async {
    let _ = signal::ctrl_c().await;
  };
  #[cfg(unix)]
  let terminate = async {
    match unix::signal(SignalKind::terminate()) {
      Ok(mut stream) => {
        stream.recv().await;
      }
      Err(_) => future::pending::<()>().await,
    }
  };
  #[cfg(not(unix))]
  let terminate = future::pending::<()>();
  tokio::select! {
    _ = ctrl_c => {}
    _ = terminate => {}
  }
  tracing::info!("shutting down");
}

async fn admin(db: &Path, action: AdminAction) -> anyhow::Result<()> {
  let pool = db::open(db).await?;
  let (username, grant) = match &action {
    AdminAction::Grant { username } => (username, true),
    AdminAction::Revoke { username } => (username, false),
  };
  let Some(changed) = moderation::set_admin(&pool, username, grant).await?
  else {
    bail!("there's no user named {username:?}");
  };
  let state = match (changed, grant) {
    (true, true) => "is now",
    (true, false) => "is no longer",
    (false, true) => "was already",
    (false, false) => "wasn't",
  };
  println!("{username} {state} an admin.");
  Ok(())
}
