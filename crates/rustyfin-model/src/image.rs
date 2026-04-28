use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageType {
    Primary,
    Backdrop,
    Banner,
    Thumb,
    Logo,
    Art,
    Disc,
    Screenshot,
}

impl ImageType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Primary => "Primary",
            Self::Backdrop => "Backdrop",
            Self::Banner => "Banner",
            Self::Thumb => "Thumb",
            Self::Logo => "Logo",
            Self::Art => "Art",
            Self::Disc => "Disc",
            Self::Screenshot => "Screenshot",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "primary" => Some(Self::Primary),
            "backdrop" | "fanart" => Some(Self::Backdrop),
            "banner" => Some(Self::Banner),
            "thumb" | "thumbnail" => Some(Self::Thumb),
            "logo" => Some(Self::Logo),
            "art" | "clearart" => Some(Self::Art),
            "disc" | "discart" => Some(Self::Disc),
            "screenshot" => Some(Self::Screenshot),
            _ => None,
        }
    }
}

/// Maps local file names to image types
pub fn detect_image_type_from_filename(filename: &str) -> Option<ImageType> {
    let lower = filename.to_lowercase();
    let stem = lower.rsplit('/').next().unwrap_or(&lower);
    let stem = stem.split('.').next().unwrap_or(stem);

    match stem {
        "poster" | "cover" | "folder" | "default" => Some(ImageType::Primary),
        "fanart" | "backdrop" | "background" => Some(ImageType::Backdrop),
        "banner" => Some(ImageType::Banner),
        "thumb" | "thumbnail" | "landscape" => Some(ImageType::Thumb),
        "logo" | "clearlogo" => Some(ImageType::Logo),
        "art" | "clearart" => Some(ImageType::Art),
        "disc" | "discart" | "cdart" => Some(ImageType::Disc),
        _ => None,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ItemImage {
    pub item_id: Uuid,
    pub image_type: ImageType,
    /// Index for multiple images of the same type (e.g. multiple backdrops)
    pub image_index: i32,
    /// Local file path or remote URL
    pub path: String,
    /// Provider that supplied this image (local, tmdb, omdb)
    pub provider: String,
    /// Image width in pixels (if known)
    pub width: Option<i32>,
    /// Image height in pixels (if known)
    pub height: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ImageInfo {
    pub image_type: ImageType,
    pub image_index: i32,
    pub image_tag: String,
    pub path: String,
    pub width: Option<i32>,
    pub height: Option<i32>,
}
