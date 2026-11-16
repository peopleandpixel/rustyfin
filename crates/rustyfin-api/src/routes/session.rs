use crate::query::Query;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use rustyfin_model::auth::Claims;
use rustyfin_model::media::MediaItem;
use rustyfin_model::session::{ClientCapabilities, SessionCapabilitiesRequest, SessionCommand};
use rustyfin_model::ws_event::WsEvent;
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
        .route("/Sessions/Playing/Ping", post(playing_ping))
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

/// The query string's pairs; Jellyfin binds the names without regard to case.
type Pairs = Query<Vec<(String, String)>>;

/// The first non-blank value of `name` in `pairs`, matched without regard to case.
fn param<'a>(pairs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(k, v)| k.eq_ignore_ascii_case(name) && !v.trim().is_empty())
        .map(|(_, v)| v.trim())
}

/// A comma-separated list parameter, also when repeated.
fn list_param(pairs: &[(String, String)], name: &str) -> Option<Vec<String>> {
    let values: Vec<String> = pairs
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case(name))
        .flat_map(|(_, v)| v.split(','))
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .collect();
    (!values.is_empty()).then_some(values)
}

fn bad_request(message: impl Into<String>) -> ApiError {
    ApiError::BadRequest(message.into())
}

/// The value of `name` parsed as `T`, or a 400 when it is there but does not parse.
fn parsed<T: std::str::FromStr>(
    pairs: &[(String, String)],
    name: &str,
) -> Result<Option<T>, ApiError> {
    param(pairs, name)
        .map(|v| {
            v.parse()
                .map_err(|_| bad_request(format!("The value '{v}' is not valid for {name}.")))
        })
        .transpose()
}

/// GET /Sessions with Jellyfin's `controllableByUserId`, `deviceId` and `activeWithinSeconds`.
async fn get_sessions(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Query(q): Pairs,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    let filter = crate::sessions::Filter {
        controllable_by: parsed(&q, "controllableByUserId")?,
        device_id: param(&q, "deviceId").map(str::to_string),
        active_within_seconds: parsed(&q, "activeWithinSeconds")?,
    };
    // A non-admin sees only their own sessions, as in Jellyfin.
    let sessions = crate::sessions::list(&state, claims.sub, claims.is_admin, &filter).await?;
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
        crate::sessions::changed(&state);
    }
    Ok(StatusCode::NO_CONTENT)
}

