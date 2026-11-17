use std::net::SocketAddr;

use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use rustyfin_model::auth::{AuthResponse, AuthSessionInfo, LoginRequest, SetupCodeLoginRequest};
use rustyfin_model::user::{User, UserDto};
use uuid::Uuid;

use super::quick_connect;
use crate::error::ApiError;
use crate::oidc::{PASSWORD_SIGN_IN_OFF, password_allowed};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/Users/AuthenticateByName", post(login))
        .route(
            "/Users/AuthenticateWithQuickConnect",
            post(quick_connect_sign_in),
        )
        .route(
            "/Users/AuthenticateWithSetupCode",
            post(login_with_setup_code),
        )
}

/// Set on a failed sign-in whose password is a pending user's one-time code (B20 3.2).
const SETUP_REQUIRED_HEADER: &str = "x-rustyfin-setup-required";

async fn login(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<LoginRequest>,
) -> Result<Response, ApiError> {
    let device = token_device(&headers);
    let user = match rustyfin_auth::check_password(&state.db, &req.username, &req.password).await {
        Ok(user) => user,
        Err(rustyfin_auth::AuthError::InvalidCredentials) => {
            let name: String = req.username.chars().take(64).collect();
            super::activity::record(
                &state,
                &format!("Failed login try from {name}"),
                "AuthenticationFailed",
                None,
                "Error",
            )
            .await;
            // The password may be a pending user's one-time code: the header says so (B20 3.2).
            // It goes only to a user who may use a password, so a refused code gets no header (X1).
            let config = rustyfin_db::repo_oidc::get_settings(&state.db).await?;
            let pending =
                rustyfin_auth::pending_setup_user(&state.db, &req.username, &req.password).await?;
            let Some(user) = pending else {
                return Err(ApiError::Unauthorized);
            };
            if !password_allowed(&state.db, &config, &user).await? {
                return Err(ApiError::Unauthorized);
            }
            let mut response = ApiError::Unauthorized.into_response();
            response
                .headers_mut()
                .insert(SETUP_REQUIRED_HEADER, HeaderValue::from_static("true"));
            return Ok(response);
        }
        Err(e) => return Err(ApiError::from(e)),
    };
    // While password sign-in is off, only an administrator whose rights were given by hand may
    // sign in with a password (AC21, AC41).
    let config = rustyfin_db::repo_oidc::get_settings(&state.db).await?;
    if !password_allowed(&state.db, &config, &user).await? {
        return Err(ApiError::Forbidden(PASSWORD_SIGN_IN_OFF.to_string()));
    }
    let local = is_local(&state, peer, &headers);
    crate::parental::check_sign_in(&state, &user, &device.device_id, local).await?;
    let (user, token) = rustyfin_auth::issue_login_token(&state.db, user.id, &device).await?;
    Ok(Json(start_session(&state, &device, user, token).await).into_response())
}

/// `POST /Users/AuthenticateWithQuickConnect` – signs in with the secret of a Quick Connect request
/// that a user approved, as that user (AC26). Public, but behind the login rate limit.
async fn quick_connect_sign_in(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<AuthResponse>, ApiError> {
    let secret = body
        .get("Secret")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| ApiError::BadRequest("Secret is required.".to_string()))?;
    let approver = quick_connect::redeem(&state, secret).await?;
    let device = token_device(&headers);
    if let Some(user) = rustyfin_db::repo_user::get_user_by_id(&state.db, approver).await? {
        let local = is_local(&state, peer, &headers);
        crate::parental::check_sign_in(&state, &user, &device.device_id, local).await?;
    }
    let (user, token) = rustyfin_auth::authenticate_approved(&state.db, approver, &device).await?;
    Ok(Json(start_session(&state, &device, user, token).await))
}

