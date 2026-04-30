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
