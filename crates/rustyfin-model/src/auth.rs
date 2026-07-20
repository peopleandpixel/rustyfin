use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct LoginRequest {
    pub username: String,
    #[serde(rename = "Pw")]
    pub password: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct AuthResponse {
    pub user: crate::user::UserDto,
    pub session_info: AuthSessionInfo,
    pub access_token: String,
    pub server_id: String,
}

#[derive(Debug, Serialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct AuthSessionInfo {
    pub id: String,
    pub user_id: Uuid,
    pub user_name: String,
    pub client: String,
    pub device_name: String,
    pub device_id: String,
    pub application_version: String,
    pub is_active: bool,
    pub server_id: String,
    pub supports_media_control: bool,
    pub supports_remote_control: bool,
    pub has_custom_device_name: bool,
    pub playable_media_types: Vec<String>,
    pub playlist_item_id: Option<String>,
    pub now_playing_queue: Vec<serde_json::Value>,
    pub now_playing_queue_full_items: Vec<serde_json::Value>,
    pub additional_users: Vec<serde_json::Value>,
    pub capabilities: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: Uuid,
    pub name: String,
    pub is_admin: bool,
    pub exp: i64,
    pub iat: i64,
}
