//! The `easl-site-server` command: `serve`, `seed`, and `admin`.

use std::env;
use std::future;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, bail};
use clap::{Parser, Subcommand};
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
use easl_site_server::mail::Mailer;
use easl_site_server::seed;
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
    /// The public origin, e.g. https://easl-lang.org (default:
    /// http://<bind>).
    #[arg(long)]
    origin: Option<String>,
    /// Development mode: dev origins accepted, cookies not Secure, templates
    /// reloaded, emails logged and their links shown on the page.
    #[arg(long)]
    dev: bool,
    /// Take client addresses from X-Forwarded-For's last hop (set this
    /// behind a reverse proxy, and only then).
    #[arg(long)]
    trust_proxy: bool,
  },
  /// Fills an empty database with demo users, sketches, and more (for
  /// development).
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
      origin,
      dev,
      trust_proxy,
    } => {
      let origin = origin.unwrap_or_else(|| format!("http://{bind}"));
      let mut config = Config::new(site_dir, &origin, dev);
      config.trust_proxy = trust_proxy;
      serve(bind, &db, config).await
    }
    Command::Seed { db, site_dir } => seed::run(&db, site_dir).await,
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
) -> anyhow::Result<()> {
  if !config.site_dir.is_dir() {
    tracing::warn!(
      "the site directory {} doesn't exist; build the site with the \
       generator first",
      config.site_dir.display()
    );
  }
  let pool = db::open(db).await?;
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
    "serving {} on http://{bind} as {}{}",
    config.site_dir.display(),
    config.origin,
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
  if !users::set_admin(&pool, username, grant).await? {
    bail!("there's no user named {username:?}");
  }
  println!(
    "{username} {} an admin.",
    if grant { "is now" } else { "is no longer" }
  );
  Ok(())
}
