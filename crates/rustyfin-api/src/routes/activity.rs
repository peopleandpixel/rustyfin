use crate::query::Query;
use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Extension, Json, Router};
use uuid::Uuid;

use rustyfin_model::activity::ActivityLogResult;
use rustyfin_model::auth::Claims;
use rustyfin_model::notification::NotificationResult;

use crate::error::ApiError;
use crate::query;
use crate::routes::playback::path_user;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/Notifications/{userId}", get(get_notifications))
}

/// Administrator-only activity log routes. `routes::build_router` wraps them in `require_admin`.
pub fn admin_routes() -> Router<AppState> {
    Router::new().route("/System/ActivityLog/Entries", get(get_activity_log))
}

/// Writes an entry to the activity log, with Jellyfin's `Type` and `Severity` names. A failure is
/// logged, never returned: the action it records has already happened.
pub(crate) async fn record(
    state: &AppState,
    name: &str,
    entry_type: &str,
    user_id: Option<Uuid>,
    severity: &str,
) {
    if let Err(e) = rustyfin_db::repo_activity::log_activity(
        &state.db, name, entry_type, user_id, None, severity, None,
    )
    .await
    {
        tracing::warn!("could not write the activity log: {e}");
    }
}

/// Entries per page when the request gives no `limit`.
const DEFAULT_LIMIT: usize = 100;

/// GET /System/ActivityLog/Entries - Jellyfin's parameters (`startIndex`, `limit`, `minDate`,
/// `hasUserId`), matched without case.
async fn get_activity_log(
    State(state): State<AppState>,
    Query(params): Query<Vec<(String, String)>>,
) -> Result<Json<ActivityLogResult>, ApiError> {
    let start_index = query::count_param(&params, "startIndex")?.unwrap_or(0);
    let limit = query::count_param(&params, "limit")?.unwrap_or(DEFAULT_LIMIT);
    let min_date = query::first_value(&params, "minDate")
        .map(|date| {
            parse_min_date(date)
                .map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string())
                .ok_or_else(|| ApiError::BadRequest(format!("minDate: \"{date}\" is not a date")))
        })
        .transpose()?;
    let has_user_id = query::bool_param(&params, "hasUserId")?;
    let result = rustyfin_db::repo_activity::query_activity_log(
        &state.db_ro,
        i64::try_from(start_index).unwrap_or(i64::MAX),
        i64::try_from(limit).unwrap_or(i64::MAX),
        min_date.as_deref(),
        has_user_id,
    )
    .await?;
    Ok(Json(result))
}

/// A `minDate` as UTC: an ISO 8601 date and time with an offset, or without one (taken as UTC),
/// or a date alone.
pub(crate) fn parse_min_date(date: &str) -> Option<chrono::NaiveDateTime> {
    if let Ok(d) = chrono::DateTime::parse_from_rfc3339(date) {
        return Some(d.naive_utc());
    }
    if let Ok(d) = chrono::NaiveDateTime::parse_from_str(date, "%Y-%m-%dT%H:%M:%S%.f") {
        return Some(d);
    }
    chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
}

async fn get_notifications(
    State(state): State<AppState>,
    Path(user_id): Path<Uuid>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<NotificationResult>, ApiError> {
    let user_id = path_user(&state, &claims, user_id).await?;
    let result = rustyfin_db::repo_notification::get_notifications(&state.db_ro, user_id).await?;
    Ok(Json(result))
}
