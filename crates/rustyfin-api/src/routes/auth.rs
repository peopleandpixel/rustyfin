use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use rustyfin_model::auth::{AuthResponse, AuthSessionInfo, LoginRequest, SetupCodeLoginRequest};
use rustyfin_model::user::{User, UserDto};
use uuid::Uuid;

use crate::error::ApiError;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/Users/AuthenticateByName", post(login))
        .route(
            "/Users/AuthenticateWithSetupCode",
            post(login_with_setup_code),
        )
}

/// Set on a failed sign-in whose password is a pending user's one-time code (B20 3.2).
const SETUP_REQUIRED_HEADER: &str = "x-rustyfin-setup-required";

async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<LoginRequest>,
) -> Result<Response, ApiError> {
    let device = token_device(&headers);
    match rustyfin_auth::authenticate(&state.db, &req.username, &req.password, &device).await {
        Ok((user, token)) => {
            let answer = start_session(&state, &device, user, token).await;
            Ok(Json(answer).into_response())
        }
        Err(rustyfin_auth::AuthError::InvalidCredentials) => {
            // The password may be a pending user's one-time code: the header says so (B20 3.2).
            if !rustyfin_auth::setup_code_pending(&state.db, &req.username, &req.password).await? {
                return Err(ApiError::Unauthorized);
            }
            let mut response = ApiError::Unauthorized.into_response();
            response
                .headers_mut()
                .insert(SETUP_REQUIRED_HEADER, HeaderValue::from_static("true"));
            Ok(response)
        }
        Err(e) => Err(ApiError::from(e)),
    }
}

/// `POST /Users/AuthenticateWithSetupCode` (B20 3.3): a pending user's code sets the new password
/// and signs in. It is routed with the login endpoints, so the login rate limit covers it.
async fn login_with_setup_code(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<SetupCodeLoginRequest>,
) -> Result<Json<AuthResponse>, ApiError> {
    if req.new_pw.is_empty() {
        return Err(ApiError::BadRequest("NewPw is required.".to_string()));
    }
    let device = token_device(&headers);
    let (user, token) = rustyfin_auth::authenticate_with_setup_code(
        &state.db,
        &req.username,
        &req.code,
        &req.new_pw,
        &device,
    )
    .await?;
    Ok(Json(start_session(&state, &device, user, token).await))
}

/// The token device of a sign-in request, from its Emby authorization headers.
fn token_device(headers: &HeaderMap) -> rustyfin_auth::TokenDevice {
    let (client, version, device, device_id) = parse_emby_auth(headers);
    rustyfin_auth::TokenDevice {
        device_id,
        device_name: device,
        app_name: client,
        app_version: version,
    }
}

/// Records the session of a new sign-in and builds its answer. Both sign-in routes end here.
async fn start_session(
    state: &AppState,
    device: &rustyfin_auth::TokenDevice,
    user: User,
    token: String,
) -> AuthResponse {
    let session_id = Uuid::new_v4();

    // Create session in database with empty capabilities (will be updated by client)
    rustyfin_db::repo_session::create_session(
        &state.db,
        session_id,
        user.id,
        &user.name,
        &device.app_name,
        &device.device_name,
        &device.device_id,
        &device.app_version,
        "{}",
    )
    .await
    .ok();

    let session_info = AuthSessionInfo {
        id: session_id.to_string(),
        user_id: user.id,
        user_name: user.name.clone(),
        client: device.app_name.clone(),
        device_name: device.device_name.clone(),
        device_id: device.device_id.clone(),
        application_version: device.app_version.clone(),
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

    let flags = rustyfin_db::repo_user::user_flags(&state.db, user.id)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "could not read the user's flags for the sign-in answer");
            Default::default()
        });

    AuthResponse {
        user: UserDto::new(user, flags),
        session_info,
        access_token: token,
        server_id: state.server_id.to_string(),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;

    #[test]
    fn test_parse_emby_auth_full() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "X-Emby-Authorization",
            "MediaBrowser Client=\"Jellyfin MPV\", Device=\"MPV\", DeviceId=\"test123\", Version=\"1.0.0\", Token=\"abc\""
                .parse()
                .unwrap(),
        );
        let (client, version, device, device_id) = parse_emby_auth(&headers);
        assert_eq!(client, "Jellyfin MPV");
        assert_eq!(version, "1.0.0");
        assert_eq!(device, "MPV");
        assert_eq!(device_id, "test123");
    }

    #[test]
    fn test_parse_emby_auth_lowercase_header() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-emby-authorization",
            "MediaBrowser Client=\"Test\", Device=\"Dev\", DeviceId=\"d1\", Version=\"2.0\""
                .parse()
                .unwrap(),
        );
        let (client, version, device, device_id) = parse_emby_auth(&headers);
        assert_eq!(client, "Test");
        assert_eq!(device, "Dev");
        assert_eq!(device_id, "d1");
        assert_eq!(version, "2.0");
    }

    #[test]
    fn test_parse_emby_auth_authorization_fallback() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "Authorization",
            "MediaBrowser Client=\"Fallback\", Device=\"Dev\", DeviceId=\"d1\", Version=\"1.0\""
                .parse()
                .unwrap(),
        );
        let (client, _version, device, _device_id) = parse_emby_auth(&headers);
        assert_eq!(client, "Fallback");
        assert_eq!(device, "Dev");
    }

    #[test]
    fn test_parse_emby_auth_partial_fields() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "X-Emby-Authorization",
            "MediaBrowser Client=\"OnlyClient\"".parse().unwrap(),
        );
        let (client, version, device, device_id) = parse_emby_auth(&headers);
        assert_eq!(client, "OnlyClient");
        assert_eq!(version, "");
        assert_eq!(device, "");
        assert_eq!(device_id, "");
    }

    #[test]
    fn test_parse_emby_auth_empty_header() {
        let mut headers = HeaderMap::new();
        headers.insert("X-Emby-Authorization", "".parse().unwrap());
        let (client, version, device, device_id) = parse_emby_auth(&headers);
        assert_eq!(client, "");
        assert_eq!(version, "");
        assert_eq!(device, "");
        assert_eq!(device_id, "");
    }

    #[test]
    fn test_parse_emby_auth_missing_header() {
        let headers = HeaderMap::new();
        let (client, version, device, device_id) = parse_emby_auth(&headers);
        assert_eq!(client, "");
        assert_eq!(version, "");
        assert_eq!(device, "");
        assert_eq!(device_id, "");
    }

    #[test]
    fn test_parse_emby_auth_without_mediabrowser_prefix() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "X-Emby-Authorization",
            "Client=\"NoPrefix\", Device=\"Dev\", DeviceId=\"d1\", Version=\"1.0\""
                .parse()
                .unwrap(),
        );
        let (client, _version, device, _device_id) = parse_emby_auth(&headers);
        assert_eq!(client, "NoPrefix");
        assert_eq!(device, "Dev");
    }

    #[test]
    fn test_parse_emby_auth_x_emby_wins_over_authorization() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "X-Emby-Authorization",
            "MediaBrowser Client=\"EmbyHeader\"".parse().unwrap(),
        );
        headers.insert(
            "Authorization",
            "MediaBrowser Client=\"AuthHeader\"".parse().unwrap(),
        );
        let (client, _version, _device, _device_id) = parse_emby_auth(&headers);
        assert_eq!(client, "EmbyHeader");
    }
}
