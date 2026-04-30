use axum::extract::{Path, Query, State};
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use rustyfin_model::auth::Claims;
use rustyfin_model::session::{DeviceInfoQueryResult, SessionCommand, SessionInfo};
use serde::Deserialize;

use crate::error::ApiError;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/Sessions", get(get_sessions))
        .route("/Sessions/Logout", post(logout_session))
        .route("/Sessions/{id}/Command", post(send_command))
        .route("/Devices", get(get_devices))
        .route("/Devices", delete(delete_device))
}

async fn get_sessions(
    State(state): State<AppState>,
) -> Result<Json<Vec<SessionInfo>>, ApiError> {
    let sessions = rustyfin_db::repo_session::get_active_sessions(&state.db).await?;
    Ok(Json(sessions))
}

async fn logout_session(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Build session id from user + device info (same logic as middleware)
    // Delete all sessions for this user as fallback
    let sessions = rustyfin_db::repo_session::get_active_sessions(&state.db).await?;
    for s in &sessions {
        if s.user_id == claims.sub {
            rustyfin_db::repo_session::delete_session(&state.db, &s.id).await?;
            break;
        }
    }
    Ok(Json(serde_json::json!({})))
}

async fn send_command(
    Path(session_id): Path<String>,
    Json(_cmd): Json<SessionCommand>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // WebSocket-based command delivery is a placeholder;
    // real implementation would push via WS to the target session.
    tracing::info!("command sent to session {session_id} (not delivered – no WS connection)");
    Ok(Json(serde_json::json!({})))
}

#[derive(Debug, Deserialize)]
pub struct DeviceIdQuery {
    pub id: String,
}

async fn get_devices(
    State(state): State<AppState>,
) -> Result<Json<DeviceInfoQueryResult>, ApiError> {
    let devices = rustyfin_db::repo_session::get_all_devices(&state.db).await?;
    let count = devices.len();
    Ok(Json(DeviceInfoQueryResult {
        items: devices,
        total_record_count: count,
    }))
}

async fn delete_device(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Query(q): Query<DeviceIdQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !claims.is_admin {
        return Err(ApiError::Unauthorized);
    }
    rustyfin_db::repo_session::delete_device(&state.db, &q.id).await?;
    Ok(Json(serde_json::json!({})))
}
