//! `/admin`: the moderation queue.

use axum::extract::State;
use minijinja::context;

use super::{PageContext, render_page};
use crate::auth::PageUser;
use crate::error::{PageError, PageResult};
use crate::reports;
use crate::state::AppState;

/// `GET /admin` (admins only; a 404 to anyone else): open reports, with
/// links to their targets and moderation actions.
pub async fn admin(
  State(state): State<AppState>,
  PageUser(user): PageUser,
  page: PageContext,
) -> PageResult {
  if !user.is_admin {
    return Err(PageError::not_found());
  }
  let reports = reports::open(&state.db).await?;
  render_page(&state, &page, "admin.html", context! { reports })
}
