//! `/settings`: the signed-in user's profile, email, password, and account.
//!
//! Each section posts to its own route. Success redirects back with
//! `?saved=<section>`; a failure re-renders the page with the error shown in
//! that section.

use axum::Form;
use axum::extract::{Query, State};
use axum::http::header::SET_COOKIE;
use minijinja::value::merge_maps;
use minijinja::{Value, context};
use serde::Deserialize;

use super::account::send_verification;
use super::{PageContext, render_page, render_status, see_other};
use crate::auth::{
  self, CurrentUser, PageUser, clear_session_cookie, delete_other_sessions,
};
use crate::error::{AppError, PageResult};
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
/// then needs verifying again.
pub async fn email(
  State(state): State<AppState>,
  PageUser(user): PageUser,
  page: PageContext,
  Form(form): Form<EmailForm>,
) -> PageResult {
  let result = async {
    let email = validate::email(&form.email)?;
    if !auth::verify_password(&state, Some(&user.password_hash), &form.password)
      .await?
    {
      return Err(AppError::forbidden("That password isn't right."));
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
/// other sessions.
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
    if !auth::verify_password(&state, Some(&user.password_hash), &form.current)
      .await?
    {
      return Err(AppError::forbidden("The current password isn't right."));
    }
    let hash = auth::hash_password(&state, password).await?;
    users::set_password_hash(&state.db, user.id, &hash).await?;
    delete_other_sessions(&state.db, user.id, Some(&user.session_hash)).await?;
    Ok(())
  }
  .await;
  match result {
    Ok(()) => Ok(see_other("/settings?saved=password")),
    Err(error) => failed(&state, &page, &user, "password", error, context! {}),
  }
}

/// `POST /settings/verification`: sends a new verification email.
pub async fn resend_verification(
  State(state): State<AppState>,
  PageUser(user): PageUser,
  page: PageContext,
) -> PageResult {
  if user.email_verified() {
    return Ok(see_other("/settings"));
  }
  if let Err(error) = state.limiter.check(
    Action::VerificationEmail,
    &user.id.to_string(),
    state.config.limits.verification_emails,
  ) {
    return failed(&state, &page, &user, "email", error, context! {});
  }
  let link = send_verification(&state, &user).await?;
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
    if !auth::verify_password(&state, Some(&user.password_hash), &form.password)
      .await?
    {
      return Err(AppError::forbidden("That password isn't right."));
    }
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