/// `POST /Users/AuthenticateWithSetupCode` (B20 3.3): a pending user's code sets the new password
/// and signs in. It is routed with the login endpoints, so the login rate limit covers it.
async fn login_with_setup_code(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<SetupCodeLoginRequest>,
) -> Result<Json<AuthResponse>, ApiError> {
    if req.new_pw.is_empty() {
        return Err(ApiError::BadRequest("NewPw is required.".to_string()));
    }
    let device = token_device(&headers);
    let verified = rustyfin_auth::accept_setup_code(&state.db, &req.username, &req.code).await?;
    // The password switch applies as at login. A refused request leaves the code unused (X1).
    let config = rustyfin_db::repo_oidc::get_settings(&state.db).await?;
    if !password_allowed(&state.db, &config, verified.user()).await? {
        return Err(ApiError::Forbidden(PASSWORD_SIGN_IN_OFF.to_string()));
    }
    let local = is_local(&state, peer, &headers);
    crate::parental::check_sign_in(&state, verified.user(), &device.device_id, local).await?;
    let (user, token) =
        rustyfin_auth::redeem_setup_code(&state.db, verified, &req.new_pw, &device).await?;
    Ok(Json(start_session(&state, &device, user, token).await))
}

/// Whether a sign-in comes from the local network.
fn is_local(state: &AppState, peer: SocketAddr, headers: &HeaderMap) -> bool {
    crate::middleware::resolve_client(peer.ip(), headers, &state.known_proxies).is_local()
}

/// The device of a sign-in, from its `MediaBrowser` header, read by the same parser as every
/// later request (`middleware::media_browser_header`), so the stored device name and id match
/// what those requests state. Fields the header lacks are empty.
pub(crate) fn token_device(headers: &HeaderMap) -> rustyfin_auth::TokenDevice {
    let auth = crate::middleware::media_browser_header(headers).unwrap_or_default();
    rustyfin_auth::TokenDevice {
        device_id: auth.device_id.unwrap_or_default(),
        device_name: auth.device.unwrap_or_default(),
        app_name: auth.client.unwrap_or_default(),
        app_version: auth.version.unwrap_or_default(),
    }
}

