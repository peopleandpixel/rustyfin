use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use rustyfin_model::scheduled_task::{ScheduledTaskInfo, TaskTriggerInfo};

use crate::error::ApiError;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/ScheduledTasks", get(get_scheduled_tasks))
        .route("/ScheduledTasks/{id}", get(get_scheduled_task))
        .route("/ScheduledTasks/{id}/Triggers", post(set_task_triggers))
        .route(
            "/ScheduledTasks/Running/{taskId}",
            post(start_task).delete(stop_task),
        )
}

/// GET /ScheduledTasks - every task; `isHidden` keeps only the tasks with that `IsHidden`, as in
/// Jellyfin (none is hidden).
async fn get_scheduled_tasks(
    State(state): State<AppState>,
    Query(params): Query<Vec<(String, String)>>,
) -> Result<Json<Vec<ScheduledTaskInfo>>, ApiError> {
    let is_hidden = crate::query::bool_param(&params, "isHidden")?;
    let mut tasks = state.task_scheduler.get_all_tasks().await;
    if let Some(hidden) = is_hidden {
        tasks.retain(|t| t.is_hidden == hidden);
    }
    Ok(Json(tasks))
}

async fn set_task_triggers(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(triggers): Json<Vec<TaskTriggerInfo>>,
) -> Result<StatusCode, ApiError> {
    if state.task_scheduler.set_triggers(&id, triggers).await {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound(format!("task '{}' not found", id)))
    }
}

/// GET /ScheduledTasks/{taskId} - Get a specific task
async fn get_scheduled_task(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> Result<Json<ScheduledTaskInfo>, ApiError> {
    let tasks = state.task_scheduler.get_all_tasks().await;
    tasks
        .into_iter()
        .find(|t| t.id == task_id)
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("task '{}' not found", task_id)))
}

/// POST /ScheduledTasks/Running/{taskId} - Start a task
async fn start_task(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    if state.task_scheduler.start_task(&task_id).await {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound(format!("task '{}' not found", task_id)))
    }
}

/// DELETE /ScheduledTasks/Running/{taskId} - Stop a running task
async fn stop_task(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    if state.task_scheduler.stop_task(&task_id).await {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound(format!("task '{}' not found", task_id)))
    }
}
