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