/// Starts the session of a sign-in on `device` and builds the answer that every sign-in returns.
pub(crate) async fn start_session(
    state: &AppState,
    device: &rustyfin_auth::TokenDevice,
    user: User,
    token: String,
) -> AuthResponse {
    // A user keeps their session of a client on a device across sign-ins. Another user, or another
    // client, signing in with the same device id gets a session of its own, never this one.
    let existing = match device.device_id.as_str() {
        "" => None,
        id => rustyfin_db::repo_session::find_device_session(
            &state.db,
            user.id,
            id,
            Some(&device.app_name),
        )
        .await
        .ok()
        .flatten(),
    };
    // The user's other sessions of this client on the device are replaced below; their play state
    // goes with them
    if !device.device_id.is_empty() {
        let ids = rustyfin_db::repo_session::app_session_ids(
            &state.db,
            user.id,
            &device.device_id,
            &device.app_name,
        )
        .await
        .unwrap_or_default();
        for id in ids.iter().filter(|id| Some(*id) != existing.as_ref()) {
            crate::sessions::forget(state, id);
        }
    }
    let session_id = existing
        .and_then(|id| Uuid::parse_str(&id).ok())
        .unwrap_or_else(Uuid::new_v4);
    super::activity::record(
        state,
        &format!("{} successfully authenticated", user.name),
        "AuthenticationSucceeded",
        Some(user.id),
        "Information",
    )
    .await;

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
    crate::sessions::changed(state);

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

    let parental = rustyfin_db::repo_parental::get_policy(&state.db, user.id)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "could not read the user's parental policy for the sign-in answer");
            Default::default()
        });

    let primary_image_tag = super::users::primary_image_tag(&state.db, user.id)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "could not read the user's picture for the sign-in answer");
            None
        });

    AuthResponse {
        user: UserDto {
            primary_image_tag,
            ..UserDto::new(user, flags, &state.server_id, &parental)
        },
        session_info,
        access_token: token,
        server_id: state.server_id.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;

    fn device(headers: &[(&'static str, &str)]) -> rustyfin_auth::TokenDevice {
        let mut map = HeaderMap::new();
        for (name, value) in headers {
            map.insert(*name, value.parse().unwrap());
        }
        token_device(&map)
    }

    /// Sign-in reads the header like every later request: a comma inside quotes belongs to the
    /// value, and values are URL-decoded (`+` and `%XX`).
    #[test]
    fn r3security_sign_in_parses_quoted_commas_and_encoded_values() {
        let d = device(&[(
            "X-Emby-Authorization",
            r#"MediaBrowser Client="App, Inc", Device="My%20Phone, 2", DeviceId="a%2Bb%3D%26", Version="1.0+beta""#,
        )]);
        assert_eq!(d.app_name, "App, Inc");
        assert_eq!(d.device_name, "My Phone, 2");
        assert_eq!(d.device_id, "a+b=&");
        assert_eq!(d.app_version, "1.0 beta");
        // Non-ASCII, URL-encoded as Jellyfin clients send it
        let d = device(&[(
            "Authorization",
            r#"MediaBrowser Client="Gr%C3%B6%C3%9Fe", DeviceId="d1""#,
        )]);
        assert_eq!(d.app_name, "Größe");
        assert_eq!(d.device_id, "d1");
    }

    /// What sign-in stores is what the middleware states for the request's device, for the same
    /// header, so later requests find the session they signed in with.
    #[test]
    fn r3security_sign_in_and_middleware_read_the_same_device() {
        let header =
            r#"MediaBrowser Client="A, B", Device="C%2CD", DeviceId="x+y", Version="2", Token="t""#;
        let mut map = HeaderMap::new();
        map.insert("Authorization", header.parse().unwrap());
        let signed_in = token_device(&map);
        let auth = crate::middleware::media_browser_header(&map).unwrap();
        assert_eq!(Some(signed_in.device_id), auth.device_id);
        assert_eq!(Some(signed_in.device_name), auth.device);
        assert_eq!(Some(signed_in.app_name), auth.client);
        assert_eq!(Some(signed_in.app_version), auth.version);
    }

    /// `Authorization` comes first, as in Jellyfin's `AuthorizationContext`; a `Bearer` value there
    /// leaves `X-Emby-Authorization` to name the device. Another scheme names none.
    #[test]
    fn r3security_sign_in_header_order_and_scheme() {
        let d = device(&[
            ("X-Emby-Authorization", r#"MediaBrowser Client="Emby""#),
            ("Authorization", r#"MediaBrowser Client="Auth""#),
        ]);
        assert_eq!(d.app_name, "Auth");
        let d = device(&[
            ("Authorization", "Bearer abc"),
            ("X-Emby-Authorization", r#"Emby Client="Emby""#),
        ]);
        assert_eq!(d.app_name, "Emby");
        assert_eq!(
            device(&[("Authorization", r#"Client="NoScheme""#)]).app_name,
            ""
        );
        assert_eq!(device(&[]).device_id, "");
    }

    #[test]
    fn test_token_device_full_and_partial() {
        let d = device(&[(
            "x-emby-authorization",
            r#"MediaBrowser Client="Jellyfin MPV", Device="MPV", DeviceId="test123", Version="1.0.0", Token="abc""#,
        )]);
        assert_eq!(d.app_name, "Jellyfin MPV");
        assert_eq!(d.app_version, "1.0.0");
        assert_eq!(d.device_name, "MPV");
        assert_eq!(d.device_id, "test123");
        let d = device(&[(
            "X-Emby-Authorization",
            r#"MediaBrowser Client="OnlyClient""#,
        )]);
        assert_eq!(d.app_name, "OnlyClient");
        assert_eq!(
            (d.app_version, d.device_name, d.device_id),
            Default::default()
        );
        assert_eq!(device(&[("X-Emby-Authorization", "")]).app_name, "");
    }
}
