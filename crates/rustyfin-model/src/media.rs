use chrono::{DateTime, Utc};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize, Serializer};
use uuid::Uuid;

use crate::image::ImageType;
use crate::media_source::MediaSourceResponse;
use crate::playback::UserItemDataDto;

/// Person (cast/crew) entry on a media item.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PersonInfo {
    pub name: String,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Type of person, e.g. "Actor", "Director"
    #[serde(rename = "Type", skip_serializing_if = "Option::is_none")]
    pub person_type: Option<String>,
}

/// Studio entry on a media item.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct StudioInfo {
    pub name: String,
    pub id: String,
}

/// Genre item with ID.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct GenreItem {
    pub name: String,
    pub id: String,
}

/// Location type for media items.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum LocationType {
    #[default]
    FileSystem,
    Virtual,
    Remote,
}

impl LocationType {
    pub fn as_str(self) -> &'static str {
        match self {
            LocationType::FileSystem => "FileSystem",
            LocationType::Virtual => "Virtual",
            LocationType::Remote => "Remote",
        }
    }
}

/// Video type for media items.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum VideoType {
    #[default]
    VideoFile,
    Iso,
    Ifo,
}

impl VideoType {
    pub fn as_str(self) -> &'static str {
        match self {
            VideoType::VideoFile => "VideoFile",
            VideoType::Iso => "Iso",
            VideoType::Ifo => "Ifo",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum MediaType {
    Movie,
    Series,
    Season,
    Episode,
    Audio,
    MusicAlbum,
    Photo,
    Genre,
    Unknown,
}

impl std::str::FromStr for MediaType {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Movie" => Ok(MediaType::Movie),
            "Series" => Ok(MediaType::Series),
            "Season" => Ok(MediaType::Season),
            "Episode" => Ok(MediaType::Episode),
            "Audio" => Ok(MediaType::Audio),
            "MusicAlbum" => Ok(MediaType::MusicAlbum),
            "Photo" => Ok(MediaType::Photo),
            "Genre" => Ok(MediaType::Genre),
            _ => Ok(MediaType::Unknown),
        }
    }
}

