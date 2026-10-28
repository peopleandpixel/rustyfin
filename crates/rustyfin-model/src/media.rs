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

/// External URL (IMDb, TMDb, etc.)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ExternalUrl {
    pub name: String,
    pub url: String,
}

/// Provider IDs (TMDb, TVDb, IMDb, etc.) stored as a map
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(transparent)]
pub struct ProviderIds {
    #[serde(flatten)]
    pub ids: std::collections::HashMap<String, String>,
}

impl ProviderIds {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.ids.get(key).map(|s| s.as_str())
    }

    pub fn set(&mut self, key: String, value: String) {
        self.ids.insert(key, value);
    }
}

/// Chapter info for media items
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ChapterInfo {
    pub start_ticks: i64,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_date_modified: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
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
    /// TMDb ID for the item, used to fetch cast/crew photos and fanart.tv images
    pub tmdb_id: Option<i64>,
    /// TheTVDB ID for the item, used to fetch fanart.tv images for TV shows
    pub tvdb_id: Option<i64>,
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

    // --- Additional BaseItemDto fields ---
    /// Server ID (required by clients)
    #[serde(default)]
    pub server_id: Option<String>,
    /// Start date for live TV/programs
    #[serde(default)]
    pub start_date: Option<DateTime<Utc>>,
    /// Provider IDs (TMDb, TVDb, IMDb, etc.)
    #[serde(default)]
    pub provider_ids: ProviderIds,
    /// External URLs (IMDb, TMDb links)
    #[serde(default)]
    pub external_urls: Vec<ExternalUrl>,
    /// Remote trailers (YouTube URLs)
    #[serde(default)]
    pub remote_trailers: Vec<ExternalUrl>,
    /// Chapters array
    #[serde(default)]
    pub chapters: Vec<ChapterInfo>,
    /// Media streams (flattened from MediaSources for convenience)
    #[serde(default)]
    pub media_streams: Vec<crate::media_source::MediaStream>,
    /// Current program for live TV channels
    #[serde(default)]
    pub current_program: Option<Box<MediaItem>>,
    /// Timer ID for live TV
    #[serde(default)]
    pub timer_id: Option<Uuid>,
    /// Series timer ID for live TV
    #[serde(default)]
    pub series_timer_id: Option<Uuid>,
    /// Channel ID for live TV
    #[serde(default)]
    pub channel_id: Option<Uuid>,
    /// Channel name for live TV
    #[serde(default)]
    pub channel_name: Option<String>,
    /// Channel number for live TV
    #[serde(default)]
    pub channel_number: Option<String>,
    /// Album name for audio items
    #[serde(default)]
    pub album: Option<String>,
    /// Album ID for audio items
    #[serde(default)]
    pub album_id: Option<Uuid>,
    /// Album artist for audio items
    #[serde(default)]
    pub album_artist: Option<String>,
    /// Album artists array for audio items
    #[serde(default)]
    pub album_artists: Vec<PersonInfo>,
    /// Artists array for audio items
    #[serde(default)]
    pub artists: Vec<PersonInfo>,
    /// Artist items array
    #[serde(default)]
    pub artist_items: Vec<GenreItem>,
    /// Song count for folders
    #[serde(default)]
    pub song_count: Option<i32>,
    /// Album count for folders
    #[serde(default)]
    pub album_count: Option<i32>,
    /// Movie count for folders
    #[serde(default)]
    pub movie_count: Option<i32>,
    /// Series count for folders
    #[serde(default)]
    pub series_count: Option<i32>,
    /// Episode count for folders
    #[serde(default)]
    pub episode_count: Option<i32>,
    /// Music video count for folders
    #[serde(default)]
    pub music_video_count: Option<i32>,
    /// Photo count for folders
    #[serde(default)]
    pub photo_count: Option<i32>,
    /// Program count for folders
    #[serde(default)]
    pub program_count: Option<i32>,
    /// Child count for folders
    #[serde(default)]
    pub child_count: Option<i32>,
    /// Part count for multi-part episodes
    #[serde(default)]
    pub part_count: Option<i32>,
    /// Trailer count
    #[serde(default)]
    pub trailer_count: Option<i32>,
    /// Local trailer count
    #[serde(default)]
    pub local_trailer_count: Option<i32>,
    /// Special feature count
    #[serde(default)]
    pub special_feature_count: Option<i32>,
    /// Extra type (Trailer, Scene, ThemeSong, etc.)
    #[serde(default)]
    pub extra_type: Option<String>,
    /// Display order for items
    #[serde(default)]
    pub display_order: Option<String>,
    /// Display preferences ID
    #[serde(default)]
    pub display_preferences_id: Option<String>,
    /// ETag for cache validation
    #[serde(default)]
    pub etag: Option<String>,
    /// Source type (Default, Gaps)
    #[serde(default)]
    pub source_type: Option<String>,
    /// Playlist item ID
    #[serde(default)]
    pub playlist_item_id: Option<Uuid>,
    /// Date last media was added to folder
    #[serde(default)]
    pub date_last_media_added: Option<DateTime<Utc>>,
    /// Completion percentage (0-100)
    #[serde(default)]
    pub completion_percentage: Option<f64>,
    /// Cumulative runtime ticks for folders
    #[serde(default)]
    pub cumulative_run_time_ticks: Option<i64>,
    /// Enable media source display
    #[serde(default)]
    pub enable_media_source_display: bool,
    /// Is kids content
    #[serde(default)]
    pub is_kids: bool,
    /// Is live content
    #[serde(default)]
    pub is_live: bool,
    /// Is movie boolean flag
    #[serde(default)]
    pub is_movie: bool,
    /// Is news content
    #[serde(default)]
    pub is_news: bool,
    /// Is placeholder
    #[serde(default)]
    pub is_placeholder: bool,
    /// Is premiere
    #[serde(default)]
    pub is_premiere: bool,
    /// Is repeat
    #[serde(default)]
    pub is_repeat: bool,
    /// Is series boolean flag
    #[serde(default)]
    pub is_series: bool,
    /// Is sports content
    #[serde(default)]
    pub is_sports: bool,
    /// Lock data flag
    #[serde(default)]
    pub lock_data: bool,
    /// Locked fields array
    #[serde(default)]
    pub locked_fields: Vec<String>,
    /// Display number for episodes (e.g., "103" for S01E03)
    #[serde(default)]
    pub number: Option<String>,
    /// Episode title
    #[serde(default)]
    pub episode_title: Option<String>,
    /// Series primary image tag
    #[serde(default)]
    pub series_primary_image_tag: Option<String>,
    /// Series thumb image tag
    #[serde(default)]
    pub series_thumb_image_tag: Option<String>,
    /// Series studio name
    #[serde(default)]
    pub series_studio: Option<String>,
    /// Original title
    #[serde(default)]
    pub original_title: Option<String>,
    /// Original language code
    #[serde(default)]
    pub original_language: Option<String>,
    /// Preferred metadata country code
    #[serde(default)]
    pub preferred_metadata_country_code: Option<String>,
    /// Preferred metadata language
    #[serde(default)]
    pub preferred_metadata_language: Option<String>,
    /// Production locations
    #[serde(default)]
    pub production_locations: Vec<String>,
    /// Taglines
    #[serde(default)]
    pub taglines: Vec<String>,
    /// Budget in USD (movies)
    #[serde(default)]
    pub budget: Option<i64>,
    /// Revenue in USD (movies)
    #[serde(default)]
    pub revenue: Option<i64>,
    /// Official homepage URL
    #[serde(default)]
    pub homepage: Option<String>,
    /// Number of votes for the rating
    #[serde(default)]
    pub vote_count: Option<i32>,
    /// Popularity score from TMDb
    #[serde(default)]
    pub popularity: Option<f64>,
    /// Parent backdrop item ID
    #[serde(default)]
    pub parent_backdrop_item_id: Option<Uuid>,
    /// Parent primary image tag
    #[serde(default)]
    pub parent_primary_image_tag: Option<String>,
    /// Parent primary image item ID
    #[serde(default)]
    pub parent_primary_image_item_id: Option<Uuid>,
    /// Parent thumb image tag
    #[serde(default)]
    pub parent_thumb_image_tag: Option<String>,
    /// Parent thumb item ID
    #[serde(default)]
    pub parent_thumb_item_id: Option<Uuid>,
    /// Parent art image tag
    #[serde(default)]
    pub parent_art_image_tag: Option<String>,
    /// Parent art item ID
    #[serde(default)]
    pub parent_art_item_id: Option<Uuid>,
    /// Parent logo image tag
    #[serde(default)]
    pub parent_logo_image_tag: Option<String>,
    /// Parent logo item ID
    #[serde(default)]
    pub parent_logo_item_id: Option<Uuid>,
    /// Screenshot image tags for trickplay
    #[serde(default)]
    pub screenshot_image_tags: Vec<String>,
    /// Image blur hashes
    #[serde(default)]
    pub image_blur_hashes: std::collections::HashMap<String, String>,
    /// Video 3D format
    #[serde(default)]
    pub video_3_d_format: Option<String>,
    /// Play access flag
    #[serde(default)]
    pub play_access: Option<String>,
    /// Official rating image tag
    #[serde(default)]
    pub official_rating_image_tag: Option<String>,
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
            tmdb_id: None,
            tvdb_id: None,
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
            // --- Additional BaseItemDto fields (defaults) ---
            server_id: None,
            start_date: None,
            provider_ids: ProviderIds::default(),
            external_urls: Vec::new(),
            remote_trailers: Vec::new(),
            chapters: Vec::new(),
            media_streams: Vec::new(),
            current_program: None,
            timer_id: None,
            series_timer_id: None,
            channel_id: None,
            channel_name: None,
            channel_number: None,
            album: None,
            album_id: None,
            album_artist: None,
            album_artists: Vec::new(),
            artists: Vec::new(),
            artist_items: Vec::new(),
            song_count: None,
            album_count: None,
            movie_count: None,
            series_count: None,
            episode_count: None,
            music_video_count: None,
            photo_count: None,
            program_count: None,
            child_count: None,
            part_count: None,
            trailer_count: None,
            local_trailer_count: None,
            special_feature_count: None,
            extra_type: None,
            display_order: None,
            display_preferences_id: None,
            etag: None,
            source_type: None,
            playlist_item_id: None,
            date_last_media_added: None,
            completion_percentage: None,
            cumulative_run_time_ticks: None,
            enable_media_source_display: false,
            is_kids: false,
            is_live: false,
            is_movie: false,
            is_news: false,
            is_placeholder: false,
            is_premiere: false,
            is_repeat: false,
            is_series: false,
            is_sports: false,
            lock_data: false,
            locked_fields: Vec::new(),
            number: None,
            episode_title: None,
            series_primary_image_tag: None,
            series_thumb_image_tag: None,
            series_studio: None,
            original_title: None,
            original_language: None,
            preferred_metadata_country_code: None,
            preferred_metadata_language: None,
            production_locations: Vec::new(),
            taglines: Vec::new(),
            budget: None,
            revenue: None,
            homepage: None,
            vote_count: None,
            popularity: None,
            parent_backdrop_item_id: None,
            parent_primary_image_tag: None,
            parent_primary_image_item_id: None,
            parent_thumb_image_tag: None,
            parent_thumb_item_id: None,
            parent_art_image_tag: None,
            parent_art_item_id: None,
            parent_logo_image_tag: None,
            parent_logo_item_id: None,
            screenshot_image_tags: Vec::new(),
            image_blur_hashes: std::collections::HashMap::new(),
            video_3_d_format: None,
            play_access: None,
            official_rating_image_tag: None,
        }
    }
}

