use crate::query::Query;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use rustyfin_model::auth::Claims;
use rustyfin_model::media::MediaItem;
use rustyfin_model::session::{
    ClientCapabilities, SessionCapabilitiesRequest, SessionCommand, SessionInfo,
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
}

/// Administrator-only device routes. `routes::build_router` wraps them in `require_admin`. The
/// device list, info and options are in `devices::routes`.
pub fn admin_routes() -> Router<AppState> {
    Router::new().route("/Devices", delete(delete_device))
}

async fn get_sessions(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<Vec<SessionInfo>>, ApiError> {
    let mut sessions = rustyfin_db::repo_session::get_active_sessions(&state.db_ro).await?;
    // A non-admin sees only their own sessions, as in Jellyfin.
    if !claims.is_admin {
        sessions.retain(|s| s.user_id == claims.sub);
    }
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
    Extension(claims): Extension<Claims>,
) -> Result<Json<Option<MediaItem>>, ApiError> {
    if let Some(session_id) = query.session_id {
        let owner =
            rustyfin_db::repo_session::get_session_user_id(&state.db_ro, &session_id).await?;
        check_control(&claims, owner, "Session")?;
        let now_playing =
            rustyfin_db::repo_session::get_now_playing_item(&state.db_ro, &session_id).await?;
        Ok(Json(now_playing))
    } else {
        Ok(Json(None))
    }
}

async fn logout_session(
    State(state): State<AppState>,
    token: Option<Extension<rustyfin_auth::RequestToken>>,
) -> Result<StatusCode, ApiError> {
    if let Some(Extension(rt)) = token {
        rustyfin_auth::logout(&state.db, &rt.0).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn send_command(
    Path(session_id): Path<String>,
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(cmd): Json<SessionCommand>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let owner = rustyfin_db::repo_session::get_session_user_id(&state.db_ro, &session_id).await?;
    check_control(&claims, owner, "Session")?;
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
                "UserId": owner,
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

async fn delete_device(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Query(q): Query<DeviceIdQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !claims.is_admin {
        return Err(ApiError::Forbidden(
            "Administrator access is required.".to_string(),
        ));
    }
    rustyfin_db::repo_session::delete_device(&state.db, &q.id).await?;
    Ok(Json(serde_json::json!({})))
}

/// POST /Devices/{id}/Command — Send a remote command to a device via HTTP.
/// Delegates to the WebSocket command delivery system.
async fn send_device_command(
    Path(device_id): Path<String>,
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(cmd): Json<SessionCommand>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let owner = rustyfin_db::repo_session::get_device_user_id(&state.db_ro, &device_id).await?;
    check_control(&claims, owner, "Device")?;
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
                "UserId": owner,
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
    Extension(claims): Extension<Claims>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let owner = rustyfin_db::repo_session::get_session_user_id(&state.db_ro, &session_id).await?;
    check_control(&claims, owner, "Session")?;
    let _ = crate::routes::websocket::broadcast_event(
        &state,
        rustyfin_model::ws_event::WsEvent::SessionChanged {
            data: serde_json::json!({"Id": session_id, "Command": _command, "UserId": owner}),
        },
    )
    .await;
    Ok(Json(serde_json::json!({})))
}

/// POST /Sessions/{sessionId}/Message
async fn send_message(
    Path(session_id): Path<String>,
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(_body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let owner = rustyfin_db::repo_session::get_session_user_id(&state.db_ro, &session_id).await?;
    check_control(&claims, owner, "Session")?;
    let _ = crate::routes::websocket::broadcast_event(
        &state,
        rustyfin_model::ws_event::WsEvent::SessionChanged {
            data: serde_json::json!({"Id": session_id, "MessageType": "Notification", "UserId": owner}),
        },
    )
    .await;
    Ok(Json(serde_json::json!({})))
}

/// Remote control of a session or device needs its owner or an administrator (Jellyfin's
/// `SessionManager.AssertCanControl`; Rustyfin has no `EnableRemoteControlOfOtherUsers` policy).
/// `owner` is `None` when the target does not exist: 404 for an administrator, and 403 for
/// anyone else, so other users' ids cannot be probed.
fn check_control(claims: &Claims, owner: Option<uuid::Uuid>, target: &str) -> Result<(), ApiError> {
    match owner {
        Some(owner) if owner == claims.sub || claims.is_admin => Ok(()),
        None if claims.is_admin => Err(ApiError::NotFound(format!("{target} not found"))),
        _ => Err(ApiError::Forbidden(
            "The current user does not have permission to remote control other users.".to_string(),
        )),
    }
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

/// Store the capabilities on the requesting device's sessions when the request's token
/// names a device. Otherwise store them on the user's most recently active session.
async fn update_user_capabilities(
    state: &AppState,
    user_id: uuid::Uuid,
    token: Option<&str>,
    caps: &ClientCapabilities,
) -> Result<(), ApiError> {
    let caps_json = serde_json::to_string(caps).map_err(|e| ApiError::Internal(e.to_string()))?;

    if let Some(token) = token
        && let Some(device_id) = rustyfin_auth::token_device_id(&state.db, token).await?
    {
        rustyfin_db::repo_session::update_device_capabilities(
            &state.db, user_id, &device_id, &caps_json,
        )
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
        return Ok(());
    }

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

/// The capabilities of a `POST /Sessions/Capabilities`. Jellyfin binds them from the query
/// (`playableMediaTypes`, `supportedCommands`, `supportsMediaControl`,
/// `supportsPersistentIdentifier`) and the request has no body. A JSON body, as Rustyfin took
/// before, is still read; the query fills what it leaves out.
fn capabilities_from_query(
    params: &[(String, String)],
    body: &[u8],
) -> Result<ClientCapabilities, ApiError> {
    use crate::query::{bool_param, list_values};

    let mut req: SessionCapabilitiesRequest = if body.iter().all(u8::is_ascii_whitespace) {
        SessionCapabilitiesRequest::default()
    } else {
        serde_json::from_slice(body)
            .map_err(|e| ApiError::BadRequest(format!("Invalid capabilities: {e}")))?
    };
    let list = |key| Some(list_values(params, key, ',')).filter(|values| !values.is_empty());
    if req.playable_media_types.is_none() {
        req.playable_media_types = list("PlayableMediaTypes");
    }
    if req.supported_commands.is_none() {
        req.supported_commands = list("SupportedCommands");
    }
    let mut caps = build_capabilities_from_request(&req);
    if let Some(control) = bool_param(params, "SupportsMediaControl")? {
        caps.supports_media_control = Some(control);
        caps.supports_remote_control = Some(control);
    }
    if let Some(persistent) = bool_param(params, "SupportsPersistentIdentifier")? {
        caps.supports_persistent_identifier = Some(persistent);
    }
    Ok(caps)
}

/// POST /Sessions/Capabilities
/// Clients report their capabilities so the server knows what features they support. 204, as
/// Jellyfin answers.
async fn session_capabilities(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    token: Option<Extension<rustyfin_auth::RequestToken>>,
    Query(params): Query<Vec<(String, String)>>,
    body: axum::body::Bytes,
) -> Result<StatusCode, ApiError> {
    let caps = capabilities_from_query(&params, &body)?;
    let token = token.as_ref().map(|Extension(rt)| rt.0.as_str());
    update_user_capabilities(&state, claims.sub, token, &caps).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// POST /Sessions/Capabilities/Full
/// Full capabilities report, same as /Sessions/Capabilities but may include additional details.
async fn session_capabilities_full(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    token: Option<Extension<rustyfin_auth::RequestToken>>,
    Json(req): Json<SessionCapabilitiesRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let caps = build_capabilities_from_request(&req);
    let token = token.as_ref().map(|Extension(rt)| rt.0.as_str());
    update_user_capabilities(&state, claims.sub, token, &caps).await?;
    Ok(Json(serde_json::json!({})))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn r3playback_capabilities_come_from_the_query_without_a_body() {
        let query = params(&[
            ("playableMediaTypes", "Video,Audio"),
            ("supportedCommands", "Play"),
            ("supportedCommands", "Seek"),
            ("supportsMediaControl", "true"),
            ("SUPPORTSPERSISTENTIDENTIFIER", "false"),
        ]);
        for body in [b"".as_slice(), b"  \n".as_slice()] {
            let caps = capabilities_from_query(&query, body).unwrap();
            assert_eq!(
                caps.playable_media_types,
                Some(vec!["Video".to_string(), "Audio".to_string()])
            );
            assert_eq!(
                caps.supported_commands,
                Some(vec!["Play".to_string(), "Seek".to_string()])
            );
            assert_eq!(caps.supports_media_control, Some(true));
            assert_eq!(caps.supports_remote_control, Some(true));
            assert_eq!(caps.supports_persistent_identifier, Some(false));
            assert_eq!(caps.supports_video_playback, Some(true));
        }
        // Nothing at all is an empty report, not an error
        let caps = capabilities_from_query(&[], b"").unwrap();
        assert_eq!(caps.playable_media_types, None);
        assert_eq!(caps.supports_media_control, None);
    }

    #[test]
    fn r3playback_a_json_body_is_still_read_and_the_query_fills_the_rest() {
        let body = br#"{"PlayableMediaTypes":["Audio"]}"#;
        let caps =
            capabilities_from_query(&params(&[("SupportedCommands", "Play")]), body).unwrap();
        assert_eq!(caps.playable_media_types, Some(vec!["Audio".to_string()]));
        assert_eq!(caps.supported_commands, Some(vec!["Play".to_string()]));
        assert!(matches!(
            capabilities_from_query(&[], b"{not json"),
            Err(ApiError::BadRequest(_))
        ));
        assert!(matches!(
            capabilities_from_query(&params(&[("SupportsMediaControl", "maybe")]), b""),
            Err(ApiError::BadRequest(_))
        ));
    }
}
