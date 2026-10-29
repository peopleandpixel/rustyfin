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

#[cfg(test)]
mod tests {
    use super::*;

    // ── ImageType::as_str ──

    #[test]
    fn test_image_type_as_str_primary() {
        assert_eq!(ImageType::Primary.as_str(), "Primary");
    }

    #[test]
    fn test_image_type_as_str_backdrop() {
        assert_eq!(ImageType::Backdrop.as_str(), "Backdrop");
    }

    #[test]
    fn test_image_type_as_str_banner() {
        assert_eq!(ImageType::Banner.as_str(), "Banner");
    }

    #[test]
    fn test_image_type_as_str_thumb() {
        assert_eq!(ImageType::Thumb.as_str(), "Thumb");
    }

    #[test]
    fn test_image_type_as_str_logo() {
        assert_eq!(ImageType::Logo.as_str(), "Logo");
    }

    #[test]
    fn test_image_type_as_str_art() {
        assert_eq!(ImageType::Art.as_str(), "Art");
    }

    #[test]
    fn test_image_type_as_str_disc() {
        assert_eq!(ImageType::Disc.as_str(), "Disc");
    }

    #[test]
    fn test_image_type_as_str_screenshot() {
        assert_eq!(ImageType::Screenshot.as_str(), "Screenshot");
    }

    // ── ImageType::from_str ──

    #[test]
    fn test_image_type_from_str_primary() {
        assert_eq!(ImageType::from_str("primary"), Some(ImageType::Primary));
    }

    #[test]
    fn test_image_type_from_str_backdrop() {
        assert_eq!(ImageType::from_str("backdrop"), Some(ImageType::Backdrop));
    }

    #[test]
    fn test_image_type_from_str_fanart() {
        assert_eq!(ImageType::from_str("fanart"), Some(ImageType::Backdrop));
    }

    #[test]
    fn test_image_type_from_str_banner() {
        assert_eq!(ImageType::from_str("banner"), Some(ImageType::Banner));
    }

    #[test]
    fn test_image_type_from_str_thumb() {
        assert_eq!(ImageType::from_str("thumb"), Some(ImageType::Thumb));
    }

    #[test]
    fn test_image_type_from_str_thumbnail() {
        assert_eq!(ImageType::from_str("thumbnail"), Some(ImageType::Thumb));
    }

    #[test]
    fn test_image_type_from_str_logo() {
        assert_eq!(ImageType::from_str("logo"), Some(ImageType::Logo));
    }

    #[test]
    fn test_image_type_from_str_art() {
        assert_eq!(ImageType::from_str("art"), Some(ImageType::Art));
    }

    #[test]
    fn test_image_type_from_str_clearart() {
        assert_eq!(ImageType::from_str("clearart"), Some(ImageType::Art));
    }

    #[test]
    fn test_image_type_from_str_disc() {
        assert_eq!(ImageType::from_str("disc"), Some(ImageType::Disc));
    }

    #[test]
    fn test_image_type_from_str_discart() {
        assert_eq!(ImageType::from_str("discart"), Some(ImageType::Disc));
    }

    #[test]
    fn test_image_type_from_str_screenshot() {
        assert_eq!(ImageType::from_str("screenshot"), Some(ImageType::Screenshot));
    }

    #[test]
    fn test_image_type_from_str_unknown() {
        assert_eq!(ImageType::from_str("unknown"), None);
    }

    #[test]
    fn test_image_type_from_str_case_insensitive() {
        assert_eq!(ImageType::from_str("PRIMARY"), Some(ImageType::Primary));
        assert_eq!(ImageType::from_str("Backdrop"), Some(ImageType::Backdrop));
    }

    // ── detect_image_type_from_filename ──

    #[test]
    fn test_detect_poster() {
        assert_eq!(
            detect_image_type_from_filename("/path/to/poster.jpg"),
            Some(ImageType::Primary)
        );
    }

    #[test]
    fn test_detect_cover() {
        assert_eq!(
            detect_image_type_from_filename("/path/to/cover.png"),
            Some(ImageType::Primary)
        );
    }

    #[test]
    fn test_detect_fanart() {
        assert_eq!(
            detect_image_type_from_filename("/path/to/fanart.jpg"),
            Some(ImageType::Backdrop)
        );
    }

    #[test]
    fn test_detect_backdrop() {
        assert_eq!(
            detect_image_type_from_filename("/path/to/backdrop.jpg"),
            Some(ImageType::Backdrop)
        );
    }

    #[test]
    fn test_detect_banner() {
        assert_eq!(
            detect_image_type_from_filename("/path/to/banner.jpg"),
            Some(ImageType::Banner)
        );
    }

    #[test]
    fn test_detect_thumb() {
        assert_eq!(
            detect_image_type_from_filename("/path/to/thumb.jpg"),
            Some(ImageType::Thumb)
        );
    }

    #[test]
    fn test_detect_logo() {
        assert_eq!(
            detect_image_type_from_filename("/path/to/logo.png"),
            Some(ImageType::Logo)
        );
    }

    #[test]
    fn test_detect_clearlogo() {
        assert_eq!(
            detect_image_type_from_filename("/path/to/clearlogo.png"),
            Some(ImageType::Logo)
        );
    }

    #[test]
    fn test_detect_clearart() {
        assert_eq!(
            detect_image_type_from_filename("/path/to/clearart.png"),
            Some(ImageType::Art)
        );
    }

    #[test]
    fn test_detect_disc() {
        assert_eq!(
            detect_image_type_from_filename("/path/to/disc.jpg"),
            Some(ImageType::Disc)
        );
    }

    #[test]
    fn test_detect_keyword_in_filename() {
        // File with keyword embedded in name
        assert_eq!(
            detect_image_type_from_filename("/path/to/movie-poster.jpg"),
            Some(ImageType::Primary)
        );
    }

    #[test]
    fn test_detect_unknown_filename() {
        assert_eq!(
            detect_image_type_from_filename("/path/to/random.jpg"),
            None
        );
    }

    #[test]
    fn test_detect_with_extension() {
        assert_eq!(
            detect_image_type_from_filename("poster.jpg"),
            Some(ImageType::Primary)
        );
    }

    #[test]
    fn test_detect_folder_filename() {
        assert_eq!(
            detect_image_type_from_filename("/path/to/folder.jpg"),
            Some(ImageType::Primary)
        );
    }

    #[test]
    fn test_detect_default_filename() {
        assert_eq!(
            detect_image_type_from_filename("/path/to/default.jpg"),
            Some(ImageType::Primary)
        );
    }

    #[test]
    fn test_detect_landscape() {
        assert_eq!(
            detect_image_type_from_filename("/path/to/landscape.jpg"),
            Some(ImageType::Thumb)
        );
    }

    #[test]
    fn test_detect_background() {
        assert_eq!(
            detect_image_type_from_filename("/path/to/background.jpg"),
            Some(ImageType::Backdrop)
        );
    }

    #[test]
    fn test_detect_cdart() {
        assert_eq!(
            detect_image_type_from_filename("/path/to/cdart.jpg"),
            Some(ImageType::Disc)
        );
    }
}
