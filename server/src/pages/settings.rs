//! `/settings`: the signed-in user's profile, email, password, and account.
//!
//! Each section posts to its own route. Success redirects back with
//! `?saved=<section>`; a failure re-renders the page with the error shown in
//! that section.
//!
//! Checks of the current password are rate-limited per account (before
//! hashing), and verification emails (re-sent, or for a changed address)
//! per account, besides the site's daily email budget.

use axum::Form;
use axum::extract::{Query, State};
use axum::http::header::SET_COOKIE;
use minijinja::value::merge_maps;
use minijinja::{Value, context};
use serde::Deserialize;

use super::account::send_verification;
use super::{PageContext, render_page, render_status, see_other};
use crate::auth::{
  self, CurrentUser, PageUser, TokenPurpose, clear_session_cookie,
  delete_email_tokens, delete_other_sessions,
};
use crate::error::{AppError, PageResult};
use crate::outbox;
use crate::ratelimit::Action;
use crate::state::AppState;
use crate::users;
use crate::validate;

#[derive(Deserialize)]
pub struct SettingsQuery {
  saved: Option<String>,
}

/// The page's context: the stored profile fields and email.
fn settings_context(user: &CurrentUser) -> Value {
  context! {
    hide_verify_banner => true,
    display_name => &user.display_name,
    bio => &user.bio,
    email => &user.email,
    email_verified => user.email_verified(),
  }
}

/// `GET /settings`.
pub async fn show(
  State(state): State<AppState>,
  PageUser(user): PageUser,
  page: PageContext,
  Query(query): Query<SettingsQuery>,
) -> PageResult {
  let saved = query
    .saved
    .filter(|s| ["profile", "email", "password"].contains(&s.as_str()));
  render_page(
    &state,
    &page,
    "settings.html",
    context! { saved, ..settings_context(&user) },
  )
}

/// Re-renders the page with an error in `section`, keeping `values` (the
/// submitted non-secret fields, which take the place of the stored ones) in
/// the form.
fn failed(
  state: &AppState,
  page: &PageContext,
  user: &CurrentUser,
  section: &str,
  error: AppError,
  values: Value,
) -> PageResult {
  render_status(
    state,
    page,
    error.status(),
    "settings.html",
    merge_maps([
      settings_context(user),
      values,
      context! { error_section => section, error => error.message() },
    ]),
  )
}

/// Fails unless `password` is the user's current password, with `wrong`
/// as the message when it isn't. Limited per account, checked before
/// hashing.
async fn check_password(
  state: &AppState,
  user: &CurrentUser,
  password: &str,
  wrong: &str,
) -> Result<(), AppError> {
  state.limiter.check(
    Action::PasswordCheck,
    &user.id.to_string(),
    state.config.limits.password_checks,
  )?;
  if auth::verify_password(state, Some(&user.password_hash), password).await? {
    Ok(())
  } else {
    Err(AppError::forbidden(wrong))
  }
}

/// Fails when the account has had too many verification emails lately.
fn check_verification_limit(
  state: &AppState,
  user: &CurrentUser,
) -> Result<(), AppError> {
  state.limiter.check_all(
    Action::VerificationEmail,
    &user.id.to_string(),
    &state.config.limits.verification_emails,
  )
}

#[derive(Deserialize)]
pub struct ProfileForm {
  #[serde(default)]
  display_name: String,
  #[serde(default)]
  bio: String,
}

/// `POST /settings/profile`.
pub async fn profile(
  State(state): State<AppState>,
  PageUser(user): PageUser,
  page: PageContext,
  Form(form): Form<ProfileForm>,
) -> PageResult {
  let result = async {
    let display_name = validate::display_name(&form.display_name)?;
    let bio = validate::bio(&form.bio)?;
    users::update_profile(&state.db, user.id, &display_name, &bio).await?;
    Ok::<_, AppError>(())
  }
  .await;
  match result {
    Ok(()) => Ok(see_other("/settings?saved=profile")),
    Err(error) => failed(
      &state,
      &page,
      &user,
      "profile",
      error,
      context! { display_name => &form.display_name, bio => &form.bio },
    ),
  }
}

#[derive(Deserialize)]
pub struct EmailForm {
  #[serde(default)]
  email: String,
  #[serde(default)]
  password: String,
}

