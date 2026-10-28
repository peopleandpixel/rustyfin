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

/// Client capabilities reported by Jellyfin clients.
/// Clients send this via POST /Sessions/Capabilities or /Sessions/Capabilities/Full.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct ClientCapabilities {
    /// Whether the client supports playing media
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub playable_media_types: Option<Vec<String>>,
    /// Whether the client supports remote control commands
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_media_control: Option<bool>,
    /// Whether the client supports remote control
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_remote_control: Option<bool>,
    /// Whether the client supports gapless playback
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_gapless_media_playback: Option<bool>,
    /// Supported command types
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supported_commands: Option<Vec<String>>,
    /// Whether the client supports content uploading
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_content_upload: Option<bool>,
    /// Whether the client supports album art
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_album_art: Option<bool>,
    /// Whether the client supports pairing
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_pairing: Option<bool>,
    /// Whether the client supports persistent identifier
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_persistent_identifier: Option<bool>,
    /// Whether the client supports audio playback
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_audio_playback: Option<bool>,
    /// Whether the client supports video playback
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_video_playback: Option<bool>,
    /// Whether the client supports photo playback
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_photo_playback: Option<bool>,
    /// Device profile for transcoding hints
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_profile: Option<serde_json::Value>,
}

/// Request body for POST /Sessions/Capabilities
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SessionCapabilitiesRequest {
    #[serde(default)]
    pub playable_media_types: Option<Vec<String>>,
    #[serde(default)]
    pub supported_commands: Option<Vec<String>>,
    #[serde(default)]
    pub unsupported_commands: Option<Vec<String>>,
    #[serde(default)]
    pub supported_functions: Option<Vec<String>>,
    #[serde(default)]
    pub device_profile: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SessionCommand {
    pub command: String,
    #[serde(default)]
    pub arguments: Option<serde_json::Value>,
}
