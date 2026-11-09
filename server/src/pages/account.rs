//! Signing up, in, and out; email verification; password resets.

use axum::Form;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::http::header::SET_COOKIE;
use axum::response::Response;
use minijinja::context;
use serde::Deserialize;

use super::{PageContext, render_page, render_status, see_other};
use crate::auth::{
  self, CurrentUser, TokenPurpose, clear_session_cookie, consume_email_token,
  create_email_token, create_session, delete_email_tokens,
  delete_other_sessions, peek_email_token, session_cookie,
};
use crate::error::{AppError, PageError, PageResult};
use crate::mail::{reset_email, verification_email};
use crate::ratelimit::Action;
use crate::security::ClientIp;
use crate::state::AppState;
use crate::tokens::hash_token;
use crate::users::{self, User};
use crate::validate;

/// A response that sets the session cookie and goes to `location`.
fn signed_in_redirect(
  state: &AppState,
  token: &str,
  location: &str,
) -> Response {
  let mut response = see_other(location);
  response
    .headers_mut()
    .append(SET_COOKIE, session_cookie(&state.config, token));
  response
}

fn check_auth_limit(state: &AppState, ip: &ClientIp) -> Result<(), AppError> {
  state
    .limiter
    .check(Action::Auth, &ip.0, state.config.limits.auth)
}

/// Sends a verification email for the user's current address, returning
/// the link (shown on the page in dev).
pub async fn send_verification(
  state: &AppState,
  user: &User,
) -> Result<String, AppError> {
  let token =
    create_email_token(&state.db, user.id, TokenPurpose::Verify).await?;
  let link = state.config.editor_url(&format!("/verify/{token}"));
  state
    .mailer
    .send(verification_email(&user.email, &user.username, &link));
  Ok(link)
}

#[derive(Deserialize)]
pub struct NextQuery {
  next: Option<String>,
}

/// `GET /login`.
pub async fn login_form(
  State(state): State<AppState>,
  page: PageContext,
  Query(query): Query<NextQuery>,
) -> PageResult {
  let next = validate::next_path(query.next.as_deref());
  if page.user.is_some() {
    return Ok(see_other(&next));
  }
  render_page(&state, &page, "login.html", context! { next })
}

#[derive(Deserialize)]
pub struct LoginForm {
  #[serde(default)]
  login: String,
  #[serde(default)]
  password: String,
  next: Option<String>,
}

/// `POST /login`: signs in with a username or email and a password. The
/// error doesn't say whether the account exists.
pub async fn login(
  State(state): State<AppState>,
  page: PageContext,
  ip: ClientIp,
  Form(form): Form<LoginForm>,
) -> PageResult {
  let next = validate::next_path(form.next.as_deref());
  let fail = |status: StatusCode, message: &str| {
    render_status(
      &state,
      &page,
      status,
      "login.html",
      context! { next => &next, login => &form.login, error => message },
    )
  };
  if let Err(error) = check_auth_limit(&state, &ip) {
    return fail(error.status(), &error.message());
  }
  let user = users::by_login(&state.db, &form.login).await?;
  let hash = user.as_ref().map(|u| u.password_hash.as_str());
  if !auth::verify_password(&state, hash, &form.password).await? {
    return fail(
      StatusCode::UNAUTHORIZED,
      "That username or email and password don't match an account.",
    );
  }
  let Some(user) = user else {
    return Err(PageError(AppError::Unauthorized));
  };
  if user.banned_at.is_some() {
    return fail(StatusCode::FORBIDDEN, "This account has been suspended.");
  }
  let token = create_session(&state.db, user.id).await?;
  Ok(signed_in_redirect(&state, &token, &next))
}

/// `GET /signup`.
pub async fn signup_form(
  State(state): State<AppState>,
  page: PageContext,
  Query(query): Query<NextQuery>,
) -> PageResult {
  let next = validate::next_path(query.next.as_deref());
  if page.user.is_some() {
    return Ok(see_other(&next));
  }
  render_page(&state, &page, "signup.html", context! { next })
}

#[derive(Deserialize)]
pub struct SignupForm {
  #[serde(default)]
  username: String,
  #[serde(default)]
  email: String,
  #[serde(default)]
  password: String,
  next: Option<String>,
}

/// `POST /signup`: creates the account, signs in, and sends the
/// verification email.
pub async fn signup(
  State(state): State<AppState>,
  page: PageContext,
  ip: ClientIp,
  Form(form): Form<SignupForm>,
) -> PageResult {
  let next = validate::next_path(form.next.as_deref());
  match create_account(&state, &ip, &form).await {
    Ok((user, token, link)) => {
      let signed_in = PageContext {
        user: Some(CurrentUser {
          user: user.clone(),
          session_hash: hash_token(&token),
        }),
        path: page.path.clone(),
      };
      let mut response = render_page(
        &state,
        &signed_in,
        "check_email.html",
        context! {
          hide_verify_banner => true,
          purpose => "verify",
          email => &user.email,
          next => &next,
          dev_link => state.config.dev.then_some(link),
        },
      )?;
      response
        .headers_mut()
        .append(SET_COOKIE, session_cookie(&state.config, &token));
      Ok(response)
    }
    Err(error) => render_status(
      &state,
      &page,
      error.status(),
      "signup.html",
      context! {
        next,
        username => &form.username,
        email => &form.email,
        error => error.message(),
      },
    ),
  }
}

