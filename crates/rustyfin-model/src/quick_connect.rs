use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct QuickConnectResult {
    pub authenticated: bool,
    pub secret: String,
    pub code: String,
    pub device_id: String,
    pub device_name: String,
    pub app_name: String,
    pub app_version: String,
    pub date_added: DateTime<Utc>,
    /// Set once authorized – used internally to issue a token.
    #[serde(skip)]
    pub user_id: Option<Uuid>,
}