/// POST /Sessions/Playing/Ping: the client is still playing `playSessionId`. Jellyfin requires
/// the id and answers 400 without it.
async fn playing_ping(
    State(state): State<AppState>,
    Query(q): Pairs,
    token: Option<Extension<rustyfin_auth::RequestToken>>,
) -> Result<StatusCode, ApiError> {
    if param(&q, "playSessionId").is_none() {
        return Err(bad_request("The playSessionId field is required."));
    }
    let token = token.as_ref().map(|Extension(rt)| rt.0.as_str());
    if let Some(session) = crate::sessions::for_token(&state, token).await? {
        crate::sessions::ping(&session);
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Jellyfin's `GeneralCommandType` names.
const GENERAL_COMMANDS: &[&str] = &[
    "MoveUp",
    "MoveDown",
    "MoveLeft",
    "MoveRight",
    "PageUp",
    "PageDown",
    "PreviousLetter",
    "NextLetter",
    "ToggleOsd",
    "ToggleContextMenu",
    "Select",
    "Back",
    "TakeScreenshot",
    "SendKey",
    "SendString",
    "GoHome",
    "GoToSettings",
    "VolumeUp",
    "VolumeDown",
    "Mute",
    "Unmute",
    "ToggleMute",
    "SetVolume",
    "SetAudioStreamIndex",
    "SetSubtitleStreamIndex",
    "ToggleFullscreen",
    "DisplayContent",
    "GoToSearch",
    "DisplayMessage",
    "SetRepeatMode",
    "ChannelUp",
    "ChannelDown",
    "Guide",
    "ToggleStats",
    "PlayMediaSource",
    "PlayTrailers",
    "SetShuffleQueue",
    "PlayState",
    "PlayNext",
    "ToggleOsdMenu",
    "Play",
    "SetMaxStreamingBitrate",
    "SetPlaybackOrder",
];

/// Jellyfin's `PlaystateCommand` names.
const PLAYSTATE_COMMANDS: &[&str] = &[
    "Stop",
    "Pause",
    "Unpause",
    "NextTrack",
    "PreviousTrack",
    "Seek",
    "Rewind",
    "FastForward",
    "PlayPause",
];

/// Jellyfin's `PlayCommand` names.
const PLAY_COMMANDS: &[&str] = &[
    "PlayNow",
    "PlayNext",
    "PlayLast",
    "PlayInstantMix",
    "PlayShuffle",
];

/// `name` as written in `names`, matched without regard to case as ASP.NET binds an enum.
fn known(names: &[&'static str], name: &str) -> Option<&'static str> {
    names.iter().copied().find(|n| n.eq_ignore_ascii_case(name))
}

/// Checks that the caller may control `session_id`, then sends it `event` over its sockets.
/// Answers 204 also when the session has no open socket, as Jellyfin does.
async fn deliver(
    state: &AppState,
    claims: &Claims,
    session_id: &str,
    event: WsEvent,
) -> Result<StatusCode, ApiError> {
    let owner = rustyfin_db::repo_session::get_session_user_id(&state.db_ro, session_id).await?;
    check_control(claims, owner, "Session")?;
    if !crate::routes::websocket::send_to_session(session_id, event).await {
        tracing::debug!("session {session_id} has no open socket; the command was not delivered");
    }
    Ok(StatusCode::NO_CONTENT)
}

/// A `GeneralCommand` message's data.
fn general_command(name: &str, controller: uuid::Uuid, arguments: serde_json::Value) -> WsEvent {
    WsEvent::GeneralCommand {
        data: serde_json::json!({
            "Name": name,
            "ControllingUserId": controller,
            "Arguments": arguments,
        }),
    }
}

/// POST /Sessions/{sessionId}/Command: a `GeneralCommand` body, sent as a `GeneralCommand`.
async fn send_command(
    Path(session_id): Path<String>,
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(cmd): Json<SessionCommand>,
) -> Result<StatusCode, ApiError> {
    let name = known(GENERAL_COMMANDS, &cmd.name)
        .ok_or_else(|| bad_request(format!("Unknown command '{}'.", cmd.name)))?;
    let arguments = cmd.arguments.unwrap_or_else(|| serde_json::json!({}));
    let event = general_command(name, claims.sub, arguments);
    deliver(&state, &claims, &session_id, event).await
}

/// POST /Sessions/{sessionId}/Command/{command}: a `GeneralCommand` without arguments.
async fn send_command_by_name(
    Path((session_id, command)): Path<(String, String)>,
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
) -> Result<StatusCode, ApiError> {
    let name = known(GENERAL_COMMANDS, &command)
        .ok_or_else(|| bad_request(format!("Unknown command '{command}'.")))?;
    let event = general_command(name, claims.sub, serde_json::json!({}));
    deliver(&state, &claims, &session_id, event).await
}

/// Jellyfin's `MessageCommand`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct MessageCommand {
    #[serde(default, alias = "header")]
    header: Option<String>,
    #[serde(alias = "text")]
    text: String,
    #[serde(default, alias = "timeoutMs")]
    timeout_ms: Option<i64>,
}

/// POST /Sessions/{sessionId}/Message: a `DisplayMessage` command. Jellyfin sends it with an
/// empty controlling user and "Message from Server" for a missing header.
async fn send_message(
    Path(session_id): Path<String>,
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(msg): Json<MessageCommand>,
) -> Result<StatusCode, ApiError> {
    let header = msg
        .header
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "Message from Server".to_string());
    let mut arguments = serde_json::json!({"Header": header, "Text": msg.text});
    if let Some(ms) = msg.timeout_ms {
        arguments["TimeoutMs"] = serde_json::json!(ms.to_string());
    }
    let event = general_command("DisplayMessage", uuid::Uuid::nil(), arguments);
    deliver(&state, &claims, &session_id, event).await
}

/// POST /Sessions/{sessionId}/Playing: Jellyfin's play request, from the query string
/// (`playCommand`, `itemIds`, `startPositionTicks`, `mediaSourceId`, `audioStreamIndex`,
/// `subtitleStreamIndex`, `startIndex`), sent as a `Play` message.
async fn session_playing(
    Path(session_id): Path<String>,
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Query(q): Pairs,
) -> Result<StatusCode, ApiError> {
    let command = param(&q, "playCommand")
        .ok_or_else(|| bad_request("The playCommand field is required."))?;
    let command = known(PLAY_COMMANDS, command)
        .ok_or_else(|| bad_request(format!("Unknown play command '{command}'.")))?;
    let ids =
        list_param(&q, "itemIds").ok_or_else(|| bad_request("The itemIds field is required."))?;
    for id in &ids {
        uuid::Uuid::parse_str(id).map_err(|_| bad_request(format!("'{id}' is not an item id.")))?;
    }
    let mut data = serde_json::json!({"ItemIds": ids});
    if let Some(ticks) = parsed::<i64>(&q, "startPositionTicks")? {
        data["StartPositionTicks"] = ticks.into();
    }
    data["PlayCommand"] = command.into();
    data["ControllingUserId"] = serde_json::json!(claims.sub);
    if let Some(id) = param(&q, "mediaSourceId") {
        data["MediaSourceId"] = id.into();
    }
    for name in ["AudioStreamIndex", "SubtitleStreamIndex", "StartIndex"] {
        if let Some(v) = parsed::<i32>(&q, name)? {
            data[name] = v.into();
        }
    }
    deliver(&state, &claims, &session_id, WsEvent::Play { data }).await
}

/// POST /Sessions/{sessionId}/Playing/{command}: a `Playstate` message (`seekPositionTicks` for a
/// seek).
async fn session_playing_command(
    Path((session_id, command)): Path<(String, String)>,
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Query(q): Pairs,
) -> Result<StatusCode, ApiError> {
    let command = known(PLAYSTATE_COMMANDS, &command)
        .ok_or_else(|| bad_request(format!("Unknown playstate command '{command}'.")))?;
    let mut data = serde_json::json!({"Command": command});
    if let Some(ticks) = parsed::<i64>(&q, "seekPositionTicks")? {
        data["SeekPositionTicks"] = ticks.into();
    }
    data["ControllingUserId"] = serde_json::json!(claims.sub);
    deliver(&state, &claims, &session_id, WsEvent::Playstate { data }).await
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
    // The device's session, which its sockets joined
    let session = rustyfin_db::repo_session::find_device_session(&state.db_ro, &device_id)
        .await?
        .unwrap_or(device_id.clone());
    let event = general_command(
        &cmd.name,
        claims.sub,
        cmd.arguments.unwrap_or_else(|| serde_json::json!({})),
    );
    if !crate::routes::websocket::send_to_session(&session, event).await {
        tracing::warn!(
            "command '{}' for device {} not delivered – no active WebSocket connection",
            cmd.name,
            device_id
        );
    }
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

/// Build ClientCapabilities from a SessionCapabilitiesRequest. `SupportsMediaControl` is the
/// client's own flag, as in Jellyfin; older clients that only list a `RemoteControl` function
/// count too.
fn build_capabilities_from_request(req: &SessionCapabilitiesRequest) -> ClientCapabilities {
    let has_remote_control = req
        .supported_functions
        .as_ref()
        .map(|f| f.iter().any(|s| s.contains("RemoteControl")));
    let has_content_upload = req
        .supported_functions
        .as_ref()
        .map(|f| f.iter().any(|s| s.contains("ContentUpload")));
    let media_control = match (req.supports_media_control, has_remote_control) {
        (Some(v), _) => Some(v),
        (None, Some(true)) => Some(true),
        _ => Some(false),
    };

    ClientCapabilities {
        playable_media_types: req.playable_media_types.clone(),
        supported_commands: req.supported_commands.clone(),
        supports_media_control: media_control,
        supports_remote_control: media_control,
        supports_gapless_media_playback: None,
        supports_content_upload: has_content_upload,
        supports_album_art: None,
        supports_pairing: None,
        supports_persistent_identifier: req.supports_persistent_identifier,
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

/// Store the capabilities on the requesting device's session. A token without a device stores
/// them on the user's most recently active session.
async fn update_user_capabilities(
    state: &AppState,
    user_id: uuid::Uuid,
    token: Option<&str>,
    caps: &ClientCapabilities,
) -> Result<(), ApiError> {
    let caps_json = serde_json::to_string(caps).map_err(|e| ApiError::Internal(e.to_string()))?;

    if let Some(session) = crate::sessions::for_token(state, token).await? {
        rustyfin_db::repo_session::update_session_capabilities(&state.db, &session, &caps_json)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
        crate::sessions::changed(state);
        return Ok(());
    }

    let sessions = rustyfin_db::repo_session::get_active_sessions(&state.db).await?;
    for s in &sessions {
        if s.user_id == user_id {
            rustyfin_db::repo_session::update_session_capabilities(&state.db, &s.id, &caps_json)
                .await
                .map_err(|e| ApiError::Internal(e.to_string()))?;
            crate::sessions::changed(state);
            break;
        }
    }
    Ok(())
}

/// POST /Sessions/Capabilities: Jellyfin's query-string form (`playableMediaTypes`,
/// `supportedCommands`, `supportsMediaControl`, `supportsPersistentIdentifier`). A JSON body, as
/// older Rustyfin clients post, is read too.
async fn session_capabilities(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    token: Option<Extension<rustyfin_auth::RequestToken>>,
    Query(q): Pairs,
    body: Option<Json<SessionCapabilitiesRequest>>,
) -> Result<StatusCode, ApiError> {
    let req = match body {
        Some(Json(req)) => req,
        None => SessionCapabilitiesRequest {
            playable_media_types: list_param(&q, "playableMediaTypes"),
            supported_commands: list_param(&q, "supportedCommands"),
            unsupported_commands: None,
            supported_functions: None,
            supports_media_control: parsed(&q, "supportsMediaControl")?,
            supports_persistent_identifier: parsed(&q, "supportsPersistentIdentifier")?,
            device_profile: None,
        },
    };
    let caps = build_capabilities_from_request(&req);
    let token = token.as_ref().map(|Extension(rt)| rt.0.as_str());
    update_user_capabilities(&state, claims.sub, token, &caps).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// POST /Sessions/Capabilities/Full: the capabilities as a JSON body.
async fn session_capabilities_full(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    token: Option<Extension<rustyfin_auth::RequestToken>>,
    Json(req): Json<SessionCapabilitiesRequest>,
) -> Result<StatusCode, ApiError> {
    let caps = build_capabilities_from_request(&req);
    let token = token.as_ref().map(|Extension(rt)| rt.0.as_str());
    update_user_capabilities(&state, claims.sub, token, &caps).await?;
    Ok(StatusCode::NO_CONTENT)
}
