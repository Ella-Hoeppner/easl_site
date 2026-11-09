//! `POST /api/reports`: flagging content for moderators.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

use super::ApiJson;
use crate::auth::{ApiUser, Viewer};
use crate::comments;
use crate::error::{ApiResult, AppError};
use crate::ratelimit::Action;
use crate::reports::{self, ReportKind};
use crate::sketches;
use crate::state::AppState;
use crate::users;
use crate::validate;

#[derive(Deserialize)]
pub struct NewReport {
  kind: ReportKind,
  target: String,
  #[serde(default)]
  reason: String,
}

/// `POST /api/reports` `{kind, target, reason}`: `{"id": …}` (201). The
/// target must exist and be visible to the reporter (a sketch id, a comment
/// id, or a username). Needs a verified email address.
pub async fn create(
  State(state): State<AppState>,
  ApiUser(user): ApiUser,
  ApiJson(report): ApiJson<NewReport>,
) -> ApiResult<(StatusCode, Json<Value>)> {
  user.require_verified("report content")?;
  let viewer = Viewer::of(Some(&user));
  let reason = validate::report_reason(&report.reason)?;
  let target = report.target.trim();
  match report.kind {
    ReportKind::Sketch => {
      sketches::find_visible(&state.db, target, &viewer).await?;
    }
    ReportKind::Comment => {
      let id: i64 = target.parse().map_err(|_| AppError::NotFound)?;
      let comment = comments::find(&state.db, id)
        .await?
        .filter(|c| c.deleted_at.is_none())
        .ok_or(AppError::NotFound)?;
      sketches::find_visible(&state.db, &comment.sketch_id, &viewer).await?;
    }
    ReportKind::User => {
      users::by_username(&state.db, target)
        .await?
        .ok_or(AppError::NotFound)?;
    }
  }
  state.limiter.check(
    Action::Report,
    &user.id.to_string(),
    state.config.limits.reports,
  )?;
  let id =
    reports::create(&state.db, user.id, report.kind, target, &reason).await?;
  Ok((StatusCode::CREATED, Json(json!({ "id": id }))))
}
