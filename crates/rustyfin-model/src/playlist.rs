use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PlaylistDto {
    pub id: Uuid,
    pub name: String,
    pub user_id: Uuid,
    pub media_type: Option<String>,
    pub child_count: i64,
    pub date_created: DateTime<Utc>,
    /// Jellyfin's public playlist: every user may read it.
    #[serde(default)]
    pub open_access: bool,
    /// The users it is shared with (Jellyfin's `Shares`).
    #[serde(default)]
    pub shares: Vec<PlaylistUserPermissions>,
    /// Its item ids in order, as Jellyfin's `GET /Playlists/{id}` gives them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_ids: Option<Vec<Uuid>>,
    /// Whether the caller may change it: its owner, a user it is shared with for editing, or an
    /// administrator. A Rustyfin extra, so clients needn't ask `/Playlists/{id}/Users/{userId}`.
    #[serde(default)]
    pub can_edit: bool,
}

/// One user a playlist is shared with (Jellyfin's `PlaylistUserPermissions`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PlaylistUserPermissions {
    pub user_id: Uuid,
    pub can_edit: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CreatePlaylistRequest {
    pub name: String,
    #[serde(default)]
    pub ids: Option<Vec<Uuid>>,
    #[serde(default)]
    pub media_type: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PlaylistAddItemsRequest {
    pub ids: Vec<Uuid>,
}
