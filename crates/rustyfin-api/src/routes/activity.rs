use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use uuid::Uuid;

use rustyfin_model::activity::ActivityLogResult;
use rustyfin_model::notification::NotificationResult;

use crate::error::ApiError;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/System/ActivityLog/Entries", get(get_activity_log))
        .route("/Notifications/{userId}", get(get_notifications))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ActivityLogQuery {
    #[serde(default)]
    pub start_index: Option<i64>,
    #[serde(default = "default_limit")]
    pub limit: i64,
}

fn default_limit() -> i64 {
    100
}

async fn get_activity_log(
    State(state): State<AppState>,
    Query(query): Query<ActivityLogQuery>,
) -> Result<Json<ActivityLogResult>, ApiError> {
    let result = rustyfin_db::repo_activity::get_activity_log(
        &state.db_ro,
        query.start_index.unwrap_or(0),
        query.limit,
    )
    .await?;
    Ok(Json(result))
}

async fn get_notifications(
    State(state): State<AppState>,
    Path(user_id): Path<Uuid>,
) -> Result<Json<NotificationResult>, ApiError> {
    let result = rustyfin_db::repo_notification::get_notifications(&state.db_ro, user_id).await?;
    Ok(Json(result))
}