impl Serialize for MediaItem {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("MediaItem", 160)?;
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
        // --- Additional BaseItemDto Fields ---
        s.serialize_field("ServerId", &self.server_id)?;
        s.serialize_field("StartDate", &self.start_date)?;
        s.serialize_field("ProviderIds", &self.provider_ids)?;
        s.serialize_field("ExternalUrls", &self.external_urls)?;
        s.serialize_field("RemoteTrailers", &self.remote_trailers)?;
        s.serialize_field("Chapters", &self.chapters)?;
        s.serialize_field("MediaStreams", &self.media_streams)?;
        s.serialize_field("CurrentProgram", &self.current_program)?;
        s.serialize_field("TimerId", &self.timer_id)?;
        s.serialize_field("SeriesTimerId", &self.series_timer_id)?;
        s.serialize_field("ChannelId", &self.channel_id)?;
        s.serialize_field("ChannelName", &self.channel_name)?;
        s.serialize_field("ChannelNumber", &self.channel_number)?;
        s.serialize_field("Album", &self.album)?;
        s.serialize_field("AlbumId", &self.album_id)?;
        s.serialize_field("AlbumArtist", &self.album_artist)?;
        s.serialize_field("AlbumArtists", &self.album_artists)?;
        s.serialize_field("Artists", &self.artists)?;
        s.serialize_field("ArtistItems", &self.artist_items)?;
        s.serialize_field("SongCount", &self.song_count)?;
        s.serialize_field("AlbumCount", &self.album_count)?;
        s.serialize_field("MovieCount", &self.movie_count)?;
        s.serialize_field("SeriesCount", &self.series_count)?;
        s.serialize_field("EpisodeCount", &self.episode_count)?;
        s.serialize_field("MusicVideoCount", &self.music_video_count)?;
        s.serialize_field("PhotoCount", &self.photo_count)?;
        s.serialize_field("ProgramCount", &self.program_count)?;
        s.serialize_field("ChildCount", &self.child_count)?;
        s.serialize_field("PartCount", &self.part_count)?;
        s.serialize_field("TrailerCount", &self.trailer_count)?;
        s.serialize_field("LocalTrailerCount", &self.local_trailer_count)?;
        s.serialize_field("SpecialFeatureCount", &self.special_feature_count)?;
        s.serialize_field("ExtraType", &self.extra_type)?;
        s.serialize_field("DisplayOrder", &self.display_order)?;
        s.serialize_field("DisplayPreferencesId", &self.display_preferences_id)?;
        s.serialize_field("Etag", &self.etag)?;
        s.serialize_field("SourceType", &self.source_type)?;
        s.serialize_field("PlaylistItemId", &self.playlist_item_id)?;
        s.serialize_field("DateLastMediaAdded", &self.date_last_media_added)?;
        s.serialize_field("CompletionPercentage", &self.completion_percentage)?;
        s.serialize_field("CumulativeRunTimeTicks", &self.cumulative_run_time_ticks)?;
        s.serialize_field(
            "EnableMediaSourceDisplay",
            &self.enable_media_source_display,
        )?;
        s.serialize_field("IsKids", &self.is_kids)?;
        s.serialize_field("IsLive", &self.is_live)?;
        s.serialize_field("IsMovie", &self.is_movie)?;
        s.serialize_field("IsNews", &self.is_news)?;
        s.serialize_field("IsPlaceHolder", &self.is_placeholder)?;
        s.serialize_field("IsPremiere", &self.is_premiere)?;
        s.serialize_field("IsRepeat", &self.is_repeat)?;
        s.serialize_field("IsSeries", &self.is_series)?;
        s.serialize_field("IsSports", &self.is_sports)?;
        s.serialize_field("LockData", &self.lock_data)?;
        s.serialize_field("LockedFields", &self.locked_fields)?;
        s.serialize_field("Number", &self.number)?;
        s.serialize_field("EpisodeTitle", &self.episode_title)?;
        s.serialize_field("SeriesPrimaryImageTag", &self.series_primary_image_tag)?;
        s.serialize_field("SeriesThumbImageTag", &self.series_thumb_image_tag)?;
        s.serialize_field("SeriesStudio", &self.series_studio)?;
        s.serialize_field("OriginalTitle", &self.original_title)?;
        s.serialize_field("OriginalLanguage", &self.original_language)?;
        s.serialize_field(
            "PreferredMetadataCountryCode",
            &self.preferred_metadata_country_code,
        )?;
        s.serialize_field(
            "PreferredMetadataLanguage",
            &self.preferred_metadata_language,
        )?;
        s.serialize_field("ProductionLocations", &self.production_locations)?;
        s.serialize_field("Taglines", &self.taglines)?;
        s.serialize_field("Budget", &self.budget)?;
        s.serialize_field("Revenue", &self.revenue)?;
        s.serialize_field("Homepage", &self.homepage)?;
        s.serialize_field("VoteCount", &self.vote_count)?;
        s.serialize_field("Popularity", &self.popularity)?;
        s.serialize_field("ParentBackdropItemId", &self.parent_backdrop_item_id)?;
        s.serialize_field("ParentPrimaryImageTag", &self.parent_primary_image_tag)?;
        s.serialize_field(
            "ParentPrimaryImageItemId",
            &self.parent_primary_image_item_id,
        )?;
        s.serialize_field("ParentThumbImageTag", &self.parent_thumb_image_tag)?;
        s.serialize_field("ParentThumbItemId", &self.parent_thumb_item_id)?;
        s.serialize_field("ParentArtImageTag", &self.parent_art_image_tag)?;
        s.serialize_field("ParentArtItemId", &self.parent_art_item_id)?;
        s.serialize_field("ParentLogoImageTag", &self.parent_logo_image_tag)?;
        s.serialize_field("ParentLogoItemId", &self.parent_logo_item_id)?;
        s.serialize_field("ScreenshotImageTags", &self.screenshot_image_tags)?;
        s.serialize_field("ImageBlurHashes", &self.image_blur_hashes)?;
        s.serialize_field("Video3DFormat", &self.video_3_d_format)?;
        s.serialize_field("PlayAccess", &self.play_access)?;
        s.serialize_field("OfficialRatingImageTag", &self.official_rating_image_tag)?;
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
