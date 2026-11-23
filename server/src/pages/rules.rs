//! `/rules`: the editor's rules and privacy policy, one page linked from
//! the app's header and the editor's bar.

use axum::extract::State;
use minijinja::context;

use super::{PageContext, render_page};
use crate::error::PageResult;
use crate::state::AppState;

pub async fn rules(
  State(state): State<AppState>,
  page: PageContext,
) -> PageResult {
  render_page(&state, &page, "rules.html", context! {})
}
