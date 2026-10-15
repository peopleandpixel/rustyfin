use serde::{Deserialize, Serialize};

/// Trickplay preview info for an item.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct TrickplayInfo {
    /// Item ID
    pub item_id: String,
    /// Thumbnail width
    pub width: u32,
    /// Thumbnail height
    pub height: u32,
    /// Interval between thumbnails in ticks
    #[serde(rename = "IntervalTicks")]
    pub interval_ticks: i64,
    /// Number of thumbnails
    #[serde(rename = "ThumbnailCount")]
    pub thumbnail_count: u32,
    /// Directory containing the thumbnails
    #[serde(rename = "DirectoryPath")]
    pub directory_path: String,
}
