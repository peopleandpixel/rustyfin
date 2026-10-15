use axum::extract::State;
use axum::http::HeaderMap;
use axum::routing::post;
use axum::{Json, Router};
use rustyfin_model::auth::{AuthResponse, AuthSessionInfo, LoginRequest};
use rustyfin_model::user::UserDto;
use uuid::Uuid;

use crate::error::ApiError;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/Users/AuthenticateByName", post(login))
}

async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<LoginRequest>,
) -> Result<Json<AuthResponse>, ApiError> {
    let (user, token) =
        rustyfin_auth::authenticate(&state.db, &req.username, &req.password, &state.jwt_secret)
            .await?;

    let (client, version, device, device_id) = parse_emby_auth(&headers);
    let session_id = Uuid::new_v4();

    // Create session in database
    rustyfin_db::repo_session::create_session(
        &state.db, session_id, user.id, &user.name, &client, &device, &device_id, &version,
    )
    .await
    .ok();

    let session_info = AuthSessionInfo {
        id: session_id.to_string(),
        user_id: user.id,
        user_name: user.name.clone(),
        client,
        device_name: device,
        device_id,
        application_version: version,
        is_active: true,
        server_id: state.server_id.to_string(),
        supports_media_control: false,
        supports_remote_control: false,
        has_custom_device_name: false,
        playable_media_types: Vec::new(),
        playlist_item_id: None,
        now_playing_queue: Vec::new(),
        now_playing_queue_full_items: Vec::new(),
        additional_users: Vec::new(),
        capabilities: serde_json::json!({}),
    };

    Ok(Json(AuthResponse {
        user: UserDto::from(user),
        session_info,
        access_token: token,
        server_id: state.server_id.to_string(),
    }))
}

/// Parst den `X-Emby-Authorization`/`Authorization` Header.
/// Format: `MediaBrowser Client="...", Device="...", DeviceId="...", Version="...", Token="..."`
fn parse_emby_auth(headers: &HeaderMap) -> (String, String, String, String) {
    let raw = headers
        .get("X-Emby-Authorization")
        .or_else(|| headers.get("x-emby-authorization"))
        .or_else(|| headers.get("Authorization"))
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let mut client = String::new();
    let mut version = String::new();
    let mut device = String::new();
    let mut device_id = String::new();
    for part in raw.split(',') {
        let part = part.trim();
        let part = part.strip_prefix("MediaBrowser").unwrap_or(part).trim();
        if let Some((k, v)) = part.split_once('=') {
            let val = v.trim().trim_matches('"').to_string();
            match k.trim() {
                "Client" => client = val,
                "Device" => device = val,
                "DeviceId" => device_id = val,
                "Version" => version = val,
                _ => {}
            }
        }
    }
    (client, version, device, device_id)
}
