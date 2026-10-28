use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PlaybackStartInfo {
    pub item_id: Uuid,
    #[serde(default)]
    pub item_name: Option<String>,
    #[serde(default)]
    pub position_ticks: Option<i64>,
    #[serde(default)]
    pub media_source_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PlaybackProgressInfo {
    pub item_id: Uuid,
    #[serde(default)]
    pub item_name: Option<String>,
    #[serde(default)]
    pub position_ticks: Option<i64>,
    #[serde(default)]
    pub is_paused: Option<bool>,
    #[serde(default)]
    pub media_source_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PlaybackStopInfo {
    pub item_id: Uuid,
    #[serde(default)]
    pub item_name: Option<String>,
    #[serde(default)]
    pub position_ticks: Option<i64>,
    #[serde(default)]
    pub media_source_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct UserItemDataDto {
    pub playback_position_ticks: i64,
    pub play_count: i64,
    pub is_favorite: bool,
    pub played: bool,
    pub last_played_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub likes: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_rating: Option<i32>,
    /// Unique key for this user/item combination
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// User's rating (0-10)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rating: Option<f64>,
    /// Unplayed item count for folders
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unplayed_item_count: Option<i32>,
    /// Favorite flag (alias for is_favorite)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub favorite: Option<bool>,
}

/// Request body for PinEpisode endpoint
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PinEpisodeRequest {
    pub item_id: Uuid,
}

/// Request body for LastActivityDate endpoint
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct LastActivityDateRequest {
    pub item_ids: Vec<Uuid>,
}

/// Request body for setting user rating
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SetRatingRequest {
    pub likes: Option<bool>,
    pub user_rating: Option<i32>,
}
