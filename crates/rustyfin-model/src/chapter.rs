use serde::{Deserialize, Serialize};

/// A chapter marker in a video file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Chapter {
    /// Unique ID
    pub id: String,
    /// Item ID this chapter belongs to
    pub item_id: String,
    /// Chapter number (1-based)
    #[serde(rename = "ChapterNumber")]
    pub chapter_number: i32,
    /// Chapter name
    pub name: String,
    /// Start time in ticks (100-nanosecond units)
    #[serde(rename = "StartPositionTicks")]
    pub start_position_ticks: i64,
    /// End time in ticks (100-nanosecond units)
    #[serde(rename = "EndPositionTicks")]
    pub end_position_ticks: i64,
    /// The chapter image's path; empty when extracting it failed, so it isn't tried again
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_path: Option<String>,
    /// When the chapter image was made (RFC 3339)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_date_modified: Option<String>,
}
