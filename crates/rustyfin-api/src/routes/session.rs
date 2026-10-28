use axum::extract::{Path, Query, State};
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use rustyfin_model::auth::Claims;
use rustyfin_model::media::MediaItem;
use rustyfin_model::session::{
    ClientCapabilities, DeviceInfoQueryResult, SessionCapabilitiesRequest, SessionCommand,
    SessionInfo,
};
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
        .route(
            "/Sessions/{sessionId}/Command/{command}",
            post(send_command_by_name),
        )
        .route("/Sessions/{sessionId}/Message", post(send_message))
        .route("/Sessions/{sessionId}/Playing", post(session_playing))
        .route(
            "/Sessions/{sessionId}/Playing/{command}",
            post(session_playing_command),
        )
        .route("/Sessions/{sessionId}/Viewing", post(session_viewing))
        .route(
            "/Sessions/{sessionId}/User/{userId}",
            post(session_add_user).delete(session_remove_user),
        )
        .route("/Sessions/Capabilities", post(session_capabilities))
        .route(
            "/Sessions/Capabilities/Full",
            post(session_capabilities_full),
        )
        .route("/Devices/{deviceId}/Command", post(send_device_command))
        .route("/Devices", delete(delete_device))
        .route("/Devices/Info", get(get_devices_info))
        .route(
            "/Devices/Options",
            get(get_device_options).post(update_device_options),
        )
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

/// POST /Sessions/{sessionId}/Command/{command}
async fn send_command_by_name(
    Path((session_id, _command)): Path<(String, String)>,
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let _ = crate::routes::websocket::broadcast_event(
        &state,
        rustyfin_model::ws_event::WsEvent::SessionChanged {
            data: serde_json::json!({"Id": session_id, "Command": _command}),
        },
    )
    .await;
    Ok(Json(serde_json::json!({})))
}

/// POST /Sessions/{sessionId}/Message
async fn send_message(
    Path(session_id): Path<String>,
    State(state): State<AppState>,
    Json(_body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let _ = crate::routes::websocket::broadcast_event(
        &state,
        rustyfin_model::ws_event::WsEvent::SessionChanged {
            data: serde_json::json!({"Id": session_id, "MessageType": "Notification"}),
        },
    )
    .await;
    Ok(Json(serde_json::json!({})))
}

/// POST /Sessions/{sessionId}/Playing
async fn session_playing(
    Path(_session_id): Path<String>,
    State(_state): State<AppState>,
    Json(_body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(serde_json::json!({})))
}

/// POST /Sessions/{sessionId}/Playing/{command}
async fn session_playing_command(
    Path((_session_id, _command)): Path<(String, String)>,
    State(_state): State<AppState>,
    Json(_body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(serde_json::json!({})))
}

/// POST /Sessions/{sessionId}/Viewing
async fn session_viewing(
    Path(_session_id): Path<String>,
    State(_state): State<AppState>,
    Json(_body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(serde_json::json!({})))
}

/// POST /Sessions/{sessionId}/User/{userId}
async fn session_add_user(
    Path((_session_id, _user_id)): Path<(String, String)>,
    State(_state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(serde_json::json!({})))
}

/// DELETE /Sessions/{sessionId}/User/{userId}
async fn session_remove_user(
    Path((_session_id, _user_id)): Path<(String, String)>,
    State(_state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(serde_json::json!({})))
}

/// Build ClientCapabilities from a SessionCapabilitiesRequest.
fn build_capabilities_from_request(req: &SessionCapabilitiesRequest) -> ClientCapabilities {
    let has_remote_control = req
        .supported_functions
        .as_ref()
        .map(|f| f.iter().any(|s| s.contains("RemoteControl")));
    let has_content_upload = req
        .supported_functions
        .as_ref()
        .map(|f| f.iter().any(|s| s.contains("ContentUpload")));

    ClientCapabilities {
        playable_media_types: req.playable_media_types.clone(),
        supported_commands: req.supported_commands.clone(),
        supports_media_control: has_remote_control,
        supports_remote_control: has_remote_control,
        supports_gapless_media_playback: None,
        supports_content_upload: has_content_upload,
        supports_album_art: None,
        supports_pairing: None,
        supports_persistent_identifier: None,
        supports_audio_playback: req
            .playable_media_types
            .as_ref()
            .map(|t| t.iter().any(|s| s == "Audio")),
        supports_video_playback: req
            .playable_media_types
            .as_ref()
            .map(|t| t.iter().any(|s| s == "Video")),
        supports_photo_playback: req
            .playable_media_types
            .as_ref()
            .map(|t| t.iter().any(|s| s == "Photo")),
        device_profile: req.device_profile.clone(),
    }
}

/// Update session capabilities for the given user.
async fn update_user_capabilities(
    state: &AppState,
    user_id: uuid::Uuid,
    caps: &ClientCapabilities,
) -> Result<(), ApiError> {
    let caps_json = serde_json::to_string(caps).map_err(|e| ApiError::Internal(e.to_string()))?;

    let sessions = rustyfin_db::repo_session::get_active_sessions(&state.db).await?;
    for s in &sessions {
        if s.user_id == user_id {
            rustyfin_db::repo_session::update_session_capabilities(&state.db, &s.id, &caps_json)
                .await
                .map_err(|e| ApiError::Internal(e.to_string()))?;
            break;
        }
    }
    Ok(())
}

/// POST /Sessions/Capabilities
/// Clients report their capabilities so the server knows what features they support.
async fn session_capabilities(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<SessionCapabilitiesRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let caps = build_capabilities_from_request(&req);
    update_user_capabilities(&state, claims.sub, &caps).await?;
    Ok(Json(serde_json::json!({})))
}

/// POST /Sessions/Capabilities/Full
/// Full capabilities report, same as /Sessions/Capabilities but may include additional details.
async fn session_capabilities_full(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<SessionCapabilitiesRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let caps = build_capabilities_from_request(&req);
    update_user_capabilities(&state, claims.sub, &caps).await?;
    Ok(Json(serde_json::json!({})))
}

/// GET /Devices/Info
async fn get_devices_info(
    State(state): State<AppState>,
) -> Result<Json<DeviceInfoQueryResult>, ApiError> {
    get_devices(State(state)).await
}

/// GET /Devices/Options
async fn get_device_options(
    State(_state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(serde_json::json!({
        "CommandCapabilities": [],
        "PlayableMediaTypes": ["Audio", "Video"],
        "SupportedCommands": ["MoveUp", "MoveDown", "MoveLeft", "MoveRight"],
        "SupportsMediaControl": true
    })))
}

/// POST /Devices/Options
async fn update_device_options(
    State(_state): State<AppState>,
    Json(_body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(serde_json::json!({})))
}