impl std::fmt::Display for MediaType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.item_kind())
    }
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
            MediaType::Genre => "Genre",
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
            MediaType::Genre => "Unknown",
            MediaType::Unknown => "Unknown",
        }
    }

    pub fn is_folder(self) -> bool {
        matches!(
            self,
            MediaType::Series | MediaType::Season | MediaType::MusicAlbum | MediaType::Genre
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
    /// Media sources (files) for this item, populated from DB when needed
    #[serde(default)]
    pub media_sources: Vec<MediaSourceResponse>,

    // --- MediaItem DTO Fields ---
    /// Per-user playback state embedded in every item
    #[serde(default)]
    pub user_data: Option<UserItemDataDto>,
    /// Image type to tag/hash map for caching
    #[serde(default)]
    pub image_tags: std::collections::HashMap<ImageType, String>,
    /// Array of backdrop image tags
    #[serde(default)]
    pub backdrop_image_tags: Vec<String>,
    /// Array of parent backdrop image tags
    #[serde(default)]
    pub parent_backdrop_image_tags: Vec<String>,
    /// Primary image aspect ratio (e.g., "1.777777777777778")
    #[serde(default)]
    pub primary_image_aspect_ratio: Option<f64>,
    /// Permission flag: can the current user delete this item
    #[serde(default)]
    pub can_delete: bool,
    /// Permission flag: can the current user download this item
    #[serde(default)]
    pub can_download: bool,
    /// File container (mp4, mkv, etc.)
    #[serde(default)]
    pub container: Option<String>,
    /// Overall bitrate in bps
    #[serde(default)]
    pub bitrate: Option<u32>,
    /// 3D content flag
    #[serde(default)]
    pub video_3_d: bool,
    /// End date for series/seasons
    #[serde(default)]
    pub end_date: Option<DateTime<Utc>>,
    /// Series name for episodes
    #[serde(default)]
    pub series_name: Option<String>,
    /// Series ID for episodes
    #[serde(default)]
    pub series_id: Option<Uuid>,
    /// Season name for episodes
    #[serde(default)]
    pub season_name: Option<String>,
    /// Series status (Continuing, Ended, etc.)
    #[serde(default)]
    pub status: Option<String>,
    /// User's personal rating (1-5 stars)
    #[serde(default)]
    pub custom_rating: Option<f64>,
    /// Critic score (0-10)
    #[serde(default)]
    pub critic_rating: Option<f64>,
    /// Audience score (0-10)
    #[serde(default)]
    pub audience_rating: Option<f64>,
    /// User-defined tags
    #[serde(default)]
    pub tags: Vec<String>,
    /// Cast/crew array
    #[serde(default)]
    pub people: Vec<PersonInfo>,
    /// Studio array
    #[serde(default)]
    pub studios: Vec<StudioInfo>,
    /// Genre objects with IDs
    #[serde(default)]
    pub genre_items: Vec<GenreItem>,
    /// Custom sort name
    #[serde(default)]
    pub forced_sort_name: Option<String>,
    /// Video width in pixels
    #[serde(default)]
    pub width: Option<i32>,
    /// Video height in pixels
    #[serde(default)]
    pub height: Option<i32>,
    /// Video aspect ratio string (e.g., "16:9")
    #[serde(default)]
    pub aspect_ratio: Option<String>,
    /// Number of chapters
    #[serde(default)]
    pub chapter_count: i32,
    /// Number of media sources
    #[serde(default)]
    pub media_source_count: i32,
    /// Location type (FileSystem, Virtual, Remote)
    #[serde(default)]
    pub location_type: LocationType,
    /// Intro detection start time in ticks
    #[serde(default)]
    pub intro_start_time_ticks: Option<i64>,
    /// Intro detection end time in ticks
    #[serde(default)]
    pub intro_end_time_ticks: Option<i64>,
    /// HD content flag (1080p)
    #[serde(default)]
    pub is_hd: bool,
    /// 4K content flag (2160p)
    #[serde(default)]
    pub is_4_k: bool,
    /// Video type (VideoFile, Iso, Ifo)
    #[serde(default)]
    pub video_type: Option<VideoType>,
}

impl MediaItem {
    /// Returns a MediaItem with all DTO fields set to their default values.
    /// Used with struct update syntax (`..MediaItem::default_dto_fields()`) when
    /// constructing MediaItem instances that only set the core fields.
    pub fn default_dto_fields() -> Self {
        Self {
            id: Uuid::nil(),
            name: String::new(),
            sort_name: None,
            overview: None,
            media_type: MediaType::Unknown,
            path: String::new(),
            parent_id: None,
            library_id: Uuid::nil(),
            production_year: None,
            community_rating: None,
            official_rating: None,
            genres: Vec::new(),
            date_created: Utc::now(),
            premiere_date: None,
            runtime_ticks: None,
            index_number: None,
            parent_index_number: None,
            media_sources: Vec::new(),
            // --- MediaItem DTO Fields (defaults) ---
            user_data: None,
            image_tags: std::collections::HashMap::new(),
            backdrop_image_tags: Vec::new(),
            parent_backdrop_image_tags: Vec::new(),
            primary_image_aspect_ratio: None,
            can_delete: false,
            can_download: false,
            container: None,
            bitrate: None,
            video_3_d: false,
            end_date: None,
            series_name: None,
            series_id: None,
            season_name: None,
            status: None,
            custom_rating: None,
            critic_rating: None,
            audience_rating: None,
            tags: Vec::new(),
            people: Vec::new(),
            studios: Vec::new(),
            genre_items: Vec::new(),
            forced_sort_name: None,
            width: None,
            height: None,
            aspect_ratio: None,
            chapter_count: 0,
            media_source_count: 0,
            location_type: LocationType::FileSystem,
            intro_start_time_ticks: None,
            intro_end_time_ticks: None,
            is_hd: false,
            is_4_k: false,
            video_type: None,
        }
    }
}

impl Serialize for MediaItem {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("MediaItem", 44)?;
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
        s.serialize_field("MediaSources", &self.media_sources)?;
        // --- MediaItem DTO Fields ---
        s.serialize_field("UserData", &self.user_data)?;
        s.serialize_field("ImageTags", &self.image_tags)?;
        s.serialize_field("BackdropImageTags", &self.backdrop_image_tags)?;
        s.serialize_field("ParentBackdropImageTags", &self.parent_backdrop_image_tags)?;
        s.serialize_field("PrimaryImageAspectRatio", &self.primary_image_aspect_ratio)?;
        s.serialize_field("CanDelete", &self.can_delete)?;
        s.serialize_field("CanDownload", &self.can_download)?;
        s.serialize_field("Container", &self.container)?;
        s.serialize_field("Bitrate", &self.bitrate)?;
        s.serialize_field("Video3D", &self.video_3_d)?;
        s.serialize_field("EndDate", &self.end_date)?;
        s.serialize_field("SeriesName", &self.series_name)?;
        s.serialize_field("SeriesId", &self.series_id)?;
        s.serialize_field("SeasonName", &self.season_name)?;
        s.serialize_field("Status", &self.status)?;
        s.serialize_field("CustomRating", &self.custom_rating)?;
        s.serialize_field("CriticRating", &self.critic_rating)?;
        s.serialize_field("AudienceRating", &self.audience_rating)?;
        s.serialize_field("Tags", &self.tags)?;
        s.serialize_field("People", &self.people)?;
        s.serialize_field("Studios", &self.studios)?;
        s.serialize_field("GenreItems", &self.genre_items)?;
        s.serialize_field("ForcedSortName", &self.forced_sort_name)?;
        s.serialize_field("Width", &self.width)?;
        s.serialize_field("Height", &self.height)?;
        s.serialize_field("AspectRatio", &self.aspect_ratio)?;
        s.serialize_field("ChapterCount", &self.chapter_count)?;
        s.serialize_field("MediaSourceCount", &self.media_source_count)?;
        s.serialize_field("LocationType", &self.location_type.as_str())?;
        s.serialize_field("IntroStartTimeTicks", &self.intro_start_time_ticks)?;
        s.serialize_field("IntroEndTimeTicks", &self.intro_end_time_ticks)?;
        s.serialize_field("IsHD", &self.is_hd)?;
        s.serialize_field("Is4K", &self.is_4_k)?;
        s.serialize_field("VideoType", &self.video_type.as_ref().map(|v| v.as_str()))?;
        s.end()
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct QueryResult {
    pub items: Vec<MediaItem>,
    pub total_record_count: i64,
    pub start_index: i64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct FiltersResult {
    pub genres: Vec<String>,
    pub years: Vec<i32>,
    pub official_ratings: Vec<String>,
}
