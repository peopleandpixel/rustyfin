use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::media::MediaType;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LibraryContentType {
    Movies,
    TvShows,
    Music,
    Photos,
    Mixed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Library {
    pub id: Uuid,
    pub name: String,
    pub content_type: LibraryContentType,
    pub paths: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CreateLibraryRequest {
    pub name: String,
    pub content_type: LibraryContentType,
    pub paths: Vec<String>,
    /// Options to save for the new library; the defaults when absent.
    #[serde(default)]
    pub library_options: Option<LibraryOptions>,
}

/// Library options for per-library metadata and image providers.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", from = "LibraryOptionsIn")]
pub struct LibraryOptions {
    /// Metadata provider (e.g., "TMDb", "TheMovieDb", "TheOpenMovieDatabase")
    #[serde(default)]
    pub metadata_provider: String,
    /// Image provider (e.g., "TMDb", "TheMovieDb")
    #[serde(default)]
    pub image_provider: String,
    /// Skip subfolders during scanning
    #[serde(default)]
    pub skip_subfolders: bool,
    /// Make chapter images (Jellyfin's EnableChapterImageExtraction), in the nightly task
    #[serde(default)]
    pub enable_chapter_extraction: bool,
    /// Also make chapter images after each scan of the library
    #[serde(default)]
    pub extract_chapter_images_during_library_scan: bool,
    /// Make trickplay tiles (Jellyfin's EnableTrickplayImageExtraction), in the nightly task
    #[serde(default)]
    pub enable_trickplay: bool,
    /// Also make trickplay tiles after each scan of the library
    #[serde(default)]
    pub extract_trickplay_images_during_library_scan: bool,
    /// Enable intro detection
    #[serde(default)]
    pub enable_intro_detection: bool,
    /// Watch the library folders for changes in real time
    #[serde(default = "default_true")]
    pub enable_realtime_monitor: bool,
}

impl Default for LibraryOptions {
    fn default() -> Self {
        Self {
            metadata_provider: String::new(),
            image_provider: String::new(),
            skip_subfolders: false,
            enable_chapter_extraction: false,
            extract_chapter_images_during_library_scan: false,
            enable_trickplay: false,
            extract_trickplay_images_during_library_scan: false,
            enable_intro_detection: false,
            enable_realtime_monitor: true,
        }
    }
}

/// `LibraryOptions` as a request sends it: chapter images and trickplay under Rustyfin's name,
/// Jellyfin's, or both. Jellyfin's name wins when both are sent, since Jellyfin clients set it.
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct LibraryOptionsIn {
    #[serde(default)]
    metadata_provider: String,
    #[serde(default)]
    image_provider: String,
    #[serde(default)]
    skip_subfolders: bool,
    #[serde(default)]
    enable_chapter_extraction: Option<bool>,
    #[serde(default)]
    enable_chapter_image_extraction: Option<bool>,
    #[serde(default)]
    extract_chapter_images_during_library_scan: bool,
    #[serde(default)]
    enable_trickplay: Option<bool>,
    #[serde(default)]
    enable_trickplay_image_extraction: Option<bool>,
    #[serde(default)]
    extract_trickplay_images_during_library_scan: bool,
    #[serde(default)]
    enable_intro_detection: bool,
    #[serde(default = "default_true")]
    enable_realtime_monitor: bool,
}

impl From<LibraryOptionsIn> for LibraryOptions {
    fn from(o: LibraryOptionsIn) -> Self {
        Self {
            metadata_provider: o.metadata_provider,
            image_provider: o.image_provider,
            skip_subfolders: o.skip_subfolders,
            enable_chapter_extraction: o
                .enable_chapter_image_extraction
                .or(o.enable_chapter_extraction)
                .unwrap_or(false),
            extract_chapter_images_during_library_scan: o
                .extract_chapter_images_during_library_scan,
            enable_trickplay: o
                .enable_trickplay_image_extraction
                .or(o.enable_trickplay)
                .unwrap_or(false),
            extract_trickplay_images_during_library_scan: o
                .extract_trickplay_images_during_library_scan,
            enable_intro_detection: o.enable_intro_detection,
            enable_realtime_monitor: o.enable_realtime_monitor,
        }
    }
}

fn default_true() -> bool {
    true
}

impl LibraryContentType {
    pub fn default_media_type(&self) -> MediaType {
        match self {
            Self::Movies => MediaType::Movie,
            Self::TvShows => MediaType::Series,
            Self::Music => MediaType::Audio,
            Self::Photos => MediaType::Photo,
            Self::Mixed => MediaType::Unknown,
        }
    }

    /// Lowercase Jellyfin `CollectionType` value used by clients
    /// (e.g. "movies", "tvshows", "music", "homevideos", "mixed").
    pub fn jellyfin_collection_type(&self) -> &'static str {
        match self {
            Self::Movies => "movies",
            Self::TvShows => "tvshows",
            Self::Music => "music",
            Self::Photos => "homevideos",
            Self::Mixed => "mixed",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn r2chapters_options_take_either_name_and_prefer_jellyfins() {
        let read = |json: &str| serde_json::from_str::<LibraryOptions>(json).unwrap();
        let o = read(r#"{"EnableChapterExtraction":true,"EnableTrickplay":true}"#);
        assert!(o.enable_chapter_extraction && o.enable_trickplay);
        let o =
            read(r#"{"EnableChapterImageExtraction":true,"EnableTrickplayImageExtraction":true}"#);
        assert!(o.enable_chapter_extraction && o.enable_trickplay);
        // Both names in one body: no "duplicate field", and Jellyfin's wins.
        let o = read(
            r#"{"EnableChapterExtraction":false,"EnableChapterImageExtraction":true,
                "EnableTrickplay":true,"EnableTrickplayImageExtraction":false}"#,
        );
        assert!(o.enable_chapter_extraction);
        assert!(!o.enable_trickplay);
        let o = read("{}");
        assert!(!o.enable_trickplay && o.enable_realtime_monitor);
        // What is stored reads back the same.
        let back = read(&serde_json::to_string(&o).unwrap());
        assert_eq!(back.enable_realtime_monitor, o.enable_realtime_monitor);
    }

    // ── LibraryContentType::default_media_type ──

    #[test]
    fn test_default_media_type_movies() {
        assert_eq!(
            LibraryContentType::Movies.default_media_type(),
            MediaType::Movie
        );
    }

    #[test]
    fn test_default_media_type_tv_shows() {
        assert_eq!(
            LibraryContentType::TvShows.default_media_type(),
            MediaType::Series
        );
    }

    #[test]
    fn test_default_media_type_music() {
        assert_eq!(
            LibraryContentType::Music.default_media_type(),
            MediaType::Audio
        );
    }

    #[test]
    fn test_default_media_type_photos() {
        assert_eq!(
            LibraryContentType::Photos.default_media_type(),
            MediaType::Photo
        );
    }

    #[test]
    fn test_default_media_type_mixed() {
        assert_eq!(
            LibraryContentType::Mixed.default_media_type(),
            MediaType::Unknown
        );
    }

    // ── LibraryContentType::jellyfin_collection_type ──

    #[test]
    fn test_jellyfin_collection_type_movies() {
        assert_eq!(
            LibraryContentType::Movies.jellyfin_collection_type(),
            "movies"
        );
    }

    #[test]
    fn test_jellyfin_collection_type_tv_shows() {
        assert_eq!(
            LibraryContentType::TvShows.jellyfin_collection_type(),
            "tvshows"
        );
    }

    #[test]
    fn test_jellyfin_collection_type_music() {
        assert_eq!(
            LibraryContentType::Music.jellyfin_collection_type(),
            "music"
        );
    }

    #[test]
    fn test_jellyfin_collection_type_photos() {
        assert_eq!(
            LibraryContentType::Photos.jellyfin_collection_type(),
            "homevideos"
        );
    }

    #[test]
    fn test_jellyfin_collection_type_mixed() {
        assert_eq!(
            LibraryContentType::Mixed.jellyfin_collection_type(),
            "mixed"
        );
    }
}
