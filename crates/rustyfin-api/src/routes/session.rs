use axum::extract::{Path, Query, State};
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use rustyfin_model::auth::Claims;
use rustyfin_model::media::MediaItem;
use rustyfin_model::session::{DeviceInfoQueryResult, SessionCommand, SessionInfo};
use serde::Deserialize;

use crate::error::ApiError;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/Sessions", get(get_sessions))
        .route("/Sessions/Logout", post(logout_session))
        .route(
            "/Sessions/Playing/NowPlayingItem",
            get(get_now_playing_item),
        )
        .route("/Sessions/{id}/Command", post(send_command))
        .route("/Devices/{id}/Command", post(send_device_command))
        .route("/Devices", get(get_devices))
        .route("/Devices", delete(delete_device))
}

async fn get_sessions(State(state): State<AppState>) -> Result<Json<Vec<SessionInfo>>, ApiError> {
    let sessions = rustyfin_db::repo_session::get_active_sessions(&state.db_ro).await?;
    Ok(Json(sessions))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct NowPlayingItemQuery {
    #[serde(default)]
    pub session_id: Option<String>,
}

async fn get_now_playing_item(
    State(state): State<AppState>,
    Query(query): Query<NowPlayingItemQuery>,
) -> Result<Json<Option<MediaItem>>, ApiError> {
    if let Some(session_id) = query.session_id {
        let now_playing =
            rustyfin_db::repo_session::get_now_playing_item(&state.db_ro, &session_id).await?;
        Ok(Json(now_playing))
    } else {
        Ok(Json(None))
    }
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
    State(state): State<AppState>,
    Json(cmd): Json<SessionCommand>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Try to deliver the command via WebSocket
    let delivered = crate::routes::websocket::send_command_to_session(
        &session_id,
        cmd.command.clone(),
        cmd.arguments.clone(),
    )
    .await;

    if delivered {
        tracing::info!(
            "command '{}' delivered to session {} via WebSocket",
            cmd.command,
            session_id
        );
    } else {
        tracing::warn!(
            "command '{}' for session {} not delivered – no active WebSocket connection",
            cmd.command,
            session_id
        );
    }

    // Also broadcast SessionChanged event so all clients know about the command
    let _ = crate::routes::websocket::broadcast_event(
        &state,
        rustyfin_model::ws_event::WsEvent::SessionChanged {
            data: serde_json::json!({
                "Id": session_id,
                "Command": cmd.command,
            }),
        },
    )
    .await;

    Ok(Json(serde_json::json!({})))
}

#[derive(Debug, Deserialize)]
pub struct DeviceIdQuery {
    pub id: String,
}

async fn get_devices(
    State(state): State<AppState>,
) -> Result<Json<DeviceInfoQueryResult>, ApiError> {
    let devices = rustyfin_db::repo_session::get_all_devices(&state.db_ro).await?;
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

/// POST /Devices/{id}/Command — Send a remote command to a device via HTTP.
/// Delegates to the WebSocket command delivery system.
async fn send_device_command(
    Path(device_id): Path<String>,
    State(state): State<AppState>,
    Json(cmd): Json<SessionCommand>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Try to deliver via WebSocket using device ID as session ID
    let delivered = crate::routes::websocket::send_command_to_session(
        &device_id,
        cmd.command.clone(),
        cmd.arguments.clone(),
    )
    .await;

    if delivered {
        tracing::info!(
            "command '{}' delivered to device {} via WebSocket",
            cmd.command,
            device_id
        );
    } else {
        tracing::warn!(
            "command '{}' for device {} not delivered – no active WebSocket connection",
            cmd.command,
            device_id
        );
    }

    // Broadcast SessionChanged event
    let _ = crate::routes::websocket::broadcast_event(
        &state,
        rustyfin_model::ws_event::WsEvent::SessionChanged {
            data: serde_json::json!({
                "Id": device_id,
                "Command": cmd.command,
            }),
        },
    )
    .await;

    Ok(Json(serde_json::json!({})))
}
