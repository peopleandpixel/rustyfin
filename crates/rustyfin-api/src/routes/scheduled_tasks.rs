use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use rustyfin_model::scheduled_task::{ScheduledTaskInfo, TaskTriggerInfo};

use crate::error::ApiError;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/ScheduledTasks", get(get_scheduled_tasks))
        .route("/ScheduledTasks/{id}/Triggers", post(set_task_triggers))
}

async fn get_scheduled_tasks(
    State(state): State<AppState>,
) -> Json<Vec<ScheduledTaskInfo>> {
    let tasks = state.task_scheduler.get_all_tasks().await;
    Json(tasks)
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
