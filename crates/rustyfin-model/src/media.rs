use chrono::{DateTime, Utc};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize, Serializer};
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

impl MediaType {
    /// Jellyfin `BaseItemKind` value (e.g. "Movie", "Series", "MusicAlbum").
    pub fn item_kind(self) -> &'static str {
        match self {
            MediaType::Movie => "Movie",
            MediaType::Series => "Series",
            MediaType::Season => "Season",
            MediaType::Episode => "Episode",
            MediaType::Audio => "Audio",
            MediaType::MusicAlbum => "MusicAlbum",
            MediaType::Photo => "Photo",
            MediaType::Unknown => "Folder",
        }
    }

    /// Jellyfin `MediaType` value (only Video/Audio/Photo/Book/Unknown).
    pub fn media_kind(self) -> &'static str {
        match self {
            MediaType::Movie | MediaType::Episode => "Video",
            MediaType::Series | MediaType::Season => "Unknown",
            MediaType::Audio | MediaType::MusicAlbum => "Audio",
            MediaType::Photo => "Photo",
            MediaType::Unknown => "Unknown",
        }
    }

    pub fn is_folder(self) -> bool {
        matches!(
            self,
            MediaType::Series | MediaType::Season | MediaType::MusicAlbum
        )
    }
}

#[derive(Debug, Clone, Deserialize)]
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

impl Serialize for MediaItem {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("MediaItem", 19)?;
        s.serialize_field("Id", &self.id)?;
        s.serialize_field("Name", &self.name)?;
        s.serialize_field("SortName", &self.sort_name)?;
        s.serialize_field("Overview", &self.overview)?;
        // Jellyfin "Type" (BaseItemKind) and "MediaType" (Video/Audio/Photo/...)
        s.serialize_field("Type", self.media_type.item_kind())?;
        s.serialize_field("MediaType", self.media_type.media_kind())?;
        s.serialize_field("IsFolder", &self.media_type.is_folder())?;
        s.serialize_field("Path", &self.path)?;
        s.serialize_field("ParentId", &self.parent_id)?;
        s.serialize_field("LibraryId", &self.library_id)?;
        s.serialize_field("ProductionYear", &self.production_year)?;
        s.serialize_field("CommunityRating", &self.community_rating)?;
        s.serialize_field("OfficialRating", &self.official_rating)?;
        s.serialize_field("Genres", &self.genres)?;
        s.serialize_field("DateCreated", &self.date_created)?;
        s.serialize_field("PremiereDate", &self.premiere_date)?;
        s.serialize_field("RunTimeTicks", &self.runtime_ticks)?;
        s.serialize_field("IndexNumber", &self.index_number)?;
        s.serialize_field("ParentIndexNumber", &self.parent_index_number)?;
        s.end()
    }
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