/// Validates a sign-up, creates the user and a session, and sends the
/// verification email. Returns the user, session token, and link.
async fn create_account(
  state: &AppState,
  ip: &ClientIp,
  form: &SignupForm,
) -> Result<(User, String, String), AppError> {
  check_auth_limit(state, ip)?;
  let username = validate::username(&form.username)?;
  let email = validate::email(&form.email)?;
  let password = validate::password(&form.password)?;
  let hash = auth::hash_password(state, password).await?;
  let user = users::create(&state.db, &username, &email, &hash).await?;
  let token = create_session(&state.db, user.id).await?;
  let link = send_verification(state, &user).await?;
  Ok((user, token, link))
}

/// `POST /logout`.
pub async fn logout(
  State(state): State<AppState>,
  page: PageContext,
) -> PageResult {
  if let Some(user) = &page.user {
    auth::delete_session(&state.db, &user.session_hash).await?;
  }
  let mut response = see_other("/");
  response
    .headers_mut()
    .append(SET_COOKIE, clear_session_cookie(&state.config));
  Ok(response)
}

/// `GET /verify/{token}`: marks the address verified.
pub async fn verify(
  State(state): State<AppState>,
  page: PageContext,
  Path(token): Path<String>,
) -> PageResult {
  let user_id =
    consume_email_token(&state.db, &token, TokenPurpose::Verify).await?;
  let Some(user_id) = user_id else {
    return render_status(
      &state,
      &page,
      StatusCode::NOT_FOUND,
      "message.html",
      context! {
        title => "Link expired",
        message => "This verification link has expired or was already \
          used. You can send a new one from your settings.",
        link => "/settings",
        link_text => "Go to settings",
      },
    );
  };
  users::mark_verified(&state.db, user_id).await?;
  render_page(
    &state,
    &page,
    "message.html",
    context! {
      title => "Email verified",
      message => "Thanks! Your email address is verified, so you can now \
        publish sketches and playlists, comment, and report.",
      link => "/",
      link_text => "Back to easl",
      // The page's visitor was read before the link verified them.
      hide_verify_banner => true,
    },
  )
}

/// `GET /forgot`.
pub async fn forgot_form(
  State(state): State<AppState>,
  page: PageContext,
) -> PageResult {
  render_page(&state, &page, "forgot.html", context! {})
}

#[derive(Deserialize)]
pub struct ForgotForm {
  #[serde(default)]
  email: String,
}

/// `POST /forgot`: emails a reset link if an account has the address; the
/// response is the same either way.
pub async fn forgot(
  State(state): State<AppState>,
  page: PageContext,
  ip: ClientIp,
  Form(form): Form<ForgotForm>,
) -> PageResult {
  if let Err(error) = check_auth_limit(&state, &ip) {
    return render_status(
      &state,
      &page,
      error.status(),
      "forgot.html",
      context! { email => &form.email, error => error.message() },
    );
  }
  let mut link = None;
  if let Some(user) = users::by_email(&state.db, form.email.trim()).await?
    && user.banned_at.is_none()
  {
    let token =
      create_email_token(&state.db, user.id, TokenPurpose::Reset).await?;
    let url = state.config.editor_url(&format!("/reset/{token}"));
    state
      .mailer
      .send(reset_email(&user.email, &user.username, &url));
    link = Some(url);
  }
  render_page(
    &state,
    &page,
    "check_email.html",
    context! {
      hide_verify_banner => true,
      purpose => "reset",
      email => form.email.trim(),
      dev_link => if state.config.dev { link } else { None },
    },
  )
}

fn expired_reset(state: &AppState, page: &PageContext) -> PageResult {
  render_status(
    state,
    page,
    StatusCode::NOT_FOUND,
    "message.html",
    context! {
      title => "Link expired",
      message => "This password reset link has expired or was already \
        used. You can ask for a new one.",
      link => "/forgot",
      link_text => "Reset your password",
    },
  )
}

/// `GET /reset/{token}`: the new-password form.
pub async fn reset_form(
  State(state): State<AppState>,
  page: PageContext,
  Path(token): Path<String>,
) -> PageResult {
  if peek_email_token(&state.db, &token, TokenPurpose::Reset)
    .await?
    .is_none()
  {
    return expired_reset(&state, &page);
  }
  render_page(&state, &page, "reset.html", context! { token })
}

#[derive(Deserialize)]
pub struct ResetForm {
  #[serde(default)]
  password: String,
  #[serde(default)]
  confirm: String,
}

/// `POST /reset/{token}`: sets the new password, ends every session, and
/// signs in here. Following the link also proves the email address, so it
/// counts as verified.
pub async fn reset(
  State(state): State<AppState>,
  page: PageContext,
  ip: ClientIp,
  Path(token): Path<String>,
  Form(form): Form<ResetForm>,
) -> PageResult {
  let fail = |error: AppError| {
    render_status(
      &state,
      &page,
      error.status(),
      "reset.html",
      context! { token => &token, error => error.message() },
    )
  };
  if let Err(error) = check_auth_limit(&state, &ip) {
    return fail(error);
  }
  let password = match validate::password(&form.password) {
    Ok(password) => password,
    Err(error) => return fail(error),
  };
  if form.password != form.confirm {
    return fail(AppError::bad_request("The passwords don't match."));
  }
  let hash = auth::hash_password(&state, password).await?;
  let mut tx = state.db.begin().await?;
  let Some(user_id) =
    consume_email_token(&mut *tx, &token, TokenPurpose::Reset).await?
  else {
    return expired_reset(&state, &page);
  };
  users::set_password_hash(&mut *tx, user_id, &hash).await?;
  users::mark_verified(&mut *tx, user_id).await?;
  delete_other_sessions(&mut *tx, user_id, None).await?;
  delete_email_tokens(&mut *tx, user_id, TokenPurpose::Reset).await?;
  let session = create_session(&mut *tx, user_id).await?;
  tx.commit().await?;
  Ok(signed_in_redirect(
    &state,
    &session,
    "/settings?saved=password",
  ))
}
