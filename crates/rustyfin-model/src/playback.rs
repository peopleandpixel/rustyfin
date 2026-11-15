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
    #[serde(default)]
    pub audio_stream_index: Option<i32>,
    #[serde(default)]
    pub subtitle_stream_index: Option<i32>,
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
    #[serde(default)]
    pub audio_stream_index: Option<i32>,
    #[serde(default)]
    pub subtitle_stream_index: Option<i32>,
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
    #[serde(default)]
    pub audio_stream_index: Option<i32>,
    #[serde(default)]
    pub subtitle_stream_index: Option<i32>,
    /// The playback errored out; Jellyfin then leaves the item's user data alone.
    #[serde(default)]
    pub failed: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
    /// 100 * position / runtime, when there is a position and a known runtime
    #[serde(skip_serializing_if = "Option::is_none")]
    pub played_percentage: Option<f64>,
    /// The item's id, as Jellyfin's UserItemDataDto.ItemId gives it, in the form item ids take
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
}

impl UserItemDataDto {
    /// Fills in the `Key` and `ItemId` Jellyfin gives every user data answer. Rustyfin keys user
    /// data by item id, so both are that id, hyphenated as item ids are, which clients match
    /// `ItemId` against.
    pub fn for_item(mut self, item_id: uuid::Uuid) -> Self {
        let id = item_id.hyphenated().to_string();
        self.key = Some(id.clone());
        self.item_id = Some(id);
        self
    }
}

/// Reads a date a client sends for a play: RFC 3339 with an offset, an ISO date-time without one
/// (Jellyfin's Kotlin SDK), or jellyfin-apiclient's `yyyyMMddHHmmss`; the forms without an offset
/// are taken as UTC. An offset's `+` left unescaped in a query string arrives as a space and is read
/// back. Anything else is None, so the caller uses its default, as Jellyfin's binder does.
pub fn parse_client_date(value: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    use chrono::{DateTime, NaiveDateTime, Utc};
    let value = value.trim();
    for candidate in [value.to_string(), value.replace(' ', "+")] {
        if let Ok(date) = DateTime::parse_from_rfc3339(&candidate) {
            return Some(date.with_timezone(&Utc));
        }
    }
    [
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y%m%d%H%M%S",
    ]
    .iter()
    .find_map(|format| NaiveDateTime::parse_from_str(value, format).ok())
    .map(|naive| naive.and_utc())
}

/// The fields `POST /UserItems/{itemId}/UserData` may set (Jellyfin's UpdateUserItemDataDto).
/// Each field that is present replaces the stored value; absent ones are kept.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct UpdateUserItemDataDto {
    #[serde(default)]
    pub playback_position_ticks: Option<i64>,
    #[serde(default)]
    pub play_count: Option<i64>,
    #[serde(default)]
    pub is_favorite: Option<bool>,
    #[serde(default)]
    pub likes: Option<bool>,
    #[serde(default)]
    pub played: Option<bool>,
    #[serde(default)]
    pub last_played_date: Option<String>,
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
