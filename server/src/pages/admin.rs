//! The admin area: `/admin`, the moderation page, with tabs for reports,
//! banned users, hidden sketches, deleted comments, and the moderation log;
//! and `/admin/users/{username}`, one user's moderation record. Both are
//! for admins only (a 404 to anyone else). Usernames on them link to users'
//! moderation pages, which link on to their public profiles.

use axum::extract::{Path, Query, State};
use minijinja::{Value, context};
use serde::{Deserialize, Serialize};

use super::{PageContext, page_links, page_number, render_page, url_with};
use crate::auth::{CurrentUser, PageUser};
use crate::error::{PageError, PageResult};
use crate::moderation;
use crate::reports;
use crate::sketches::Page;
use crate::state::AppState;
use crate::users;
use crate::validate;

#[derive(Deserialize)]
pub struct AdminQuery {
  tab: Option<String>,
  resolved: Option<String>,
  page: Option<String>,
}

/// The tabs, by their `tab` parameter, with their labels.
const TABS: [(&str, &str); 5] = [
  ("reports", "Reports"),
  ("banned", "Banned users"),
  ("hidden", "Hidden sketches"),
  ("comments", "Deleted comments"),
  ("log", "Log"),
];

/// `GET /admin?tab=reports|banned|hidden|comments|log&resolved=1&page=N`
/// (admins only; a 404 to anyone else). The reports tab lists open reports,
/// or with `resolved=1` resolved ones, newest first; the other lists are
/// newest first too. Every list but the open reports is paginated.
pub async fn admin(
  State(state): State<AppState>,
  PageUser(user): PageUser,
  page: PageContext,
  Query(query): Query<AdminQuery>,
) -> PageResult {
  require_admin(&user)?;
  let tab = TABS
    .iter()
    .map(|(key, _)| *key)
    .find(|key| query.tab.as_deref() == Some(*key))
    .unwrap_or("reports");
  let resolved = tab == "reports" && query.resolved.as_deref() == Some("1");
  let number = page_number(query.page.as_deref());
  let counts = moderation::counts(&state.db).await?;
  let tabs: Vec<_> = TABS
    .iter()
    .map(|(key, label)| {
      let count = match *key {
        "reports" => Some(counts.open_reports),
        "banned" => Some(counts.banned_users),
        "hidden" => Some(counts.hidden_sketches),
        "comments" => Some(counts.deleted_comments),
        _ => None,
      };
      context! {
        label,
        count,
        url => tab_url(key, false),
        current => *key == tab,
      }
    })
    .collect();
  let (items, has_more): (Value, bool) = match tab {
    "reports" if resolved => {
      listing(reports::resolved(&state.db, number).await?)
    }
    "reports" => (
      Value::from_serialize(reports::open(&state.db).await?),
      false,
    ),
    "banned" => listing(moderation::banned_users(&state.db, number).await?),
    "hidden" => listing(moderation::hidden_sketches(&state.db, number).await?),
    "comments" => {
      listing(moderation::deleted_comments(&state.db, number).await?)
    }
    _ => listing(moderation::log(&state.db, number).await?),
  };
  let tab_param = if tab == "reports" { "" } else { tab };
  let resolved_param = if resolved { "1" } else { "" };
  let pages = page_links(
    "/admin",
    &[("tab", tab_param), ("resolved", resolved_param)],
    number,
    has_more,
  );
  render_page(
    &state,
    &page,
    "admin.html",
    context! {
      tab,
      tabs,
      resolved,
      counts,
      items,
      pages,
      open_url => tab_url("reports", false),
      resolved_url => tab_url("reports", true),
    },
  )
}

/// The URL of a tab's first page.
fn tab_url(tab: &str, resolved: bool) -> String {
  let tab = if tab == "reports" { "" } else { tab };
  url_with(
    "/admin",
    &[("tab", tab), ("resolved", if resolved { "1" } else { "" })],
  )
}

/// A page of a list, as the template gets it.
fn listing<T: Serialize>(page: Page<T>) -> (Value, bool) {
  (Value::from_serialize(&page.items), page.has_more)
}

fn require_admin(user: &CurrentUser) -> Result<(), PageError> {
  if user.is_admin {
    Ok(())
  } else {
    Err(PageError::not_found())
  }
}

#[derive(Deserialize)]
pub struct UserQuery {
  list: Option<String>,
  page: Option<String>,
}

/// The lists on a user's moderation page, by their `list` parameter, with
/// their headings.
const USER_LISTS: [(&str, &str); 4] = [
  ("submitted", "Reports they submitted"),
  ("against", "Reports against them"),
  ("actions", "Moderation actions"),
  ("comments", "Deleted comments"),
];

/// How many rows of each list the overview shows.
const OVERVIEW_ROWS: i64 = 10;

/// `GET /admin/users/{username}?list=submitted|against|actions|comments&page=N`
/// (admins only): a user's account facts (their email address included),
/// counts, and moderation record. The overview shows the newest
/// [`OVERVIEW_ROWS`] of each list, with a link to all of it; `list` shows
/// one list, paginated.
pub async fn user(
  State(state): State<AppState>,
  PageUser(viewer): PageUser,
  page: PageContext,
  Path(username): Path<String>,
  Query(query): Query<UserQuery>,
) -> PageResult {
  require_admin(&viewer)?;
  if !validate::is_username_shaped(&username) {
    return Err(PageError::not_found());
  }
  let user = users::by_username(&state.db, &username)
    .await?
    .ok_or_else(PageError::not_found)?;
  let record = moderation::user_record(&state.db, &user).await?;
  let only = USER_LISTS
    .iter()
    .map(|(key, _)| *key)
    .find(|key| query.list.as_deref() == Some(*key));
  let number = page_number(query.page.as_deref());
  let (size, offset) = match only {
    Some(_) => (moderation::PAGE_SIZE, moderation::page_offset(number)),
    None => (OVERVIEW_ROWS, 0),
  };
  let base = format!("/admin/users/{}", user.username);
  let mut lists = Vec::new();
  for (key, heading) in USER_LISTS {
    if only.is_some_and(|only| only != key) {
      continue;
    }
    let ((items, has_more), total) = match key {
      "submitted" => {
        let page =
          reports::submitted_by(&state.db, user.id, size, offset).await?;
        (listing(page), record.reports_submitted)
      }
      "against" => {
        let page = reports::against(&state.db, user.id, size, offset).await?;
        (listing(page), record.reports_against)
      }
      "actions" => {
        let page =
          moderation::log_involving(&state.db, user.id, size, offset).await?;
        (listing(page), record.log_entries)
      }
      _ => {
        let page =
          moderation::deleted_comments_by(&state.db, user.id, size, offset)
            .await?;
        (listing(page), record.deleted_comment_count)
      }
    };
    let pages =
      only.map(|_| page_links(&base, &[("list", key)], number, has_more));
    lists.push(context! {
      key,
      heading,
      items,
      total,
      pages,
      all_url => (only.is_none() && has_more)
        .then(|| url_with(&base, &[("list", key)])),
    });
  }
  render_page(
    &state,
    &page,
    "admin_user.html",
    context! {
      user => context! {
        username => &user.username,
        display_name => user.shown_name(),
        email => &user.email,
        email_verified => user.email_verified(),
        created_at => user.created_at,
        is_admin => user.is_admin,
        banned_at => user.banned_at,
        avatar_url => users::avatar_url(&user.username),
        is_viewer => user.id == viewer.id,
      },
      record,
      lists,
      only,
      overview_url => base,
    },
  )
}
