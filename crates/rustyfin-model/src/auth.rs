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
    pub access_token: String,
    pub server_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: Uuid,
    pub name: String,
    pub is_admin: bool,
    pub exp: i64,
    pub iat: i64,
}
