use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// One `/Search/Hints` entry with Jellyfin's field names. An `Option` that is `None` is left out
/// of the JSON.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SearchHint {
    pub item_id: Uuid,
    pub id: Uuid,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index_number: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub production_year: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_index_number: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_image_tag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumb_image_tag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumb_image_item_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backdrop_image_tag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backdrop_image_item_id: Option<String>,
    #[serde(rename = "Type")]
    pub item_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_folder: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_time_ticks: Option<i64>,
    pub media_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_date: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album_artist: Option<String>,
    #[serde(default)]
    pub artists: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_image_aspect_ratio: Option<f64>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SearchHintResult {
    pub search_hints: Vec<SearchHint>,
    pub total_record_count: i64,
}
