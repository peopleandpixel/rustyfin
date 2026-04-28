use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MediaType {
    Movie,
    Series,
    Season,
    Episode,
    Audio,
    MusicAlbum,
    Photo,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MediaItem {
    pub id: Uuid,
    pub name: String,
    pub sort_name: Option<String>,
    pub overview: Option<String>,
    pub media_type: MediaType,
    pub path: String,
    pub parent_id: Option<Uuid>,
    pub library_id: Uuid,
    pub production_year: Option<i32>,
    pub community_rating: Option<f64>,
    pub official_rating: Option<String>,
    pub genres: Vec<String>,
    pub date_created: DateTime<Utc>,
    pub premiere_date: Option<DateTime<Utc>>,
    /// Runtime in ticks (1 tick = 100 nanoseconds, Jellyfin convention)
    pub runtime_ticks: Option<i64>,
    pub index_number: Option<i32>,
    pub parent_index_number: Option<i32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct QueryResult {
    pub items: Vec<MediaItem>,
    pub total_record_count: i64,
    pub start_index: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct FiltersResult {
    pub genres: Vec<String>,
    pub years: Vec<i32>,
    pub official_ratings: Vec<String>,
}
