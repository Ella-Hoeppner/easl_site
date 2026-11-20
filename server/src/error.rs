//! Error types for the JSON API and the HTML pages.
//!
//! [`AppError`] renders as `{"error": "<message>"}` with its status.
//! [`PageError`] wraps one for page handlers: its response carries an
//! [`ErrorPage`] marker that the page middleware
//! ([`crate::pages::render_error_pages`]) turns into an HTML error page, or
//! into a redirect to the sign-in page when the visitor isn't signed in.

use std::error::Error;

use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// A request failure with a status and a human-readable message.
#[derive(Debug)]
pub enum AppError {
  BadRequest(String),
  Unauthorized,
  Forbidden(String),
  NotFound,
  Conflict(String),
  TooLarge(String),
  RateLimited,
  /// Something the site can't do right now (sending email past its daily
  /// budget).
  Unavailable(String),
  /// The disk is too full to store sketch content ([`crate::disk`]).
  OutOfStorage,
  Internal(anyhow::Error),
}

impl AppError {
  pub fn bad_request(message: impl Into<String>) -> Self {
    AppError::BadRequest(message.into())
  }

  pub fn forbidden(message: impl Into<String>) -> Self {
    AppError::Forbidden(message.into())
  }

  pub fn conflict(message: impl Into<String>) -> Self {
    AppError::Conflict(message.into())
  }

  /// An internal error from any error type.
  pub fn from_error(error: impl Error + Send + Sync + 'static) -> Self {
    AppError::Internal(error.into())
  }

  /// The error for an email the site can't send right now.
  pub fn email_unavailable() -> Self {
    AppError::Unavailable(
      "We can't send email right now. Please try again later.".into(),
    )
  }

  /// The error for actions that need a verified email address.
  pub fn unverified(action: &str) -> Self {
    AppError::Forbidden(format!(
      "Verify your email address to {action}. You can resend the \
       verification email from your settings."
    ))
  }

  pub fn status(&self) -> StatusCode {
    match self {
      AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
      AppError::Unauthorized => StatusCode::UNAUTHORIZED,
      AppError::Forbidden(_) => StatusCode::FORBIDDEN,
      AppError::NotFound => StatusCode::NOT_FOUND,
      AppError::Conflict(_) => StatusCode::CONFLICT,
      AppError::TooLarge(_) => StatusCode::PAYLOAD_TOO_LARGE,
      AppError::RateLimited => StatusCode::TOO_MANY_REQUESTS,
      AppError::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
      AppError::OutOfStorage => StatusCode::INSUFFICIENT_STORAGE,
      AppError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
  }

  /// The message shown to the client. Internal errors are logged, never
  /// shown.
  pub fn message(&self) -> String {
    match self {
      AppError::BadRequest(m)
      | AppError::Forbidden(m)
      | AppError::Conflict(m)
      | AppError::TooLarge(m)
      | AppError::Unavailable(m) => m.clone(),
      AppError::Unauthorized => "You need to sign in to do that.".into(),
      AppError::NotFound => "Not found.".into(),
      AppError::RateLimited => {
        "Too many requests. Please wait a while and try again.".into()
      }
      AppError::OutOfStorage => "The site is out of storage right now, so \
        it can't save sketches or thumbnails. Your work hasn't been saved; \
        please try again later."
        .into(),
      AppError::Internal(_) => {
        "Something went wrong on our end. Please try again.".into()
      }
    }
  }

  /// The error as an [`anyhow::Error`], for command-line tools.
  pub fn into_anyhow(self) -> anyhow::Error {
    match self {
      AppError::Internal(error) => error,
      other => anyhow::Error::msg(other.message()),
    }
  }

  fn log(&self) {
    if let AppError::Internal(error) = self {
      tracing::error!("internal error: {error:#}");
    }
  }
}

impl From<sqlx::Error> for AppError {
  fn from(error: sqlx::Error) -> Self {
    AppError::Internal(error.into())
  }
}

impl From<anyhow::Error> for AppError {
  fn from(error: anyhow::Error) -> Self {
    AppError::Internal(error)
  }
}

impl From<JsonRejection> for AppError {
  fn from(rejection: JsonRejection) -> Self {
    match rejection.status() {
      StatusCode::PAYLOAD_TOO_LARGE => {
        AppError::TooLarge("The request body is too large.".into())
      }
      _ => AppError::BadRequest(rejection.body_text()),
    }
  }
}

impl IntoResponse for AppError {
  fn into_response(self) -> Response {
    self.log();
    (self.status(), Json(json!({ "error": self.message() }))).into_response()
  }
}

/// A failed page request; see the module documentation.
#[derive(Debug)]
pub struct PageError(pub AppError);

impl PageError {
  pub fn not_found() -> Self {
    PageError(AppError::NotFound)
  }
}

impl<E: Into<AppError>> From<E> for PageError {
  fn from(error: E) -> Self {
    PageError(error.into())
  }
}

/// The marker a [`PageError`] response carries for the page middleware.
#[derive(Clone, Debug)]
pub struct ErrorPage {
  pub status: StatusCode,
  pub message: String,
}

impl IntoResponse for PageError {
  fn into_response(self) -> Response {
    self.0.log();
    let marker = ErrorPage {
      status: self.0.status(),
      message: self.0.message(),
    };
    let mut response = (marker.status, marker.message.clone()).into_response();
    response.extensions_mut().insert(marker);
    response
  }
}

/// The result type of API handlers.
pub type ApiResult<T> = Result<T, AppError>;

/// The result type of page handlers.
pub type PageResult = Result<Response, PageError>;
