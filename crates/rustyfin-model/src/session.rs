use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SessionInfo {
    pub id: String,
    pub user_id: Uuid,
    pub user_name: String,
    pub device_id: String,
    pub device_name: String,
    pub app_name: String,
    pub app_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub now_playing_item: Option<NowPlayingItem>,
    pub last_activity_date: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct NowPlayingItem {
    pub id: Uuid,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position_ticks: Option<i64>,
    pub is_paused: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub app_name: String,
    pub app_version: String,
    pub last_user_id: Uuid,
    pub last_user_name: String,
    pub date_last_activity: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct DeviceInfoQueryResult {
    pub items: Vec<DeviceInfo>,
    pub total_record_count: usize,
}

/// Parsed from the X-Emby-Authorization header
#[derive(Debug, Clone, Default)]
pub struct ClientInfo {
    pub client: String,
    pub version: String,
    pub device: String,
    pub device_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SessionCommand {
    pub command: String,
    #[serde(default)]
    pub arguments: Option<serde_json::Value>,
}
