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
#[serde(rename_all = "PascalCase", from = "LibraryOptionsUpdate")]
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
    /// Jellyfin's other library options (metadata language and country, fetchers and their order,
    /// savers, subtitle options, …), kept as a client sent them so every field round-trips.
    #[serde(flatten)]
    pub other: serde_json::Map<String, serde_json::Value>,
}

/// Jellyfin's per-library fields that Rustyfin computes from the library itself, never stored.
const COMPUTED_FIELDS: [&str; 1] = ["PathInfos"];

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
            other: serde_json::Map::new(),
        }
    }
}

/// `LibraryOptions` as a request sends it: chapter images and trickplay under Rustyfin's name
/// (`EnableChapterExtraction`, `EnableTrickplay`, the one stored and returned), Jellyfin's
/// (`EnableChapterImageExtraction`, `EnableTrickplayImageExtraction`), or both. See `resolve` for
/// how both are read.
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct LibraryOptionsUpdate {
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
    #[serde(flatten)]
    other: serde_json::Map<String, serde_json::Value>,
}

/// One setting sent under two names. When both are sent and disagree, one of them is the value
/// stored now (a copy the client kept) and the other is the change, so the one that differs from
/// `stored` wins: a stale value under either name never overrides a fresh one under the other.
fn pick(ours: Option<bool>, jellyfin: Option<bool>, stored: bool) -> bool {
    match (ours, jellyfin) {
        (Some(a), Some(b)) if a != b => {
            if a != stored {
                a
            } else {
                b
            }
        }
        (Some(a), _) => a,
        (None, Some(b)) => b,
        (None, None) => false,
    }
}

impl LibraryOptionsUpdate {
    /// The options this update saves over `stored`. The update replaces them, as Jellyfin's does;
    /// `stored` only settles a setting sent under both its names with two values.
    pub fn resolve(self, stored: &LibraryOptions) -> LibraryOptions {
        let o = self;
        LibraryOptions {
            metadata_provider: o.metadata_provider,
            image_provider: o.image_provider,
            skip_subfolders: o.skip_subfolders,
            enable_chapter_extraction: pick(
                o.enable_chapter_extraction,
                o.enable_chapter_image_extraction,
                stored.enable_chapter_extraction,
            ),
            extract_chapter_images_during_library_scan: o
                .extract_chapter_images_during_library_scan,
            enable_trickplay: pick(
                o.enable_trickplay,
                o.enable_trickplay_image_extraction,
                stored.enable_trickplay,
            ),
            extract_trickplay_images_during_library_scan: o
                .extract_trickplay_images_during_library_scan,
            enable_intro_detection: o.enable_intro_detection,
            enable_realtime_monitor: o.enable_realtime_monitor,
            other: o
                .other
                .into_iter()
                .filter(|(k, _)| !COMPUTED_FIELDS.contains(&k.as_str()))
                .collect(),
        }
    }
}

impl From<LibraryOptionsUpdate> for LibraryOptions {
    /// Options read with nothing stored before (a new library, or the stored row itself).
    fn from(o: LibraryOptionsUpdate) -> Self {
        o.resolve(&LibraryOptions::default())
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
        // Both names in one body: no "duplicate field", and with nothing stored the one that
        // turns the setting on is the change (see r3tasks_both_names_the_change_wins).
        let o = read(
            r#"{"EnableChapterExtraction":false,"EnableChapterImageExtraction":true,
                "EnableTrickplay":true,"EnableTrickplayImageExtraction":false}"#,
        );
        assert!(o.enable_chapter_extraction);
        assert!(o.enable_trickplay);
        let o = read("{}");
        assert!(!o.enable_trickplay && o.enable_realtime_monitor);
        // What is stored reads back the same.
        let back = read(&serde_json::to_string(&o).unwrap());
        assert_eq!(back.enable_realtime_monitor, o.enable_realtime_monitor);
    }

    #[test]
    fn r3tasks_both_names_the_change_wins() {
        let update = |json: &str| serde_json::from_str::<LibraryOptionsUpdate>(json).unwrap();
        let on = LibraryOptions {
            enable_trickplay: true,
            enable_chapter_extraction: true,
            ..Default::default()
        };
        let off = LibraryOptions::default();
        // A stale copy under either name never overrides the change under the other.
        for (json, stored, want) in [
            (
                r#"{"EnableTrickplay":true,"EnableTrickplayImageExtraction":false}"#,
                &off,
                true,
            ),
            (
                r#"{"EnableTrickplay":false,"EnableTrickplayImageExtraction":true}"#,
                &off,
                true,
            ),
            (
                r#"{"EnableTrickplay":false,"EnableTrickplayImageExtraction":true}"#,
                &on,
                false,
            ),
            (
                r#"{"EnableTrickplay":true,"EnableTrickplayImageExtraction":false}"#,
                &on,
                false,
            ),
            (
                r#"{"EnableTrickplay":true,"EnableTrickplayImageExtraction":true}"#,
                &off,
                true,
            ),
            (r#"{"EnableTrickplay":true}"#, &off, true),
            (r#"{"EnableTrickplayImageExtraction":false}"#, &on, false),
            (r#"{}"#, &on, false),
        ] {
            assert_eq!(
                update(json).resolve(stored).enable_trickplay,
                want,
                "{json}"
            );
        }
        let o = update(r#"{"EnableChapterExtraction":true,"EnableChapterImageExtraction":false}"#)
            .resolve(&off);
        assert!(o.enable_chapter_extraction);
        // Only the canonical names are stored.
        let stored = serde_json::to_value(&o).unwrap();
        assert!(
            stored.get("EnableTrickplayImageExtraction").is_none(),
            "{stored}"
        );
        assert!(
            stored.get("EnableChapterImageExtraction").is_none(),
            "{stored}"
        );
        assert_eq!(stored["EnableChapterExtraction"], true);
    }

    #[test]
    fn r3dash3_jellyfin_options_round_trip() {
        let o: LibraryOptions = serde_json::from_str(
            r#"{"PreferredMetadataLanguage":"de","MetadataSavers":["Nfo"],
                "TypeOptions":[{"Type":"Movie","MetadataFetcherOrder":["TheMovieDb"]}],
                "PathInfos":[{"Path":"/old"}],"EnableRealtimeMonitor":false}"#,
        )
        .unwrap();
        assert!(!o.enable_realtime_monitor);
        let back = serde_json::to_value(&o).unwrap();
        assert_eq!(back["PreferredMetadataLanguage"], "de");
        assert_eq!(back["MetadataSavers"][0], "Nfo");
        assert_eq!(
            back["TypeOptions"][0]["MetadataFetcherOrder"][0],
            "TheMovieDb"
        );
        // The library's folders are its own, never a stored copy
        assert!(back.get("PathInfos").is_none(), "{back}");
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
