use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

    #[allow(clippy::should_implement_trait)]
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

    // Also check if the filename contains common image type keywords
    let contains_primary = stem.contains("poster")
        || stem.contains("cover")
        || stem.contains("folder")
        || stem.contains("default")
        || stem.contains("cover_art");
    let contains_backdrop = stem.contains("fanart")
        || stem.contains("backdrop")
        || stem.contains("background")
        || stem.contains("artwork");
    let contains_banner = stem.contains("banner");
    let contains_thumb =
        stem.contains("thumb") || stem.contains("thumbnail") || stem.contains("landscape");
    let contains_logo = stem.contains("logo") || stem.contains("clearlogo");
    let contains_art = stem.contains("clearart")
        || (stem.contains("art") && !stem.contains("fanart") && !stem.contains("clearlogo"));
    let contains_disc = stem.contains("disc") || stem.contains("cdart");

    match stem {
        "poster" | "cover" | "folder" | "default" | "cover_art" => Some(ImageType::Primary),
        "fanart" | "backdrop" | "background" | "artwork" => Some(ImageType::Backdrop),
        "banner" => Some(ImageType::Banner),
        "thumb" | "thumbnail" | "landscape" => Some(ImageType::Thumb),
        "logo" | "clearlogo" => Some(ImageType::Logo),
        "clearart" | "art" => Some(ImageType::Art),
        "disc" | "discart" | "cdart" => Some(ImageType::Disc),
        _ => {
            // Fallback: check for keywords in filename
            if contains_primary {
                Some(ImageType::Primary)
            } else if contains_backdrop {
                Some(ImageType::Backdrop)
            } else if contains_banner {
                Some(ImageType::Banner)
            } else if contains_thumb {
                Some(ImageType::Thumb)
            } else if contains_logo {
                Some(ImageType::Logo)
            } else if contains_art {
                Some(ImageType::Art)
            } else if contains_disc {
                Some(ImageType::Disc)
            } else {
                None
            }
        }
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
