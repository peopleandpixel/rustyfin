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
}

/// Library options for per-library metadata and image providers.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
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
    /// Enable automatic chapter extraction
    #[serde(default)]
    pub enable_chapter_extraction: bool,
    /// Enable trickplay generation
    #[serde(default)]
    pub enable_trickplay: bool,
    /// Enable intro detection
    #[serde(default)]
    pub enable_intro_detection: bool,
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