/// `POST /settings/email`: changes the address (needs the password), which
/// then needs verifying again. The verification email counts against the
/// account's limit, and the address stays as it is when no email can be
/// sent.
pub async fn email(
  State(state): State<AppState>,
  PageUser(user): PageUser,
  page: PageContext,
  Form(form): Form<EmailForm>,
) -> PageResult {
  let result = async {
    let email = validate::email(&form.email)?;
    check_password(&state, &user, &form.password, "That password isn't right.")
      .await?;
    check_verification_limit(&state, &user)?;
    if !outbox::has_room(&state).await? {
      return Err(AppError::email_unavailable());
    }
    users::change_email(&state.db, user.id, &email).await?;
    let updated = users::by_id(&state.db, user.id)
      .await?
      .ok_or(AppError::NotFound)?;
    send_verification(&state, &updated).await
  }
  .await;
  match result {
    Ok(_) => Ok(see_other("/settings?saved=email")),
    Err(error) => failed(
      &state,
      &page,
      &user,
      "email",
      error,
      context! { new_email => &form.email },
    ),
  }
}

#[derive(Deserialize)]
pub struct PasswordForm {
  #[serde(default)]
  current: String,
  #[serde(default)]
  password: String,
  #[serde(default)]
  confirm: String,
}

/// `POST /settings/password`: needs the current password; ends the user's
/// other sessions and invalidates their password-reset links, with the new
/// password in one transaction.
pub async fn password(
  State(state): State<AppState>,
  PageUser(user): PageUser,
  page: PageContext,
  Form(form): Form<PasswordForm>,
) -> PageResult {
  let result = async {
    let password = validate::password(&form.password)?;
    if form.password != form.confirm {
      return Err(AppError::bad_request("The new passwords don't match."));
    }
    check_password(
      &state,
      &user,
      &form.current,
      "The current password isn't right.",
    )
    .await?;
    let hash = auth::hash_password(&state, password).await?;
    let mut tx = state.db.begin().await?;
    users::set_password_hash(&mut *tx, user.id, &hash).await?;
    delete_email_tokens(&mut *tx, user.id, TokenPurpose::Reset).await?;
    delete_other_sessions(&mut *tx, user.id, Some(&user.session_hash)).await?;
    tx.commit().await?;
    Ok(())
  }
  .await;
  match result {
    Ok(()) => Ok(see_other("/settings?saved=password")),
    Err(error) => failed(&state, &page, &user, "password", error, context! {}),
  }
}

/// `POST /settings/verification`: sends a new verification email, within
/// the account's limit and the day's email budget.
pub async fn resend_verification(
  State(state): State<AppState>,
  PageUser(user): PageUser,
  page: PageContext,
) -> PageResult {
  if user.email_verified() {
    return Ok(see_other("/settings"));
  }
  let sent = match check_verification_limit(&state, &user) {
    Ok(()) => send_verification(&state, &user).await,
    Err(error) => Err(error),
  };
  let link = match sent {
    Ok(link) => link,
    Err(error) => {
      return failed(&state, &page, &user, "email", error, context! {});
    }
  };
  render_page(
    &state,
    &page,
    "check_email.html",
    context! {
      hide_verify_banner => true,
      purpose => "verify",
      email => &user.email,
      next => "/settings",
      dev_link => state.config.dev.then_some(link),
    },
  )
}

#[derive(Deserialize)]
pub struct DeleteForm {
  #[serde(default)]
  password: String,
  #[serde(default)]
  confirm: Option<String>,
}

/// `POST /settings/delete`: deletes the account and everything it owns.
/// Needs the password and the confirmation box.
pub async fn delete_account(
  State(state): State<AppState>,
  PageUser(user): PageUser,
  page: PageContext,
  Form(form): Form<DeleteForm>,
) -> PageResult {
  let result = async {
    if form.confirm.as_deref() != Some("yes") {
      return Err(AppError::bad_request(
        "Tick the box to confirm you want to delete your account.",
      ));
    }
    check_password(&state, &user, &form.password, "That password isn't right.")
      .await?;
    users::delete(&state.db, user.id).await?;
    Ok(())
  }
  .await;
  if let Err(error) = result {
    return failed(&state, &page, &user, "delete", error, context! {});
  }
  let signed_out = PageContext {
    user: None,
    path: page.path.clone(),
  };
  let mut response = render_page(
    &state,
    &signed_out,
    "message.html",
    context! {
      title => "Account deleted",
      message => "Your account and everything in it have been deleted.",
      link => "/",
      link_text => "Back to easl",
    },
  )?;
  response
    .headers_mut()
    .append(SET_COOKIE, clear_session_cookie(&state.config));
  Ok(response)
}
