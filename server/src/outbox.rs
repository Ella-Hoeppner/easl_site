//! Sending the site's emails within its daily budget.
//!
//! The site's mail provider (Resend's free tier) sends 100 emails a day, so
//! the site sends at most [`Config::daily_email_budget`] a day (UTC days),
//! as a backstop behind the per-address and per-account rate limits. Every
//! email goes through [`send`], which counts it in the database
//! (`email_days`), so a restart doesn't reset the count, and hands it to
//! the mailer; once today's budget is spent it logs a warning and sends
//! nothing.
//!
//! [`Config::daily_email_budget`]: crate::config::Config::daily_email_budget

use sqlx::SqlitePool;

use crate::db::now;
use crate::error::AppError;
use crate::mail::Email;
use crate::state::AppState;

/// One day, in seconds.
const DAY_SECONDS: i64 = 24 * 60 * 60;

/// How many days of counts are kept (for looking back at in the database).
const KEPT_DAYS: i64 = 30;

/// Today's number, counting UTC days from the unix epoch.
fn today() -> i64 {
  now().div_euclid(DAY_SECONDS)
}

/// How many emails the site has sent today.
pub async fn sent_today(db: &SqlitePool) -> sqlx::Result<i64> {
  let sent: Option<i64> =
    sqlx::query_scalar("SELECT sent FROM email_days WHERE day = ?")
      .bind(today())
      .fetch_optional(db)
      .await?;
  Ok(sent.unwrap_or(0))
}

/// Whether today's budget has room for another email, without using it.
pub async fn has_room(state: &AppState) -> sqlx::Result<bool> {
  Ok(sent_today(&state.db).await? < state.config.daily_email_budget)
}

/// Counts one email against today's budget if it has room, returning
/// whether it did. Concurrent calls never overspend it.
async fn reserve(db: &SqlitePool, budget: i64) -> sqlx::Result<bool> {
  if budget <= 0 {
    return Ok(false);
  }
  let counted: Option<i64> = sqlx::query_scalar(
    "INSERT INTO email_days (day, sent) VALUES (?, 1) \
     ON CONFLICT (day) DO UPDATE SET sent = sent + 1 WHERE sent < ? \
     RETURNING sent",
  )
  .bind(today())
  .bind(budget)
  .fetch_optional(db)
  .await?;
  Ok(counted.is_some())
}

/// Sends an email if today's budget has room, counting it; otherwise logs
/// a warning and fails with [`AppError::email_unavailable`].
pub async fn send(state: &AppState, email: Email) -> Result<(), AppError> {
  if !reserve(&state.db, state.config.daily_email_budget).await? {
    tracing::warn!(
      "not sending \"{}\": today's email budget ({}) is spent",
      email.subject,
      state.config.daily_email_budget
    );
    return Err(AppError::email_unavailable());
  }
  state.mailer.send(email);
  Ok(())
}

/// Deletes the counts of days long past.
pub async fn delete_old_counts(db: &SqlitePool) -> sqlx::Result<()> {
  sqlx::query("DELETE FROM email_days WHERE day < ?")
    .bind(today() - KEPT_DAYS)
    .execute(db)
    .await?;
  Ok(())
}
