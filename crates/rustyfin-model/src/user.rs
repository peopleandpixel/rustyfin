use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct User {
    pub id: Uuid,
    pub name: String,
    #[serde(skip_serializing)]
    pub password_hash: String,
    pub is_administrator: bool,
    pub created_at: DateTime<Utc>,
    pub last_login_date: Option<DateTime<Utc>>,
    #[serde(default)]
    pub configuration: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct UserDto {
    pub id: Uuid,
    pub name: String,
    pub is_administrator: bool,
    pub last_login_date: Option<DateTime<Utc>>,
    pub configuration: serde_json::Value,
}

impl From<User> for UserDto {
    fn from(u: User) -> Self {
        Self {
            id: u.id,
            name: u.name,
            is_administrator: u.is_administrator,
            last_login_date: u.last_login_date,
            configuration: u.configuration,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PublicUserDto {
    pub id: Uuid,
    pub name: String,
}

impl From<User> for PublicUserDto {
    fn from(u: User) -> Self {
        Self {
            id: u.id,
            name: u.name,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CreateUserRequest {
    pub name: String,
    pub password: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ChangePasswordRequest {
    pub current_pw: String,
    pub new_pw: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct UpdatePolicyRequest {
    pub is_administrator: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ForgotPasswordRequest {
    pub entered_username: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ForgotPasswordResult {
    pub action: String,
}
